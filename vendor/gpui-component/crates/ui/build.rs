use std::{env, path::Path};

fn main() {
    // The default icons live in the sibling `gpui-component-assets` crate,
    // whose checkout location is fixed relative to this crate: both sit in
    // `crates/` of the same repository and are consumed together as path
    // dependencies. This build script derives the absolute icons directory
    // from our own `CARGO_MANIFEST_DIR`, so no `links`-style metadata
    // handshake between the two build scripts is needed.
    //
    // That handshake was removed deliberately: it required the `links` field
    // in the assets crate's manifest, and cargo rejects `links`-declaring
    // packages that lack a build script, which is exactly the shape
    // Dependabot's cargo updater produces when it copies manifests without
    // build scripts. The sibling-relative computation is equivalent for
    // every consumer in this repository.
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set by cargo");
    let icons_dir = Path::new(&manifest_dir)
        .join("../assets")
        .join("assets/icons");

    // Fail here rather than letting `icon_named!` panic mid-expansion.
    if !icons_dir.is_dir() {
        panic!(
            "expected default icons at {}, but the directory is missing",
            icons_dir.display(),
        );
    }

    println!("cargo:rustc-env=GPUI_COMPONENT_DEFAULT_ICONS_DIR={}", icons_dir.display());

    // Rerun if the icon set changes (rename/add/remove).
    //
    // This MUST name the absolute path rather than the relative
    // `../assets/assets/icons`. Cargo records these instructions in this
    // build script's fingerprint, so an absolute path pins our location:
    // relocating the checkout, or reusing a `target/` directory produced
    // under a different path, makes the recorded instructions differ,
    // cargo reports "the rerun-if-changed instructions changed", and we
    // rerun and republish a correct `icons-dir` above.
    //
    // The relative form is location-independent, so cargo instead replays
    // this script's cached output. The stale absolute `icons-dir` that
    // output carries then either breaks the dependent's `icon_named!`
    // expansion with a "failed to read" panic (old path gone) or, worse,
    // silently builds against the other checkout's icons (old path still
    // present).
    println!("cargo:rerun-if-changed={}", icons_dir.display());

    // Also rerun if anyone fiddles with this script itself.
    println!("cargo:rerun-if-changed=build.rs");
}
