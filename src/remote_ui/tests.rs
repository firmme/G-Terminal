use super::*;

#[test]
fn timestamps_shorten_themselves_like_ls() {
    // `civil_parts` is the shared core, so it carries the calendar checks.
    assert_eq!(civil_parts(0), (1970, 1, 1, 0, 0));
    assert_eq!(civil_parts(951_782_400), (2000, 2, 29, 0, 0));
    assert_eq!(civil_parts(1_709_164_800), (2024, 2, 29, 0, 0));
    assert_eq!(civil_parts(1_709_251_200), (2024, 3, 1, 0, 0));
    assert_eq!(civil_parts(1_709_214_300), (2024, 2, 29, 13, 45));
    // Past 2038, which the 32-bit SFTP mtime field cannot itself express.
    assert_eq!(civil_parts(4_102_444_800), (2100, 1, 1, 0, 0));

    // The full form is what the tooltip shows, so it never varies.
    assert_eq!(format_time_full(0), "1970-01-01 00:00");
    assert_eq!(format_time_full(1_709_214_300), "2024-02-29 13:45");

    // The short form drops the clock for older files and the year for this
    // year's, so it is always shorter than the full one.
    let this_year = current_year();
    for seconds in [0, 951_782_400, 1_709_214_300, 4_102_444_800] {
        let (year, ..) = civil_parts(seconds);
        let short = format_time(seconds);
        let limit = if year == this_year { 11 } else { 10 };
        assert!(
            short.len() <= limit,
            "{short:?} is wider than the column is sized for"
        );
        assert!(short.len() < format_time_full(seconds).len());
    }
}

#[test]
fn parent_path_never_leaves_a_dotdot_component() {
    assert_eq!(parent_path("/root/etc"), "/root");
    assert_eq!(parent_path("/root"), "/");
    assert_eq!(parent_path("/"), "/");
    assert_eq!(parent_path("/root/"), "/");
    assert_eq!(parent_path(""), "/");
}

#[test]
fn join_path_does_not_double_the_root_slash() {
    assert_eq!(join_path("/root", "etc"), "/root/etc");
    assert_eq!(join_path("/", "etc"), "/etc");
    assert_eq!(join_path("/root/", "etc"), "/root/etc");
}

#[test]
fn file_icons_follow_the_name() {
    use crate::icons::Icon;
    // What a thing *is* beats what it is called.
    assert_eq!(file_icon("src", true, false), Icon::Folder);
    assert_eq!(file_icon("archive.zip", true, false), Icon::Folder);
    assert_eq!(file_icon("run.sh", false, true), Icon::FileBinary);

    assert_eq!(file_icon("main.rs", false, false), Icon::FileCode);
    assert_eq!(file_icon("Makefile", false, false), Icon::File);
    assert_eq!(file_icon("config.toml", false, false), Icon::FileCode);
    assert_eq!(file_icon("notes.md", false, false), Icon::FileText);
    assert_eq!(file_icon("photo.PNG", false, false), Icon::FileImage);
    assert_eq!(file_icon("backup.tar.gz", false, false), Icon::FileArchive);
    assert_eq!(file_icon("clip.mp4", false, false), Icon::FileMedia);
    assert_eq!(file_icon("libfoo.so", false, false), Icon::FileBinary);
    // Unknown, and no extension at all, both fall back to a plain page.
    assert_eq!(file_icon("mystery.qqq", false, false), Icon::File);
    assert_eq!(file_icon("README", false, false), Icon::File);
}

/// A leading dot belongs to the name. `.bashrc` is a dotfile, not a file of
/// type "bashrc" — treating it as one would put it in a category nothing else
/// shares.
#[test]
fn a_dotfile_has_no_extension() {
    assert_eq!(file_extension(".bashrc"), "");
    assert_eq!(file_extension(".gitignore"), "");
    assert_eq!(file_extension("."), "");
    assert_eq!(file_extension(".."), "");
    assert_eq!(file_extension(""), "");
    assert_eq!(file_extension("archive.tar.gz"), "gz");
    assert_eq!(file_extension("main.RS"), "rs");
    // A dot that is part of the name, not before an extension.
    assert_eq!(file_extension("my file.txt"), "txt");
}

