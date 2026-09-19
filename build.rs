use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    select_murglar_backend();

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        compile_windows_resources();
    }
}

fn compile_windows_resources() {
    println!("cargo:rerun-if-changed=windows.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

    let version = env::var("CARGO_PKG_VERSION").expect("Cargo must provide the package version");
    let version_numbers = ["MAJOR", "MINOR", "PATCH"]
        .map(|part| {
            let name = format!("CARGO_PKG_VERSION_{part}");
            env::var(&name)
                .unwrap_or_else(|_| panic!("Cargo must provide {name}"))
                .parse::<u16>()
                .unwrap_or_else(|_| panic!("{name} must fit a Windows version component"))
        })
        .map(|part| part.to_string())
        .join(",");

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must provide CARGO_MANIFEST_DIR"),
    );
    let icon_path = manifest_dir.join("assets/icon.ico");
    let template = fs::read_to_string(manifest_dir.join("windows.rc"))
        .expect("Windows resource template must be readable");
    let resources = template
        .replace(
            "@ICON_PATH@",
            &icon_path.to_string_lossy().replace('\\', "/"),
        )
        .replace("@VERSION_NUMBERS@", &format!("{version_numbers},0"))
        .replace("@VERSION_STRING@", &version);

    let resource_path = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"))
        .join("ralgrum.rc");
    fs::write(&resource_path, resources).expect("Windows resources must be writable");
    embed_resource::compile(&resource_path, embed_resource::NONE)
        .manifest_required()
        .expect("Windows application resources must compile");
}

fn select_murglar_backend() {
    println!("cargo:rustc-check-cfg=cfg(ralgrum_private_backend)");
    println!("cargo:rerun-if-env-changed=RALGRUM_MURGLAR_PRIVATE_DIR");

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR")
            .expect("Cargo must provide CARGO_MANIFEST_DIR when selecting the Murglar backend"),
    );
    let configured_dir = env::var_os("RALGRUM_MURGLAR_PRIVATE_DIR")
        .map(PathBuf::from)
        .map(|path| absolutize(path, &manifest_dir))
        .unwrap_or_else(|| manifest_dir.join("src/murglar_backend/implementation"));
    let configured_mod = configured_dir.join("mod.rs");
    println!("cargo:rerun-if-changed={}", configured_mod.display());

    let (module_path, is_private) = if configured_mod.is_file() {
        (configured_mod, true)
    } else {
        let stub = manifest_dir.join("src/murglar_backend/stub.rs");
        println!("cargo:rerun-if-changed={}", stub.display());
        (stub, false)
    };
    let module_path = module_path
        .canonicalize()
        .unwrap_or_else(|error| panic!("Murglar backend selector path is unavailable: {error}"));

    if is_private {
        println!("cargo:rustc-cfg=ralgrum_private_backend");
    }

    let out_dir = PathBuf::from(
        env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR for the backend selector"),
    );
    let selector = out_dir.join("murglar_backend_selector.rs");
    let module_path = rust_path_literal(&module_path);
    let source = format!("#[path = \"{module_path}\"]\nmod implementation;\n");
    fs::write(&selector, source).expect("Murglar backend selector must be writable");
}

fn absolutize(path: PathBuf, manifest_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        manifest_dir.join(path)
    }
}

fn rust_path_literal(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .replace('"', "\\\"")
}
