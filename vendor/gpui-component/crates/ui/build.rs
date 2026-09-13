use std::{env, path::Path};

fn main() {
    // `gpui-component-assets` exposes the absolute path of its default
    // icons directory via cargo's `links` mechanism (see its `Cargo.toml`
    // and `build.rs`). We receive that path as
    // `DEP_GPUI_COMPONENT_DEFAULT_ICONS_ICONS_DIR` here, then re-publish
    // it as a `rustc-env` so it's visible to the
    // `icon_named!("$GPUI_COMPONENT_DEFAULT_ICONS_DIR")` proc-macro call
    // in `src/icon.rs` at expansion time. This is what lets the default
    // `IconName` enum be generated from the assets crate's icon set
    // without a sibling-crate reference, which would otherwise break
    // `cargo vendor` and `cargo publish`.
    //
    // Cargo only propagates `DEP_<name>_<key>` through *regular*
    // dependencies, not through build-deps — see the `dependencies`
    // (not `build-dependencies`) entry for `gpui-component-assets` in
    // `Cargo.toml`.
    let icons_dir = env::var("DEP_GPUI_COMPONENT_DEFAULT_ICONS_ICONS_DIR").expect(
        "DEP_GPUI_COMPONENT_DEFAULT_ICONS_ICONS_DIR is set by gpui-component-assets's \
         build.rs via its `links` field; make sure the regular dependency on \
         gpui-component-assets is intact in Cargo.toml",
    );

    // Fail here rather than letting `icon_named!` panic mid-expansion.
    // The assets crate validates this directory when it publishes the
    // path, so a miss at this point means we were handed a value that no
    // longer describes this machine: almost always a `target/` directory
    // carrying build script output recorded under a different checkout.
    if !Path::new(&icons_dir).is_dir() {
        panic!(
            "gpui-component-assets published an icons directory that does not exist:\n  \
             {icons_dir}\n\
             This is stale build script output, usually from a `target/` directory that was \
             produced under a different path. Run `cargo clean -p gpui-component-assets -p \
             gpui-component` and build again."
        );
    }

    println!("cargo:rustc-env=GPUI_COMPONENT_DEFAULT_ICONS_DIR={icons_dir}");

    // Rerun if the icons directory we point at changes. The assets crate's
    // build.rs already declares the same `rerun-if-changed`, but cargo
    // invalidates each build script independently, so this keeps us in
    // lockstep.
    println!("cargo:rerun-if-changed={icons_dir}");
    println!("cargo:rerun-if-changed=build.rs");
}
