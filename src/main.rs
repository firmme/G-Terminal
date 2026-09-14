#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
#[cfg(windows)]
mod native_dx11;
mod remote_ui;
mod shaping;
mod theme;
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
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("G-Terminal")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([760.0, 480.0]),
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
