// Hide the console window in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod bigpicture;
mod builtin;
mod chrome;
mod demo;
mod friends_window;
mod image_cache;
mod sized_images;
mod ingame;
mod launch_popup;
mod links;
mod login;
mod offline_ui;
mod overlay;
mod shell;
mod taskbar;
mod theme_renderer;
#[cfg(target_os = "linux")]
mod hyprland;
mod tray;
#[cfg(target_os = "linux")]
mod tray_linux;
mod updater;
#[cfg(target_os = "linux")]
mod wayland_activate;

fn main() -> eframe::Result {
    if std::env::args().any(|arg| arg == "--adwaita-helper") {
        if let Err(error) = themes::adwaita::run_adwaita_helper() {
            eprintln!("Native Adwaita helper: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    tune_malloc();
    // A `steam://` link (or a plain second launch) goes to the running copy.
    let link = std::env::args().skip(1).find(|a| a.starts_with("steam://"));
    // `--separate` runs a second copy (for previews and testing).
    let separate = std::env::args().any(|a| a == "--separate");
    #[cfg(target_os = "linux")]
    let activation_token = std::env::var("XDG_ACTIVATION_TOKEN").ok();
    #[cfg(not(target_os = "linux"))]
    let activation_token: Option<String> = None;
    let request = link.as_deref().or_else(|| links::big_picture_request(std::env::args().skip(1))).unwrap_or("show");
    if !separate && links::forward_to_running(request, activation_token.as_deref()) {
        return Ok(());
    }
    // An update downloaded last time installs now, before the app opens.
    if !separate && updater::apply_pending_on_start() {
        return Ok(());
    }

    init_logging();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(i18n::BRAND)
            // Starts at sign-in size; the app resizes itself after sign-in.
            .with_inner_size([560.0, 400.0])
            // Our own title bar (see chrome.rs) instead of the Windows one.
            .with_decorations(false)
            .with_transparent(true)
            .with_icon(std::sync::Arc::new(ui::brand::window_icon(256))),
        // The app keeps the main window's geometry itself (app.rs): eframe's
        // copy would reopen the small sign-in window at the main size.
        persist_window: false,
        ..Default::default()
    };
    // "SteamInRust" names the settings folder (and Wayland app id): kept so
    // existing settings carry over to Aurora.
    ui::native::run("SteamInRust", options, Box::new(move |cc| Ok(Box::new(app::SteamApp::new(cc, link)))))
}

/// glibc's malloc raises its mmap threshold (up to 32 MiB) each time a big
/// buffer is freed. After a few decoded pictures (several MB each), they all
/// come from the heap, and the freed ones stay in the process, scattered:
/// opening games grew memory by ~100 MB this way. A fixed threshold keeps
/// big buffers in their own mappings, given back to the system when freed.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn tune_malloc() {
    // SAFETY: plain allocator settings, set before any other thread starts.
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 128 * 1024);
        libc::mallopt(libc::M_TRIM_THRESHOLD, 128 * 1024);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn tune_malloc() {}

/// Give freed heap memory back to the system (the window just went to the tray).
pub fn release_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim only returns free pages; it's thread-safe.
    unsafe {
        libc::malloc_trim(0);
    }
}


/// Open `aurora.log` safely (0700 dir, 0600 file, never following symlinks).
fn open_log_file(dir: &std::path::Path) -> Option<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true).mode(0o700);
        b.create(dir).ok()?;
        let dm = std::fs::symlink_metadata(dir).ok()?;
        if dm.file_type().is_symlink() || !dm.is_dir() {
            return None;
        }
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        let path = dir.join("aurora.log");
        let old = dir.join("aurora.old.log");
        if std::fs::symlink_metadata(&old).is_ok_and(|m| m.file_type().is_symlink()) {
            return None;
        }
        if let Ok(m) = std::fs::symlink_metadata(&path) {
            if m.file_type().is_symlink() {
                return None;
            }
            let _ = std::fs::rename(&path, &old);
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true).mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let f = opts.open(&path).ok()?;
        let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
        Some(f)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir).ok()?;
        let path = dir.join("aurora.log");
        let old = dir.join("aurora.old.log");
        if std::fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_symlink()) {
            let _ = std::fs::rename(&path, &old);
        }
        std::fs::File::create(path).ok()
    }
}

/// Log to the terminal and to `aurora.log` in the settings folder (the
/// last run's log is kept as `aurora.old.log`). On Windows the app has no
/// console, so the file is the only log a player can send.
fn init_logging() {
    use tracing_subscriber::prelude::*;
    let filter = || tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,eframe=warn,egui_glow=warn".into());
    let dir = client::store::config_dir();
    let file = open_log_file(&dir);
    let to_file = file.map(|f| tracing_subscriber::fmt::layer().with_ansi(false).with_writer(std::sync::Mutex::new(f)).with_filter(filter()));
    tracing_subscriber::registry().with(tracing_subscriber::fmt::layer().with_filter(filter())).with(to_file).init();
}

#[cfg(all(test, unix))]
mod log_tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn open_log_file_sets_0600_and_refuses_symlinks() {
        let base = std::env::temp_dir().join(format!("aurora-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("cfg");
        let f = super::open_log_file(&dir).expect("creates aurora.log");
        drop(f);
        let dmode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        let fmode = std::fs::metadata(dir.join("aurora.log")).unwrap().permissions().mode() & 0o777;
        assert_eq!(dmode, 0o700);
        assert_eq!(fmode, 0o600);

        // Refuses symlink at aurora.log.
        std::fs::remove_file(dir.join("aurora.log")).unwrap();
        let outside = base.join("outside.txt");
        std::fs::write(&outside, b"keep").unwrap();
        symlink(&outside, dir.join("aurora.log")).unwrap();
        assert!(super::open_log_file(&dir).is_none());
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
        let _ = std::fs::remove_dir_all(&base);
    }
}
