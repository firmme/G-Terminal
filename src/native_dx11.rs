//! Windows renderer: egui/winit input and accessibility, native D3D11 painting.
//! GPU resources are owned by this window and allocated to its actual size.
use anyhow::{Context as _, Result};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HMODULE, HWND},
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0},
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
        System::Threading::WaitForSingleObjectEx,
    },
    core::Interface,
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowId},
};

enum Event {
    Repaint(Instant),
    Accessibility(accesskit_winit::Event),
}
impl From<accesskit_winit::Event> for Event {
    fn from(value: accesskit_winit::Event) -> Self {
        Self::Accessibility(value)
    }
}

pub fn run(screenshot: Option<PathBuf>) -> Result<()> {
    let event_loop = EventLoop::<Event>::with_user_event().build()?;
    let mut runner = Runner {
        proxy: event_loop.create_proxy(),
        native: None,
        screenshot,
        error: None,
        repaint: None,
    };
    event_loop.run_app(&mut runner)?;
    if let Some(error) = runner.error {
        return Err(error);
    }
    Ok(())
}

/// The taskbar/Alt-Tab icon. `None` when winit rejects the bitmap: a missing icon
/// must never be a reason the window fails to open.
fn window_icon() -> Option<winit::window::Icon> {
    let icon = crate::icons::default_app_icon(64);
    winit::window::Icon::from_rgba(icon.rgba, icon.width, icon.height).ok()
}