#[test]
fn entry_suffix_follows_the_ls_convention() {
    assert_eq!(entry_suffix(true, false), "/");
    assert_eq!(entry_suffix(false, true), "@");
    assert_eq!(entry_suffix(false, false), "");
    assert_eq!(entry_suffix(true, true), "/");
}

#[test]
fn columns_are_ordered_and_span_the_row() {
    let offsets = column_offsets(&REMOTE_COLUMNS, 600.0);
    assert_eq!(offsets.len(), 5, "four columns have five edges");
    assert_eq!(offsets[0], 0.0);
    for pair in offsets.windows(2) {
        assert!(pair[1] > pair[0], "edges must increase: {offsets:?}");
    }
    assert!((offsets[4] - 600.0).abs() < 0.01, "columns span the row");
}

#[test]
fn local_columns_follow_the_platform_permissions() {
    let edges = column_offsets(&LOCAL_COLUMNS, 600.0).len();
    #[cfg(unix)]
    assert_eq!(edges, 5, "a POSIX local table has a permissions column");
    #[cfg(not(unix))]
    assert_eq!(edges, 4, "Windows has no permissions column");
}

/// Adjacent columns have to be separated by more than their cells' own
/// padding, or a value one character wider than expected ends up touching its
/// neighbour with nothing between them.
#[test]
fn columns_are_separated_by_a_gap() {
    for width in [320.0, 480.0, 700.0] {
        let offsets = column_offsets(&REMOTE_COLUMNS, width);
        let row = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 18.0));
        let rects: Vec<_> = (0..offsets.len() - 1)
            .filter_map(|index| column_rect(row, &offsets, index))
            .collect();
        for pair in rects.windows(2) {
            let gap = pair[1].left() - pair[0].right();
            assert!(
                gap >= COLUMN_GAP - 0.01,
                "columns sit {gap} apart at width {width}"
            );
        }
        // The gap is given back by every column but the last, so the row still
        // ends exactly at its right edge.
        assert!((rects.last().unwrap().right() - row.right()).abs() < 0.01);
    }
}

/// A pane's rows have to stay inside that pane. They are laid out in a child
/// `Ui` nested inside a `ScrollArea`, and if either reported the window's width
/// rather than the pane's, the remote table's first column would paint across
/// into the local table on its left.
#[test]
fn a_panes_rows_stay_inside_the_pane() {
    const PANE_LEFT: f32 = 460.0;
    const PANE_RIGHT: f32 = 900.0;
    let ctx = egui::Context::default();
    let cells = [
        Cell {
            text: "readme.txt".into(),
            right: false,
            color: egui::Color32::WHITE,
            full: None,
            icon: None,
            link: false,
        },
        Cell {
            text: "1.0 KiB".into(),
            right: true,
            color: egui::Color32::GRAY,
            full: None,
            icon: None,
            link: false,
        },
        Cell {
            text: "-rw-r--r--".into(),
            right: true,
            color: egui::Color32::GRAY,
            full: None,
            icon: None,
            link: false,
        },
        Cell {
            text: "03-14 20:32".into(),
            right: true,
            color: egui::Color32::GRAY,
            full: None,
            icon: None,
            link: false,
        },
    ];
    let mut measured: Option<egui::Rect> = None;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 600.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                // Exactly the shape the file window builds for its panes.
                let pane = egui::Rect::from_min_max(
                    egui::pos2(PANE_LEFT, 0.0),
                    egui::pos2(PANE_RIGHT, 400.0),
                );
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(pane)
                        .layout(egui::Layout::top_down(egui::Align::LEFT)),
                );
                egui::ScrollArea::vertical()
                    .id_salt("test-pane")
                    .auto_shrink([false, false])
                    .max_height(300.0)
                    .show(&mut child, |ui| {
                        measured =
                            Some(table_row(ui, &cells, &REMOTE_COLUMNS, false, 440.0).0.rect);
                    });
            });
        },
    );
    let row = measured.expect("a row was laid out");
    assert!(
        row.left() >= PANE_LEFT - 0.5,
        "the row started left of its pane: {row:?}"
    );
    assert!(
        row.right() <= PANE_RIGHT + 0.5,
        "the row ran past its pane: {row:?}"
    );
}

