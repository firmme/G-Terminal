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
        Foundation::{HMODULE, HWND},
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0},
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
    },
    core::Interface,
};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
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
                        .with_inner_size(LogicalSize::new(1280., 800.))
                        .with_min_inner_size(LogicalSize::new(400., 300.)),
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
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Err(error) = native.gpu.resize(size.width, size.height) {
                    self.fail(event_loop, error);
                }
            }
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

struct Native {
    // Drop GPU references before destroying the HWND.
    gpu: Gpu,
    app: crate::app::App,
    ctx: egui::Context,
    input: egui_winit::State,
    info: egui::ViewportInfo,
    window: Arc<Window>,
}
impl Native {
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
        })
    }
    fn draw(&mut self) -> Result<bool> {
        if self.window.is_minimized() == Some(true) || self.gpu.target.is_none() {
            return Ok(false);
        }
        egui_winit::update_viewport_info(&mut self.info, &self.ctx, &self.window, false);
        let mut raw = self.input.take_egui_input(&self.window);
        raw.viewports
            .insert(egui::ViewportId::ROOT, self.info.clone());
        let output = self.ctx.run(raw, |ctx| self.app.render(ctx));
        let (paint, platform, viewports) = egui_directx11::split_output(output);
        self.input.handle_platform_output(&self.window, platform);
        self.gpu.paint(&self.ctx, paint)?;
        let mut close = false;
        let mut actions = vec![];
        if let Some(viewport) = viewports.get(&egui::ViewportId::ROOT) {
            close = viewport
                .commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Close));
            egui_winit::process_viewport_commands(
                &self.ctx,
                &mut self.info,
                viewport.commands.clone(),
                &self.window,
                &mut actions,
            );
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
        Ok(close)
    }
}

struct Gpu {
    target: Option<ID3D11RenderTargetView>,
    renderer: egui_directx11::Renderer,
    swap: IDXGISwapChain,
    context: ID3D11DeviceContext,
    device: ID3D11Device,
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
            let dxgi: IDXGIDevice = device.cast()?;
            let factory: IDXGIFactory = dxgi.GetAdapter()?.GetParent()?;
            let size = window.inner_size();
            let hwnd = HWND(handle.hwnd.get() as _);
            let desc = DXGI_SWAP_CHAIN_DESC {
                BufferDesc: DXGI_MODE_DESC {
                    Width: size.width.max(1),
                    Height: size.height.max(1),
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
            factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
            let swap = swap.context("Missing swap chain")?;
            let target = Some(Self::target(&device, &swap)?);
            let renderer = egui_directx11::Renderer::new(&device)?;
            Ok(Self {
                device,
                context,
                swap,
                target,
                renderer,
            })
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
            self.swap.ResizeBuffers(
                2,
                width,
                height,
                DXGI_FORMAT_R8G8B8A8_UNORM,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
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
