#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod appicon;
mod editing;
mod icons;
#[cfg(windows)]
mod native_dx11;
mod remote_ui;
mod shaping;
mod theme;
mod toolbox;
mod view;

fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args_os().collect();
    let screenshot = args
        .windows(2)
        .find(|a| a[0] == "--screenshot")
        .map(|a| std::path::PathBuf::from(&a[1]));
    #[cfg(windows)]
    if std::env::var("GTERMINAL_RENDERER").as_deref() != Ok("wgpu") {
        return native_dx11::run(screenshot)
            .map_err(|error| eframe::Error::AppCreation(error.into()));
    }
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::default();
    let descriptor = setup.device_descriptor.clone();
    setup.device_descriptor = std::sync::Arc::new(move |adapter| {
        let mut device = descriptor(adapter);
        device.memory_hints = wgpu::MemoryHints::MemoryUsage;
        device
    });
    #[allow(unused_mut)]
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("G-Terminal")
        .with_icon(icons::default_app_icon(64))
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([400.0, 300.0]);
    // Windows (and Linux) draw their own window chrome on a frameless window.
    // macOS keeps the native window so it retains the traffic lights, rounded
    // corners, shadow and edge resizing — which winit does not expose for a
    // borderless window — but hides the titlebar and lets the content run
    // underneath, so it still reads as one continuous row.
    #[cfg(target_os = "macos")]
    {
        viewport = viewport
            .with_decorations(true)
            // `title_shown` is what hides the title text; `titlebar_shown` only
            // makes the bar transparent. Without the first, "G-Terminal" is
            // drawn over the app's own top bar.
            .with_title_shown(false)
            .with_titlebar_shown(false)
            .with_titlebar_buttons_shown(true)
            .with_fullsize_content_view(true);
    }
    #[cfg(not(target_os = "macos"))]
    {
        viewport = viewport.with_decorations(false);
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            desired_maximum_frame_latency: Some(1),
            wgpu_setup: setup.into(),
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "G-Terminal",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, screenshot)))),
    )
}