/// Drives a real egui frame, so the row geometry is checked against the layout
/// engine rather than against a reimplementation of it.
#[test]
fn table_rows_lay_cells_out_left_to_right_without_overlap() {
    let ctx = egui::Context::default();
    let cells = [
        Cell {
            text: "readme.txt".into(),
            right: false,
            color: egui::Color32::WHITE,
            full: None,
            icon: None,
            link: false,
        },
        Cell {
            text: "1.0 KiB".into(),
            right: true,
            color: egui::Color32::GRAY,
            full: None,
            icon: None,
            link: false,
        },
        Cell {
            text: "-rw-r--r--".into(),
            right: true,
            color: egui::Color32::GRAY,
            full: None,
            icon: None,
            link: false,
        },
        Cell {
            text: "2024-02-29 13:45".into(),
            right: true,
            color: egui::Color32::GRAY,
            full: None,
            icon: None,
            link: false,
        },
    ];
    let mut measured: Option<(egui::Rect, Vec<egui::Rect>)> = None;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 400.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (response, rects) = table_row(ui, &cells, &REMOTE_COLUMNS, false, 440.0);
                measured = Some((response.rect, rects));
            });
        },
    );
    let (row, rects) = measured.expect("the frame rendered a row");
    assert_eq!(rects.len(), cells.len());
    for pair in rects.windows(2) {
        assert!(
            pair[0].right() <= pair[1].left() + 0.01,
            "columns overlap: {:?} then {:?}",
            pair[0],
            pair[1]
        );
    }
    // The rightmost metadata column ends exactly at the row's right edge.
    assert!((rects[3].right() - row.right()).abs() < 0.01);
}

/// A name wider than its column used to paint straight across the columns to
/// its right, because the painter is only clipped to the scroll area.
#[test]
fn truncated_cells_never_exceed_their_column() {
    let ctx = egui::Context::default();
    let font = egui::FontId::proportional(14.0);
    let color = egui::Color32::WHITE;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 400.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let measure = |text: &str| {
                    ui.painter()
                        .layout_no_wrap(text.to_owned(), font.clone(), color)
                        .rect
                        .width()
                };
                for (text, budget) in [
                    ("short.txt", 300.0),
                    ("a-really-long-file-name-that-cannot-fit.bin", 120.0),
                    ("超长的中文文件名需要被安全截断", 60.0),
                    ("", 100.0),
                    ("no-room.bin", 0.0),
                ] {
                    let (out, truncated) = truncate_to_width(ui, text, &font, color, budget);
                    let width = measure(&out);
                    assert!(
                        width <= budget.max(0.0) + 0.5,
                        "{text:?} in {budget} produced {out:?}, {width} wide"
                    );
                    if truncated {
                        // A cut string is either marked, or had no room at all.
                        assert!(
                            out.is_empty() || out.ends_with('…'),
                            "{out:?} should be marked as cut"
                        );
                    } else {
                        assert_eq!(out, text, "text that fits must come back intact");
                    }
                }
                let (out, truncated) = truncate_to_width(ui, "fits.bin", &font, color, 300.0);
                assert_eq!(out, "fits.bin");
                assert!(!truncated);
            });
        },
    );
}

/// The editor lookup is the one part of 编辑 that can be pinned down without
/// launching anything or depending on this machine's environment.
#[test]
fn editor_lookup_prefers_visual_over_editor() {
    fn env<'a>(pairs: &'a [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        }
    }
    assert_eq!(
        editor_program_from(env(&[("EDITOR", "nano"), ("VISUAL", "code")])),
        Some("code".to_string())
    );
    assert_eq!(
        editor_program_from(env(&[("EDITOR", "nano")])),
        Some("nano".to_string())
    );
    assert_eq!(
        editor_program_from(env(&[("VISUAL", " vim ")])),
        Some("vim".to_string())
    );
    // A blank variable must not become an empty program name.
    assert_eq!(editor_program_from(env(&[("EDITOR", "   ")])), None);
    assert_eq!(editor_program_from(env(&[])), None);
}