struct Runner {
    proxy: EventLoopProxy<Event>,
    native: Option<Native>,
    screenshot: Option<PathBuf>,
    error: Option<anyhow::Error>,
    repaint: Option<Instant>,
}
impl Runner {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.error = Some(error);
        event_loop.exit();
    }
}
impl ApplicationHandler<Event> for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.native.is_some() {
            return;
        }
        let result = (|| {
            let window = Arc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title("G-Terminal")
                        .with_decorations(false)
                        .with_visible(false)
                        .with_window_icon(window_icon())
                        .with_inner_size(LogicalSize::new(DEFAULT_SIZE.0, DEFAULT_SIZE.1))
                        .with_min_inner_size(LogicalSize::new(MIN_SIZE.0, MIN_SIZE.1)),
                )?,
            );
            Native::new(
                window,
                event_loop,
                self.proxy.clone(),
                self.screenshot.take(),
            )
        })();
        match result {
            Ok(native) => {
                native.window.request_redraw();
                self.native = Some(native);
            }
            Err(error) => self.fail(event_loop, error),
        }
    }
    fn user_event(&mut self, _: &ActiveEventLoop, event: Event) {
        let Some(native) = &mut self.native else {
            return;
        };
        match event {
            Event::Repaint(at) => {
                if at <= Instant::now() {
                    native.window.request_redraw();
                } else {
                    self.repaint = Some(self.repaint.map_or(at, |old| old.min(at)));
                }
            }
            Event::Accessibility(event) if event.window_id == native.window.id() => {
                match event.window_event {
                    accesskit_winit::WindowEvent::InitialTreeRequested => {
                        native.ctx.enable_accesskit()
                    }
                    accesskit_winit::WindowEvent::ActionRequested(request) => {
                        native.input.on_accesskit_action_request(request)
                    }
                    accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                        native.ctx.disable_accesskit()
                    }
                }
                native.window.request_redraw();
            }
            _ => {}
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(native) = &mut self.native else {
            return;
        };
        if id != native.window.id() {
            return;
        }
        let response = native.input.on_window_event(&native.window, &event);
        if response.repaint {
            native.window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => {
                // Run one more frame and let the app decide: it either closes
                // the window itself or cancels the request to confirm first.
                native.close_requested = true;
                native.window.request_redraw();
            }
            WindowEvent::Resized(size) => {
                native.repair_geometry();
                if let Err(error) = native.gpu.resize(size.width, size.height) {
                    self.fail(event_loop, error);
                }
            }
            WindowEvent::Moved(_)
            | WindowEvent::ScaleFactorChanged { .. }
            | WindowEvent::Focused(true)
            | WindowEvent::Occluded(false) => native.repair_geometry(),
            WindowEvent::RedrawRequested => match native.draw() {
                Ok(true) => event_loop.exit(),
                Ok(false) => {}
                Err(error) => self.fail(event_loop, error),
            },
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(at) = self.repaint {
            if at <= Instant::now() {
                if let Some(native) = &self.native {
                    native.window.request_redraw();
                }
                self.repaint = None;
                event_loop.set_control_flow(ControlFlow::Wait);
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(at));
            }
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
    fn exiting(&mut self, _: &ActiveEventLoop) {
        if let Some(native) = &mut self.native {
            eframe::App::on_exit(&mut native.app);
        }
    }
}

/// The window's size when it opens, and the smallest it may become, in logical
/// pixels.
const DEFAULT_SIZE: (f64, f64) = (1280.0, 800.0);
const MIN_SIZE: (f64, f64) = (400.0, 300.0);

/// A rectangle in physical pixels: x, y, width, height.
type Area = (i32, i32, i32, i32);

/// Where a window that has ended up unusable should go instead, or `None` when
/// it is fine.
///
/// Logging back in at the machine after a Remote Desktop session changes the
/// resolution and DPI under the window, and Windows can leave it as a sliver a
/// few pixels high, or off every screen. Neither the title bar nor an edge can
/// then be grabbed, so the app puts it back itself: a window below the minimum
/// gets the default size, and one whose title bar is on no monitor is centred
/// on `home`. All values are physical pixels.
fn repaired_geometry(
    outer: Area,
    inner: (i32, i32),
    min: (i32, i32),
    default: (i32, i32),
    monitors: &[Area],
    home: Option<Area>,
) -> Option<Area> {
    let home = home.or_else(|| monitors.first().copied())?;
    let too_small = inner.0 < min.0 || inner.1 < min.1;
    // A piece of the title bar has to be on some screen to be draggable.
    let visible = monitors.iter().any(|&(mx, my, mw, mh)| {
        let left = outer.0.max(mx);
        let right = (outer.0 + outer.2).min(mx + mw);
        let top = outer.1.max(my);
        let bottom = (outer.1 + 40).min(my + mh);
        right - left >= 120 && bottom - top >= 20
    });
    if !too_small && visible {
        return None;
    }
    let (width, height) = if too_small {
        (
            default.0.min(home.2 * 9 / 10).max(min.0),
            default.1.min(home.3 * 9 / 10).max(min.1),
        )
    } else {
        inner
    };
    Some((
        home.0 + (home.2 - width).max(0) / 2,
        home.1 + (home.3 - height).max(0) / 2,
        width,
        height,
    ))
}

struct Native {
    // Drop GPU references before destroying the HWND.
    gpu: Gpu,
    app: crate::app::App,
    ctx: egui::Context,
    input: egui_winit::State,
    info: egui::ViewportInfo,
    window: Arc<Window>,
    /// The platform asked to close and the app has not answered yet. It is
    /// carried into the next frame's viewport info so the app can hold the
    /// close back with `CancelClose` — which is how the confirmation dialog
    /// gets its say. Nothing here exits on its own.
    close_requested: bool,
}
impl Native {
    /// Puts back a window that a display change left too small or off-screen.
    fn repair_geometry(&self) {
        let window = &self.window;
        if window.is_minimized() == Some(true) || window.fullscreen().is_some() {
            return;
        }
        let scale = window.scale_factor();
        let physical = |(w, h): (f64, f64)| ((w * scale) as i32, (h * scale) as i32);
        let inner = window.inner_size();
        let outer = window.outer_size();
        let Ok(position) = window.outer_position() else {
            return;
        };
        let area = |m: &winit::monitor::MonitorHandle| {
            let (p, s) = (m.position(), m.size());
            (p.x, p.y, s.width as i32, s.height as i32)
        };
        let monitors: Vec<Area> = window.available_monitors().map(|m| area(&m)).collect();
        let home = window
            .current_monitor()
            .or_else(|| window.primary_monitor())
            .map(|m| area(&m));
        let Some((x, y, width, height)) = repaired_geometry(
            (
                position.x,
                position.y,
                outer.width as i32,
                outer.height as i32,
            ),
            (inner.width as i32, inner.height as i32),
            physical(MIN_SIZE),
            physical(DEFAULT_SIZE),
            &monitors,
            home,
        ) else {
            return;
        };
        // A maximized window that came back tiny has to leave that state
        // first, or the new size is ignored.
        if window.is_maximized() {
            window.set_maximized(false);
        }
        window.set_min_inner_size(Some(LogicalSize::new(MIN_SIZE.0, MIN_SIZE.1)));
        let _ = window.request_inner_size(PhysicalSize::new(width as u32, height as u32));
        window.set_outer_position(PhysicalPosition::new(x, y));
        window.request_redraw();
    }

    fn new(
        window: Arc<Window>,
        event_loop: &ActiveEventLoop,
        proxy: EventLoopProxy<Event>,
        screenshot: Option<PathBuf>,
    ) -> Result<Self> {
        let gpu = Gpu::new(&window)?;
        let ctx = egui::Context::default();
        let mut input = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(8192),
        );
        input.init_accesskit(event_loop, &window, proxy.clone());
        window.set_visible(true);
        ctx.set_request_repaint_callback(move |request| {
            let delay = request.delay.min(Duration::from_secs(24 * 3600));
            let _ = proxy.send_event(Event::Repaint(Instant::now() + delay));
        });
        let app = crate::app::App::new_context(&ctx, screenshot);
        let mut info = egui::ViewportInfo::default();
        egui_winit::update_viewport_info(&mut info, &ctx, &window, true);
        Ok(Self {
            gpu,
            app,
            ctx,
            input,
            info,
            window,
            close_requested: false,
        })
    }
    fn draw(&mut self) -> Result<bool> {
        if self.window.is_minimized() == Some(true) || self.gpu.target.is_none() {
            // Nothing can be drawn, so the confirmation cannot be shown either;
            // a pending close request just goes through.
            return Ok(self.close_requested);
        }
        // Wait for the display to take a frame *before* reading input, so
        // what is drawn is the freshest pointer and key state there is.
        self.gpu.wait_for_frame();
        egui_winit::update_viewport_info(&mut self.info, &self.ctx, &self.window, false);
        // The close request rides in as a viewport event, which is where
        // `ViewportInfo::close_requested` looks for it.
        self.info.events.clear();
        if self.close_requested {
            self.info.events.push(egui::ViewportEvent::Close);
        }
        let mut raw = self.input.take_egui_input(&self.window);
        raw.viewports
            .insert(egui::ViewportId::ROOT, self.info.clone());
        let output = self.ctx.run(raw, |ctx| self.app.render(ctx));
        let (paint, platform, viewports) = egui_directx11::split_output(output);
        self.input.handle_platform_output(&self.window, platform);
        self.gpu.paint(&self.ctx, paint)?;
        let mut close = false;
        let mut cancelled = false;
        let mut actions = vec![];
        if let Some(viewport) = viewports.get(&egui::ViewportId::ROOT) {
            for command in &viewport.commands {
                match command {
                    egui::ViewportCommand::Close => close = true,
                    egui::ViewportCommand::CancelClose => cancelled = true,
                    _ => {}
                }
            }
            egui_winit::process_viewport_commands(
                &self.ctx,
                &mut self.info,
                viewport.commands.clone(),
                &self.window,
                &mut actions,
            );
        }
        if cancelled {
            self.close_requested = false;
            self.info.events.clear();
        }
        for action in actions {
            let event = match action {
                egui_winit::ActionRequested::Screenshot(user_data) => egui::Event::Screenshot {
                    viewport_id: egui::ViewportId::ROOT,
                    user_data,
                    image: Arc::new(self.gpu.capture()?),
                },
                egui_winit::ActionRequested::Copy => egui::Event::Copy,
                egui_winit::ActionRequested::Cut => egui::Event::Cut,
                egui_winit::ActionRequested::Paste => {
                    egui::Event::Paste(self.input.clipboard_text().unwrap_or_default())
                }
            };
            self.input.egui_input_mut().events.push(event);
            self.window.request_redraw();
        }
        // Screenshot must copy the rendered buffer before flip-discard Present.
        unsafe {
            self.gpu.swap.Present(1, DXGI_PRESENT(0)).ok()?;
        }
        // Close wins; a request the app cancelled keeps the window open.
        Ok(close || (self.close_requested && !cancelled))
    }
}

