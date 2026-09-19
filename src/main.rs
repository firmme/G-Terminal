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
mod update;
mod view;

/// The PNG files an `.iconset` needs, and the pixel size of each. `iconutil`
/// only accepts these names, so they are fixed rather than derived.
const ICONSET: [(&str, u32); 10] = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
];

/// Writes the application mark as the PNG set an `.iconset` directory expects.
/// It is the same code-drawn [`appicon::pixels`] the window icon and the
/// Windows `.ico` use, so the mark has one definition.
fn export_iconset(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, size) in ICONSET {
        let rgba = appicon::pixels(size);
        image::save_buffer(
            dir.join(name),
            &rgba,
            size,
            size,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    }
    Ok(())
}

/// Runs one privileged action for the owner prompt's elevated copy, if the
/// command line asks for one. The outcome goes to `--report`, because a release
/// build has no console to print to and the caller is watching that file.
fn run_elevated_action(args: &[std::ffi::OsString]) -> Option<i32> {
    let value = |flag: &str| {
        args.windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| pair[1].clone())
    };
    let outcome = if let Some(pid) = value("--kill-pid")
        .and_then(|value| value.to_str().and_then(|text| text.parse::<u32>().ok()))
    {
        g_terminal::port_owner::kill(pid)
    } else {
        let port = value("--release-port")?;
        g_terminal::port_owner::release(&port.to_string_lossy())
    };
    let message = match outcome {
        Ok(message) => message,
        Err(error) => format!("失败：{error}"),
    };
    if let Some(path) = value("--report") {
        let _ = std::fs::write(path, message.as_bytes());
    }
    Some(0)
}

fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args_os().collect(); // The owner prompt relaunches the app with "runas" (Windows) or pkexec
    // (Linux) to end a process or restart a device this user may not touch.
    // The copy runs one action and exits, before any window exists.
    if let Some(code) = run_elevated_action(&args) {
        std::process::exit(code);
    }
    // Packaging hook: emit the mark for `scripts/package-macos.sh` to turn into
    // the bundle's `.icns`. It returns before any window is created.
    if let Some(dir) = args
        .windows(2)
        .find(|a| a[0] == "--export-iconset")
        .map(|a| a[1].clone())
    {
        if let Err(error) = export_iconset(std::path::Path::new(&dir)) {
            eprintln!("export-iconset: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The elevated-helper flags are the only thing that starts it; every other
    /// command line has to fall through to the app itself.
    #[test]
    fn ordinary_arguments_do_not_start_the_helper() {
        assert_eq!(run_elevated_action(&[]), None);
        let args: Vec<_> = ["--screenshot", "shot.png", "--export-iconset", "out"]
            .iter()
            .map(std::ffi::OsString::from)
            .collect();
        assert_eq!(run_elevated_action(&args), None);
    }

    /// An action always answers through the report file, even when the action
    /// itself failed: that file is the only channel back to the caller.
    #[test]
    fn an_action_that_cannot_run_still_reports() {
        let report = std::env::temp_dir().join("g-terminal-helper-test.txt");
        let _ = std::fs::remove_file(&report);
        // A pid that cannot exist, so nothing is really touched.
        let args: Vec<_> = [
            std::ffi::OsString::from("--kill-pid"),
            std::ffi::OsString::from("4294967280"),
            std::ffi::OsString::from("--report"),
            report.clone().into_os_string(),
        ]
        .into_iter()
        .collect();
        assert_eq!(run_elevated_action(&args), Some(0));
        let message = std::fs::read_to_string(&report).expect("the report must be written");
        let _ = std::fs::remove_file(&report);
        assert!(message.contains("失败"), "{message}");
    }
}
