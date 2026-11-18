//! Zwisper — composition root.
//!
//! This is the only place that names concrete adapters. `compose` picks the
//! strategy for each port; everything else talks to traits.
//!
//! Usage:
//!   zwisper                 normal app (macOS)
//!   zwisper --demo [--open library|meeting|settings|onboarding|pill]
//!                           scripted engines + fake audio, any OS

mod demo;
#[cfg(target_os = "macos")]
mod engines;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
mod tray;

use gpui::{App, Application};
use std::sync::Arc;

fn data_dir() -> std::path::PathBuf {
    directories::ProjectDirs::from("com", "Zwisper", "Zwisper")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("zwisper"))
}

/// `~/Library/Logs/Zwisper/zwisper.log` (Console.app shows it too). Truncated
/// once it passes 5 MB, so it never grows unbounded.
fn log_file() -> Option<std::fs::File> {
    let dir = directories::BaseDirs::new()?.home_dir().join("Library/Logs/Zwisper");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("zwisper.log");
    let too_big = std::fs::metadata(&path).map(|m| m.len() > 5 << 20).unwrap_or(false);
    std::fs::OpenOptions::new().create(true).append(!too_big).write(true).truncate(too_big).open(path).ok()
}

fn init_logging() {
    use tracing_subscriber::prelude::*;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    // A bundled app launched from Finder has no visible stderr, so also log to a file.
    let file = log_file().map(|f| tracing_subscriber::fmt::layer().with_ansi(false).with_writer(Arc::new(f)));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(file)
        .init();
}

fn main() {
    init_logging();

    let args: Vec<String> = std::env::args().collect();
    let demo = args.iter().any(|a| a == "--demo") || cfg!(not(target_os = "macos"));
    let open = args.iter().position(|a| a == "--open").and_then(|i| args.get(i + 1)).cloned();

    Application::new().run(move |cx: &mut App| {
        let (core, platform): (zwisper_core::Core, Arc<dyn zwisper_ui::UiPlatform>) = if demo {
            let root = data_dir().join("demo");
            let ports = demo::ports(&root);
            let seeded = demo::seed(ports.repo.as_ref());
            let core = zwisper_core::Core::start(ports).expect("core");
            if open.is_some() {
                // Skip onboarding when opening a specific screen.
                let mut s = core.settings();
                s.onboarded = open.as_deref() != Some("onboarding");
                core.send(zwisper_core::Command::UpdateSettings(s));
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let _ = seeded;
            (core, Arc::new(demo::DemoPlatform))
        } else {
            #[cfg(target_os = "macos")]
            {
                mac::compose(cx, &data_dir())
            }
            #[cfg(not(target_os = "macos"))]
            unreachable!()
        };

        let state = zwisper_ui::install(core.clone(), platform, cx);

        match open.as_deref() {
            Some("library") => zwisper_ui::open_library(cx),
            Some("settings") => zwisper_ui::open_settings(cx),
            Some("meeting") => {
                if let Some(s) = core.library().into_iter().find(|s| s.title == "Weekly product sync") {
                    state.update(cx, |st, cx| st.open_meeting(s.id, cx));
                }
            }
            Some("live") => zwisper_ui::start_meeting(cx),
            Some("pill") => zwisper_ui::toggle_dictation(cx),
            _ => {
                if demo && core.settings().onboarded {
                    zwisper_ui::open_library(cx);
                }
            }
        }
        cx.activate(true);
    });
}