struct Gpu {
    target: Option<ID3D11RenderTargetView>,
    /// Signalled when the swap chain can accept another frame. `None` on a
    /// system without the waitable swap chain, which falls back to a latency
    /// limit on the device.
    waitable: Option<HANDLE>,
    /// The flags the swap chain was made with; resizing has to repeat them.
    flags: DXGI_SWAP_CHAIN_FLAG,
    renderer: egui_directx11::Renderer,
    swap: IDXGISwapChain,
    context: ID3D11DeviceContext,
    device: ID3D11Device,
}
impl Drop for Gpu {
    fn drop(&mut self) {
        if let Some(waitable) = self.waitable.take() {
            // SAFETY: the handle came from the swap chain and is closed once.
            unsafe {
                let _ = CloseHandle(waitable);
            }
        }
    }
}

impl Gpu {
    fn new(window: &Window) -> Result<Self> {
        let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
            anyhow::bail!("Expected a Windows HWND");
        };
        // SAFETY: COM handles remain owned for every call; output pointers refer
        // to initialized local Options. Rendering and resizing run on this thread.
        unsafe {
            let (mut device, mut context) = (None, None);
            let mut result = D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            );
            if result.is_err() {
                result = D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_WARP,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&[D3D_FEATURE_LEVEL_11_0]),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                );
            }
            result.context("Create D3D11 device")?;
            let device = device.context("Missing D3D11 device")?;
            let context = context.context("Missing D3D11 context")?;
            // Without a limit DXGI queues up to three frames ahead, so a click
            // or hover reached the screen 30-50 ms late, and by a varying
            // amount. One queued frame is what the wgpu path already asks for.
            if let Ok(device1) = device.cast::<IDXGIDevice1>() {
                let _ = device1.SetMaximumFrameLatency(1);
            }
            let dxgi: IDXGIDevice = device.cast()?;
            let factory: IDXGIFactory = dxgi.GetAdapter()?.GetParent()?;
            let size = window.inner_size();
            let hwnd = HWND(handle.hwnd.get() as _);
            let (width, height) = (size.width.max(1), size.height.max(1));
            let (swap, waitable, flags) =
                match Self::waitable_swap(&device, &factory, hwnd, width, height) {
                    Ok((swap, waitable)) => (
                        swap,
                        Some(waitable),
                        DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
                    ),
                    Err(_) => {
                        let desc = DXGI_SWAP_CHAIN_DESC {
                            BufferDesc: DXGI_MODE_DESC {
                                Width: width,
                                Height: height,
                                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                                ..Default::default()
                            },
                            SampleDesc: DXGI_SAMPLE_DESC {
                                Count: 1,
                                Quality: 0,
                            },
                            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                            BufferCount: 2,
                            OutputWindow: hwnd,
                            Windowed: true.into(),
                            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                            Flags: 0,
                        };
                        let mut swap = None;
                        factory.CreateSwapChain(&device, &desc, &mut swap).ok()?;
                        (
                            swap.context("Missing swap chain")?,
                            None,
                            DXGI_SWAP_CHAIN_FLAG(0),
                        )
                    }
                };
            factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
            let target = Some(Self::target(&device, &swap)?);
            let renderer = egui_directx11::Renderer::new(&device)?;
            Ok(Self {
                device,
                context,
                swap,
                target,
                renderer,
                waitable,
                flags,
            })
        }
    }

    /// A flip-model swap chain that signals when it can take the next frame,
    /// holding at most one frame in flight. Waiting on it before each frame
    /// keeps input-to-screen latency at the minimum vsync allows.
    unsafe fn waitable_swap(
        device: &ID3D11Device,
        factory: &IDXGIFactory,
        hwnd: HWND,
        width: u32,
        height: u32,
    ) -> Result<(IDXGISwapChain, HANDLE)> {
        unsafe {
            let factory: IDXGIFactory2 = factory.cast()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_UNSPECIFIED,
                Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
            };
            let swap = factory.CreateSwapChainForHwnd(device, hwnd, &desc, None, None)?;
            let swap2: IDXGISwapChain2 = swap.cast()?;
            swap2.SetMaximumFrameLatency(1)?;
            let waitable = swap2.GetFrameLatencyWaitableObject();
            anyhow::ensure!(!waitable.is_invalid(), "No frame latency waitable object");
            Ok((swap.cast()?, waitable))
        }
    }

    /// Blocks until the swap chain can take a frame, at most a few frames'
    /// time so a stalled display never freezes input handling.
    fn wait_for_frame(&self) {
        if let Some(waitable) = self.waitable {
            // SAFETY: the handle is owned by this Gpu and closed only on drop.
            unsafe {
                WaitForSingleObjectEx(waitable, 50, true);
            }
        }
    }
    fn target(device: &ID3D11Device, swap: &IDXGISwapChain) -> Result<ID3D11RenderTargetView> {
        unsafe {
            let buffer: ID3D11Texture2D = swap.GetBuffer(0)?;
            let mut target = None;
            device.CreateRenderTargetView(&buffer, None, Some(&mut target))?;
            target.context("Missing render target")
        }
    }
    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        self.target.take();
        unsafe {
            self.context.ClearState();
        }
        if width == 0 || height == 0 {
            return Ok(());
        }
        unsafe {
            self.swap
                .ResizeBuffers(2, width, height, DXGI_FORMAT_R8G8B8A8_UNORM, self.flags)?;
        }
        self.target = Some(Self::target(&self.device, &self.swap)?);
        Ok(())
    }
    fn paint(&mut self, ctx: &egui::Context, output: egui_directx11::RendererOutput) -> Result<()> {
        if let Some(target) = &self.target {
            unsafe {
                self.context
                    .ClearRenderTargetView(target, &[0.05, 0.07, 0.10, 1.0]);
            }
            self.renderer.render(&self.context, target, ctx, output)?;
        }
        Ok(())
    }
    fn capture(&self) -> Result<egui::ColorImage> {
        unsafe {
            let source: ID3D11Texture2D = self.swap.GetBuffer(0)?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            source.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            let mut staging = None;
            self.device
                .CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.context("Missing screenshot staging texture")?;
            self.context.CopyResource(&staging, &source);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let stride = desc.Width as usize * 4;
            let mut pixels = vec![0u8; stride * desc.Height as usize];
            for (row, destination) in pixels.chunks_exact_mut(stride).enumerate() {
                let source = std::slice::from_raw_parts(
                    (mapped.pData as *const u8).add(row * mapped.RowPitch as usize),
                    stride,
                );
                destination.copy_from_slice(source);
            }
            self.context.Unmap(&staging, 0);
            Ok(egui::ColorImage::from_rgba_unmultiplied(
                [desc.Width as usize, desc.Height as usize],
                &pixels,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Area = (0, 0, 1920, 1080);

    fn repair(outer: Area, inner: (i32, i32), monitors: &[Area]) -> Option<Area> {
        repaired_geometry(
            outer,
            inner,
            (400, 300),
            (1280, 800),
            monitors,
            monitors.first().copied(),
        )
    }

    #[test]
    fn a_normal_window_is_left_alone() {
        assert_eq!(repair((100, 100, 1280, 800), (1280, 800), &[SCREEN]), None);
    }

    /// The Remote Desktop sliver: full width, a few pixels high.
    #[test]
    fn a_sliver_gets_the_default_size_back_centred() {
        assert_eq!(
            repair((0, 0, 1920, 12), (1920, 4), &[SCREEN]),
            Some((320, 140, 1280, 800))
        );
    }

    #[test]
    fn a_window_off_every_screen_is_brought_back() {
        assert_eq!(
            repair((5000, 3000, 900, 600), (900, 600), &[SCREEN]),
            Some((510, 240, 900, 600))
        );
    }

    /// On a small screen the restored size still fits on it.
    #[test]
    fn the_restored_size_fits_a_small_monitor() {
        let small = (0, 0, 1024, 600);
        assert_eq!(
            repair((0, 0, 50, 50), (50, 50), &[small]),
            Some((51, 30, 921, 540))
        );
    }

    #[test]
    fn without_a_monitor_there_is_nothing_to_do() {
        assert_eq!(repair((0, 0, 10, 10), (10, 10), &[]), None);
    }
}