/// The save watcher keys off size and mtime, so a rewrite that changes only
/// one of them still registers.
#[test]
fn save_stamp_notices_a_rewrite() {
    let dir = scratch_dir(usize::MAX).expect("scratch directory");
    let file = dir.join("stamp.txt");
    std::fs::write(&file, b"one").unwrap();
    let first = save_stamp(&file);
    assert!(first.is_some());
    std::fs::write(&file, b"longer than before").unwrap();
    assert_ne!(
        save_stamp(&file),
        first,
        "a growing file must change the stamp"
    );
    assert_eq!(save_stamp(&dir.join("absent.txt")), None);
    std::fs::remove_dir_all(dir).unwrap();
}

/// The queue keeps the newest settled entries and drops the rest, oldest
/// first — removing them in that order is what leaves later indices valid.
#[test]
fn pruning_keeps_the_newest_settled_entries() {
    assert!(
        settled_to_drop(&[1, 2, 3], 5).is_empty(),
        "nothing should be dropped under the cap"
    );
    assert_eq!(settled_to_drop(&[1, 2, 3], 3), [] as [usize; 0]);
    // Oldest first, so the newest survive.
    assert_eq!(settled_to_drop(&[2, 5, 7, 9], 2), [2, 5]);
    assert_eq!(settled_to_drop(&[0, 1, 2, 3, 4], 1), [0, 1, 2, 3]);
}

/// Local POSIX modes are read from the listing, and a chmod round-trips
/// through the same routine the 属性 editor uses.
#[test]
#[cfg(unix)]
fn local_permissions_are_read_and_applied() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch_dir(usize::MAX - 2).expect("scratch directory");
    let outside = scratch_dir(usize::MAX - 3).expect("scratch directory");
    let file = dir.join("mode.txt");
    std::fs::write(&file, b"x").unwrap();
    let entries = read_local(&dir.display().to_string()).unwrap();
    let entry = entries.iter().find(|e| e.name == "mode.txt").unwrap();
    assert!(entry.perms.is_some(), "a POSIX host has to report a mode");

    let progress = Arc::new(Mutex::new(ztransfer::Progress::default()));
    let wake: remote::Wake = Arc::new(|| {});
    apply_local_mode(&file, 0o600, false, &progress, &wake).unwrap();
    assert_eq!(mode_of(&file), 0o600, "a single-file chmod has to land");

    // A recursive change reaches the tree but must not follow a symlink out
    // of it: the link's target lives elsewhere and stays untouched.
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let nested = sub.join("nested.txt");
    std::fs::write(&nested, b"y").unwrap();
    let target = outside.join("target.txt");
    std::fs::write(&target, b"z").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::os::unix::fs::symlink(&target, sub.join("link.txt")).unwrap();

    apply_local_mode(&dir, 0o750, true, &progress, &wake).unwrap();
    assert_eq!(
        mode_of(&nested),
        0o750,
        "a recursive chmod has to reach nested files"
    );
    assert_eq!(
        mode_of(&target),
        0o644,
        "a recursive chmod followed a symlink"
    );

    // A mode that drops the execute bit has to still succeed: the walk
    // changes the contents before the directory that holds them.
    apply_local_mode(&dir, 0o600, true, &progress, &wake).unwrap();
    assert_eq!(mode_of(&dir), 0o600);
    // Put traversal back, then clear the tree.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    apply_local_mode(&dir, 0o700, true, &progress, &wake).unwrap();

    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(outside).unwrap();
}

#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777
}

/// The local listing now runs on a worker thread, so its own rules are worth
/// pinning: directories first, then a case-insensitive sort by name.
#[test]
fn local_listing_sorts_directories_first() {
    let dir = scratch_dir(usize::MAX - 1).expect("scratch directory");
    std::fs::create_dir_all(dir.join("zdir")).unwrap();
    std::fs::write(dir.join("b.txt"), b"bb").unwrap();
    std::fs::write(dir.join("A.txt"), b"a").unwrap();

    let rows = read_local(&dir.display().to_string()).unwrap();
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(
        names,
        ["zdir", "A.txt", "b.txt"],
        "directories first, then names case-insensitively"
    );
    let b = rows.iter().find(|row| row.name == "b.txt").unwrap();
    assert_eq!(b.size, 2, "the size has to come through");
    assert!(b.mtime.is_some(), "the timestamp has to come through");
    // A missing directory is an error to show, not a panic.
    assert!(read_local(&dir.join("absent").display().to_string()).is_err());

    std::fs::remove_dir_all(dir).unwrap();
}

