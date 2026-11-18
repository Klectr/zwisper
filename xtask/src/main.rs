//! `cargo app` — build Zwisper.app + Zwisper.dmg and open the drag-to-install
//! window. A thin wrapper so the bundle script is reachable from cargo; cargo
//! won't let an alias replace `cargo build` itself.
//!
//! Extra arguments and env vars (`SIGN_ID`, `NO_OPEN`) pass through to
//! `scripts/bundle-macos.sh`.

use std::path::Path;
use std::process::{exit, Command};

fn main() {
    if !cfg!(target_os = "macos") {
        eprintln!("cargo app: macOS only");
        exit(1);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("workspace root");
    let status = Command::new("bash")
        .arg(root.join("scripts/bundle-macos.sh"))
        .args(std::env::args_os().skip(1))
        .current_dir(root)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("cargo app: could not run bundle-macos.sh: {e}");
            exit(1);
        });
    exit(status.code().unwrap_or(1));
}
