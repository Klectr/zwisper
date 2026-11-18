//! Enforces the dependency rule from docs/ARCHITECTURE.md §3.1.

use std::path::Path;

fn deps(manifest: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(manifest).unwrap();
    let mut out = Vec::new();
    let mut in_deps = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_deps = l == "[dependencies]" || l.starts_with("[target.") && l.ends_with(".dependencies]");
            continue;
        }
        if in_deps {
            if let Some((name, _)) = l.split_once('=') {
                let name = name.trim().split(".").next().unwrap().trim();
                if !name.is_empty() && !name.starts_with('#') {
                    out.push(name.to_string());
                }
            }
        }
    }
    out
}

#[test]
fn dependency_rule() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // 1. core depends on no engine, UI or OS crate.
    let core_allowed = ["serde", "thiserror", "crossbeam-channel", "arc-swap", "parking_lot", "chrono", "ulid", "strsim", "tracing"];
    for d in deps(&root.join("crates/core/Cargo.toml")) {
        assert!(core_allowed.contains(&d.as_str()), "core must not depend on `{d}`");
    }
    // 2. adapters depend on core, never on each other or on the UI.
    let adapters = root.join("crates/adapters");
    for entry in std::fs::read_dir(&adapters).unwrap() {
        let m = entry.unwrap().path().join("Cargo.toml");
        if !m.exists() {
            continue;
        }
        for d in deps(&m) {
            assert!(
                !(d.starts_with("zwisper-") && d != "zwisper-core") && d != "gpui" && d != "gpui-component",
                "{} must not depend on `{d}`",
                m.display()
            );
        }
    }
    // 3. ui depends only on core among workspace crates.
    let ui = root.join("crates/ui/Cargo.toml");
    if ui.exists() {
        for d in deps(&ui) {
            assert!(!(d.starts_with("zwisper-") && d != "zwisper-core"), "ui must not depend on `{d}`");
        }
    }
}