/// The address bar is laid out right to left so the field takes what is left.
/// The icons come after the field in that order, so their room has to be held
/// back — without it the field eats the lot and the icons land at negative x,
/// painting over the pane to the left of this one.
#[test]
fn the_address_bar_stays_inside_its_pane() {
    const PANE_LEFT: f32 = 300.0;
    const PANE_RIGHT: f32 = 470.0;
    let palette = Palette::new(false);
    let ctx = egui::Context::default();
    let mut bar: Option<egui::Rect> = None;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 300.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let pane = egui::Rect::from_min_max(
                    egui::pos2(PANE_LEFT, 0.0),
                    egui::pos2(PANE_RIGHT, 200.0),
                );
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(pane)
                        .layout(egui::Layout::top_down(egui::Align::LEFT)),
                );
                let mut path = String::from("C:/Users/firmm");
                child.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        crate::icons::icon_label_button(
                            ui,
                            crate::icons::Icon::Refresh,
                            "刷新",
                            palette,
                        );
                        let icons = crate::icons::Size::Button.button().x * 2.0
                            + ui.spacing().item_spacing.x * 2.0;
                        let room = ui.available_width();
                        // Same rule as the panes: the shortcuts go before the
                        // field is allowed to overflow.
                        let shortcuts = room - icons >= MIN_FIELD_WIDTH;
                        editing::field_with(ui, &mut path, |edit| {
                            edit.desired_width(if shortcuts { room - icons } else { room })
                        });
                        if shortcuts {
                            crate::icons::icon_button(
                                ui,
                                crate::icons::Icon::Home,
                                palette,
                                crate::icons::Size::Button,
                            );
                            crate::icons::icon_button(
                                ui,
                                crate::icons::Icon::FolderUp,
                                palette,
                                crate::icons::Size::Button,
                            );
                        }
                        bar = Some(ui.min_rect());
                    });
                });
            });
        },
    );
    let bar = bar.expect("the bar was laid out");
    assert!(
        bar.left() >= PANE_LEFT - 0.5,
        "the bar started left of its pane: {bar:?}"
    );
    assert!(
        bar.right() <= PANE_RIGHT + 0.5,
        "the bar ran past its pane: {bar:?}"
    );
}

/// When the pane is narrower than the table needs, the scroll area has to
/// report content wider than its viewport — that is the condition the
/// horizontal bar appears under. If the content fits, no bar is drawn and the
/// columns have nowhere to go but a truncation.
#[test]
fn a_narrow_pane_makes_the_table_overflow_sideways() {
    const VIEWPORT: f32 = 300.0;
    let ctx = egui::Context::default();
    let mut measured: Option<(f32, f32)> = None;
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(VIEWPORT, 300.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let area = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(VIEWPORT, 200.0));
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(area)
                        .layout(egui::Layout::top_down(egui::Align::LEFT)),
                );
                let cells = [
                    Cell {
                        text: "readme.txt".into(),
                        right: false,
                        color: egui::Color32::WHITE,
                        full: None,
                        icon: None,
                        link: false,
                    },
                    Cell {
                        text: "1.0 KiB".into(),
                        right: true,
                        color: egui::Color32::GRAY,
                        full: None,
                        icon: None,
                        link: false,
                    },
                    Cell {
                        text: "-rw-r--r--".into(),
                        right: true,
                        color: egui::Color32::GRAY,
                        full: None,
                        icon: None,
                        link: false,
                    },
                    Cell {
                        text: "03-14 20:32".into(),
                        right: true,
                        color: egui::Color32::GRAY,
                        full: None,
                        icon: None,
                        link: false,
                    },
                ];
                let output = egui::ScrollArea::both()
                    .id_salt("narrow-probe")
                    .auto_shrink([false, false])
                    .max_height(150.0)
                    .show(&mut child, |ui| {
                        let width = ui.available_width().max(MIN_TABLE_WIDTH);
                        table_row(ui, &cells, &REMOTE_COLUMNS, false, width);
                    });
                measured = Some((output.content_size.x, output.inner_rect.width()));
            });
        },
    );
    let (content, viewport) = measured.expect("the table was laid out");
    assert!(
        content > viewport,
        "content is {content} wide in a {viewport} viewport, so no bar would appear"
    );
}
