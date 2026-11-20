//! Kombini Mod Manager desktop app (GPUI).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod age;
mod anim;
mod app;
mod audio;
mod collections;
mod crosshair;
mod customize;
mod discover;
mod downloads;
mod gallery;
mod heroes;
mod icons;
mod instance;
mod issues;
mod launch;
mod modal;
mod mods;
mod ops;
mod page_files;
mod selection;
mod settings;
mod smooth_scroll;
mod sounds;
mod theme;
mod versions;
mod widgets;

use gpui_kit::*;

/// Logs to `<data>/logs/app.log` (previous run kept as `app.old.log`) and
/// records panics there, so a crash report can be attached to a bug.
fn init_logging(dirs: &kombini_engine::AppDirs) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let logs = dirs.logs();
    let file = std::fs::create_dir_all(&logs).ok().and_then(|()| {
        let path = logs.join("app.log");
        let _ = std::fs::rename(&path, logs.join("app.old.log"));
        std::fs::File::create(path).ok()
    });
    match file {
        Some(file) => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .try_init();
        }
        None => {
            let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
        }
    }
    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(panic = %info, %backtrace, "panic");
    }));
}

/// Process start, for the startup timings logged by the app.
pub static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn main() {
    STARTED.get_or_init(std::time::Instant::now);
    let dirs = kombini_engine::AppDirs::from_env().unwrap_or_else(|_| kombini_engine::AppDirs {
        data: std::env::temp_dir().join("kombini-data"),
        cache: std::env::temp_dir().join("kombini-cache"),
    });
    init_logging(&dirs);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let inbox = match instance::start(&dirs.data, args) {
        instance::Start::Primary(inbox) => inbox,
        instance::Start::Forwarded => return,
    };
    gpui_kit::application()
        .with_assets(icons::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::apply(cx);
            let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(960.), px(600.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Kombini Mod Manager".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            if let Err(e) = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| app::AppView::new(inbox.clone(), window, cx))
            }) {
                tracing::error!(error = %e, "could not open the main window");
            }
        });
}
