// Exists only so the `[build-dependencies]` entry in Cargo.toml takes part in feature
// unification (see the comment there).
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}
