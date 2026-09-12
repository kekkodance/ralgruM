use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

// Private endpoint URLs, provider-specific endpoint paths, and identity field
// spellings. Store only byte lengths and SHA-256 digests.
// Shared public OAuth paths and audited public client identifiers are not private.
const PRIVATE_FINGERPRINTS: &[(usize, &str)] = &[
    (
        6,
        "06e3f9ebd64a38989bc1cf7d6aa182c5d13245805f86cde7446202c3170980b3",
    ),
    (
        7,
        "27385dcdd78cffb8e780d2c79b300b846c841891290ebbd3d84885f4e8f205df",
    ),
    (
        7,
        "73dfcd94d6e41b705d3daa1ab94a8640d15e9fa2f9450c04eb5dc5dfbc6dda85",
    ),
    (
        8,
        "383370eff4e18144129f5f422dc8755e968fa477d5cacefb3a82a5769347a6dd",
    ),
    (
        8,
        "d939095ab860ac9688b36973d6f9d8e57b67e35133699c5a4ddc5a102f04c0d9",
    ),
    (
        9,
        "0823a7f52a697b7b15ac25033aff09ee45a477785a0058d16f515a2b2299c04e",
    ),
    (
        9,
        "39a8d3c9f586c6bf8b7fda189c48704ea8f0b25ca4c7abe6ff0f807e81f0613f",
    ),
    (
        9,
        "7d69c4e269c6ba8f5bf834862b80f1178701a6697d3d19ab644a790de0b64a95",
    ),
    (
        9,
        "c02b57340cfd34afd88b3ebd1bbe11cb7936f96d3d6adb429c632a9c89dff7f9",
    ),
    (
        9,
        "e5fc6040c89fc2dd69f0ad54d9b8677f574d5f82515b9ec0523abecf296f35bb",
    ),
    (
        10,
        "0839a395ed6db44b84852abe854d7a3fe190e6a41e1aedb986584648aaf14c02",
    ),
    (
        10,
        "328ea2f1ebd623bfad39663de4d29aa767ae34b084d59f8be0fd7d328d528e1d",
    ),
    (
        10,
        "4f699be8388ef35281e24f7d11fa81ce74dcee92d3b633a0f76dd35591be0d8c",
    ),
    (
        10,
        "564ae601b4b68940ae9f3f5dae4d785facca504894c39e240b045e6818b57405",
    ),
    (
        10,
        "604fa9fa159d5c48da45d6034e730115db00576c10aa14606e0c3fc22f8670b6",
    ),
    (
        10,
        "734c879a4cf14a82ae88543e7d7072187b381c7a86af868f4c70162d742acbc4",
    ),
    (
        10,
        "b756f563d3281b3962adb33d0405d91bb11962072aadef97e78247f3ac499ba5",
    ),
    (
        11,
        "368dec86a1c9b85e5b4edca7bda12505969287d64bfb8c59be307e53eae66fae",
    ),
    (
        11,
        "8dcc3ad7959afd4a102299a18deb69675660989ea2f0778849504dcd0cdb5447",
    ),
    (
        11,
        "d2b51553e252724d089fd5ffd84bd1b40f96fb4f6bdb1fc8341288c55df38e45",
    ),
    (
        11,
        "d4768dda8719eba9e3f9ce5e3771c3c3f6ed7681348d5dd2f8f777417f916d5a",
    ),
    (
        12,
        "114eda9ec4106bd335e478ce08daf00f5dd136aa224ff473c2ec7053f5a3163b",
    ),
    (
        12,
        "36e3510cf08add8afd44eb4612d797258d88dfe493638475dee2c183e90ded13",
    ),
    (
        12,
        "61dfaf7b2a08919245113142e77f8b6f34b7c3e7ec70d619cb5cbb53986dc6c6",
    ),
    (
        13,
        "3cd8f9e20b4f9c280cc5c639aaa6ade4050cecd59fcff18291fbe43a57c47bb1",
    ),
    (
        15,
        "afd6b62e1ac1cd1a81db58d2d5f0fef7b580534b8480af26b528b36815768ec9",
    ),
    (
        16,
        "178164c3ea4775f5c335914e00df7d2e129e2154193872dde08609fdcb447213",
    ),
    (
        16,
        "9dc1f4328ec698e4c58628fa83a6649a2d1fa1339882fb64c26b9262a39a7ce8",
    ),
    (
        16,
        "9fc8533ca8f446f2152e12a9331f2f288c8899a5a64a36b6bd7f4551e8788d34",
    ),
    (
        17,
        "ecfd737d5549e7f5dc114334de8a9132a6923cd94294afd6ffb6bdf454785085",
    ),
    (
        19,
        "f9b2418eff3089865d78b4c4595802aef7d548b94909cded2459ee8c0fadd7d2",
    ),
    (
        31,
        "08734d93a836bc5e042d0605ef5c095881c14b72c27318a9d81609acc9790a7f",
    ),
    (
        31,
        "92106979557c65f2f0ea2d864fcc93c08eefddade9d1afe9b2196a16d7e310da",
    ),
    (
        34,
        "5f53874b87af4c025c5577db6667452ffedf7debdd62a5b463a9581c3003ad06",
    ),
    (
        35,
        "192e0cea9019c29172bd7df5d019df7f49a50113848483bb0340e2dca3e2596a",
    ),
    (
        36,
        "d7fda4964414b352c1fa0a9d7eebe11666250f9db4f1bfab45d7268cafcf3a57",
    ),
    (
        38,
        "a06261847f6bc88859e98a36d79acf04c22f9419edaf11885b8febe0bf70005f",
    ),
    (
        53,
        "da8a7eb9693673778958f1f15adf185a4f4d1833725a8329a1ab841791624fa6",
    ),
    (
        55,
        "15418b77a97b8c1148d1660447cccd1a54c8fd5b4b836c9666d7548e58f805f3",
    ),
];

// Former imports and direct implementation access violate the facade boundary.
// Keeping these opaque also lets the scanner inspect this module itself.
const BOUNDARY_FINGERPRINTS: &[(usize, &str)] = &[
    (
        12,
        "cf25b6266087a897d1d134fb85c0ae43cb54bd5d6beb7b13a04a84b211dc458c",
    ),
    (
        16,
        "440b261976409dccb56435f4cff38862183ae70ffaf726876d75a6b58eed877f",
    ),
    (
        16,
        "6954641da8e74639c113f0f719b758cd9b713b604fa3c7526c37887b63a856b5",
    ),
    (
        21,
        "a2bb8d59108798ca7f2f8fef86c879c210dc754ef20f5446ba48c1212e30ab9f",
    ),
    (
        23,
        "72fe5ef64f7077386350c5caa0dfde366ef14ef488096b0e4e58530a30583806",
    ),
    (
        24,
        "5870222c431dbb32dadea3f6a2fb16ce2d273b2539f55b3b838518b08ab2067f",
    ),
    (
        25,
        "cc8aab3929790787428fd195572568b9ae5218b6620e596bffcd0acfdb10b538",
    ),
    (
        28,
        "9da013f8e689965ccfeafdc6902ef325fe66c08ed8f7d0a138ac66049130f50b",
    ),
    (
        31,
        "d1fb23aeacc3fc14cdb289ff58544f1043ee5f730936060199c7eda85efd662d",
    ),
    (
        32,
        "c080febb61c3f2f55a9109fb6e6ad352b60c1615ef98844c3ab4b049f3a500fd",
    ),
];

type FingerprintGroups = BTreeMap<usize, Vec<[u8; 32]>>;

fn fingerprint_groups(entries: &[(usize, &str)]) -> FingerprintGroups {
    let mut groups = BTreeMap::<usize, Vec<[u8; 32]>>::new();
    for &(length, hex) in entries {
        assert!(length > 0);
        assert_eq!(hex.len(), 64);
        let mut digest = [0; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
                .expect("invalid opaque fingerprint");
        }
        groups.entry(length).or_default().push(digest);
    }
    groups
}

fn matching_fingerprint(source: &[u8], groups: &FingerprintGroups) -> Option<[u8; 32]> {
    for (&length, fingerprints) in groups {
        for window in source.windows(length) {
            let digest: [u8; 32] = Sha256::digest(window).into();
            if fingerprints.contains(&digest) {
                return Some(digest);
            }
        }
    }
    None
}

fn hex_digest(digest: &[u8; 32]) -> String {
    use std::fmt::Write as _;

    let mut hex = String::with_capacity(64);
    for byte in digest {
        write!(&mut hex, "{byte:02x}").expect("writing a digest to a string");
    }
    hex
}

fn excluded(relative: &Path) -> bool {
    relative == Path::new("src/murglar_backend/implementation")
        || relative == Path::new("lyrics-core/target")
        || [
            ".git",
            "target",
            "vendor",
            "build",
            "dist",
            "node_modules",
            "traces",
            "logs",
        ]
        .iter()
        .any(|path| relative == Path::new(path))
        || (relative.components().count() == 1
            && relative
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("target-")))
}

fn public_rust_sources(checkout: &Path) -> Result<Vec<PathBuf>, String> {
    fn visit(checkout: &Path, directory: &Path, sources: &mut Vec<PathBuf>) -> Result<(), String> {
        let relative_directory = directory
            .strip_prefix(checkout)
            .expect("checkout descendant");
        if !relative_directory.as_os_str().is_empty()
            && directory.join(".git").try_exists().map_err(|_| {
                format!(
                    "{}: unreadable checkout boundary",
                    relative_directory.display()
                )
            })?
        {
            return Err(format!(
                "{}: unexpected nested checkout",
                relative_directory.display()
            ));
        }
        let entries = fs::read_dir(directory).map_err(|_| {
            format!(
                "{}: unreadable source directory",
                relative_directory.display()
            )
        })?;
        let mut entries = entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| format!("{}: unreadable source entry", relative_directory.display()))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(checkout).expect("checkout descendant");
            // Prune before querying metadata or following links. The ignored
            // private checkout may be absent, unreadable, or linked elsewhere.
            if excluded(relative) {
                continue;
            }
            let kind = entry
                .file_type()
                .map_err(|_| format!("{}: unreadable source metadata", relative.display()))?;
            if kind.is_symlink() {
                return Err(format!(
                    "{}: source links are not allowed",
                    relative.display()
                ));
            }
            // Windows directory junctions need the canonical-path check too.
            let canonical = path
                .canonicalize()
                .map_err(|_| format!("{}: unreadable source path", relative.display()))?;
            if canonical != path {
                return Err(format!(
                    "{}: source links are not allowed",
                    relative.display()
                ));
            }
            if kind.is_dir() {
                visit(checkout, &path, sources)?;
            } else if kind.is_file() && path.extension().is_some_and(|extension| extension == "rs")
            {
                sources.push(path);
            }
        }
        Ok(())
    }

    let checkout = checkout
        .canonicalize()
        .map_err(|_| "unreadable checkout root".to_owned())?;
    let mut sources = Vec::new();
    visit(&checkout, &checkout, &mut sources)?;
    sources.sort();
    Ok(sources)
}

fn violation(path: &Path, digest: &[u8; 32]) -> String {
    format!("{}: {}", path.display(), hex_digest(digest))
}

fn scan_public_sources(checkout: &Path, fingerprints: &FingerprintGroups) -> Result<(), String> {
    let checkout = checkout
        .canonicalize()
        .map_err(|_| "unreadable checkout root".to_owned())?;
    for path in public_rust_sources(&checkout)? {
        let relative = path.strip_prefix(&checkout).expect("checkout descendant");
        let source = fs::read(&path)
            .map_err(|_| format!("{}: unreadable Rust source", relative.display()))?;
        if let Some(digest) = matching_fingerprint(&source, fingerprints) {
            return Err(violation(relative, &digest));
        }
    }
    Ok(())
}

#[test]
fn public_rust_source_keeps_private_protocol_and_architecture_behind_the_facade() {
    let mut entries = PRIVATE_FINGERPRINTS.to_vec();
    entries.extend_from_slice(BOUNDARY_FINGERPRINTS);
    let fingerprints = fingerprint_groups(&entries);
    if let Err(failure) = scan_public_sources(Path::new(env!("CARGO_MANIFEST_DIR")), &fingerprints)
    {
        panic!("{failure}");
    }
}

#[test]
fn shared_retry_has_no_backend_protocol_dependency() {
    let retry = include_str!("../playback/retry.rs");
    let production = retry
        .split_once("#[cfg(test)]")
        .map_or(retry, |(production, _)| production);
    for marker in ["murglar", "Murglar", "acquire_request_slot"] {
        if production.contains(marker) {
            let digest: [u8; 32] = Sha256::digest(marker.as_bytes()).into();
            panic!("{}", violation(Path::new("src/playback/retry.rs"), &digest));
        }
    }
}

fn fixture_fingerprints(value: &[u8]) -> FingerprintGroups {
    let digest: [u8; 32] = Sha256::digest(value).into();
    fingerprint_groups(&[(value.len(), &hex_digest(&digest))])
}

#[test]
fn opaque_matching_finds_embedded_generic_values_at_source_boundaries() {
    let value = b"example_private_field";
    let fingerprints = fixture_fingerprints(value);
    let expected: [u8; 32] = Sha256::digest(value).into();
    for source in [
        [b"prefix_", value.as_slice(), b"_suffix"].concat(),
        value.to_vec(),
        [value.as_slice(), b" = 1;"].concat(),
        [b"let ", value.as_slice()].concat(),
        [b"// unrelated\nlet ", value.as_slice(), b" = 1;"].concat(),
    ] {
        assert_eq!(matching_fingerprint(&source, &fingerprints), Some(expected));
    }
    assert_eq!(matching_fingerprint(b"", &fingerprints), None);
    assert_eq!(
        matching_fingerprint(&value[..value.len() - 1], &fingerprints),
        None
    );
    assert_eq!(
        matching_fingerprint(b"example_public_field", &fingerprints),
        None
    );
}

#[test]
fn opaque_matching_finds_generic_endpoint_substrings() {
    let endpoint = b"https://example.invalid/private/fixture";
    let fingerprints = fixture_fingerprints(endpoint);
    let digest: [u8; 32] = Sha256::digest(endpoint).into();
    assert_eq!(
        matching_fingerprint(
            &[
                b"const URL: &str = \"",
                endpoint.as_slice(),
                b"?key=opaque\";"
            ]
            .concat(),
            &fingerprints,
        ),
        Some(digest),
    );
    assert_eq!(
        matching_fingerprint(b"https://public.example/private/fixture", &fingerprints),
        None,
    );
}

#[test]
fn a_privacy_failure_contains_only_its_source_path_and_digest() {
    let digest: [u8; 32] = Sha256::digest(b"example_private_field").into();
    let path = Path::new("src/fixture.rs");
    assert_eq!(
        violation(path, &digest),
        format!(
            "{}: {:x}",
            path.display(),
            Sha256::digest(b"example_private_field")
        )
    );
}

#[test]
fn newly_added_public_sources_are_scanned_with_and_without_a_private_checkout() {
    let checkout = tempfile::tempdir().unwrap();
    let root = checkout.path();
    let private = root.join("src/murglar_backend/implementation");
    let new_source = Path::new("tests/new_module/nested.rs");
    let value = b"example_private_field";
    let fingerprints = fixture_fingerprints(value);
    let digest: [u8; 32] = Sha256::digest(value).into();
    fs::create_dir_all(root.join("src/murglar_backend")).unwrap();
    fs::create_dir_all(root.join("tests/new_module")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        root.join("src/murglar_backend/stub.rs"),
        "struct Backend;\n",
    )
    .unwrap();

    for with_private in [false, true] {
        if with_private {
            fs::create_dir_all(&private).unwrap();
            fs::write(private.join("protocol.rs"), value).unwrap();
            // Even invalid UTF-8 and nested sources in the private tree are ignored.
            fs::create_dir_all(private.join("nested")).unwrap();
            fs::write(private.join("nested/hidden.rs"), [0xff, 0xfe]).unwrap();
        }
        assert_eq!(scan_public_sources(root, &fingerprints), Ok(()));
        fs::write(
            root.join(new_source),
            [b"const ", value.as_slice(), b": u8 = 1;"].concat(),
        )
        .unwrap();
        let error = scan_public_sources(root, &fingerprints)
            .expect_err("new public source must be scanned");
        let (reported_path, reported_digest) = error
            .split_once(": ")
            .expect("privacy failure must contain a source path and digest");
        assert_eq!(Path::new(reported_path), new_source);
        assert_eq!(reported_digest, hex_digest(&digest));
        fs::remove_file(root.join(new_source)).unwrap();
        assert_eq!(scan_public_sources(root, &fingerprints), Ok(()));
    }
}

#[test]
fn source_discovery_is_deterministic_and_only_omits_exact_excluded_roots() {
    let checkout = tempfile::tempdir().unwrap();
    let root = checkout.path();
    for path in [
        "src/z.rs",
        "src/nested/a.rs",
        "build.rs",
        "examples/demo.rs",
        "src/implementation/public.rs",
        "src/target/public.rs",
        "src/vendor/public.rs",
        "src/murglar_backend/implementation_backup/public.rs",
        "src/murglar_backend/implementation/private.rs",
        "target/generated.rs",
        "vendor/third_party.rs",
        ".git/ignored.rs",
        "target-release/generated.rs",
        "lyrics-core/target/generated.rs",
        "build/generated.rs",
        "dist/generated.rs",
        "node_modules/generated.rs",
        "traces/captured.rs",
        "logs/captured.rs",
    ] {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }
    fs::write(root.join("src/not_rust.txt"), "").unwrap();
    let root = root.canonicalize().unwrap();
    let relative = public_rust_sources(&root)
        .unwrap()
        .into_iter()
        .map(|path| path.strip_prefix(&root).unwrap().to_path_buf())
        .collect::<Vec<_>>();
    assert_eq!(
        relative,
        [
            "build.rs",
            "examples/demo.rs",
            "src/implementation/public.rs",
            "src/murglar_backend/implementation_backup/public.rs",
            "src/nested/a.rs",
            "src/target/public.rs",
            "src/vendor/public.rs",
            "src/z.rs",
        ]
        .map(PathBuf::from)
    );
}

#[test]
fn source_discovery_rejects_unexpected_external_checkouts() {
    let checkout = tempfile::tempdir().unwrap();
    fs::create_dir_all(checkout.path().join("external/.git")).unwrap();
    fs::write(
        checkout.path().join("external/private.rs"),
        "not a source to read",
    )
    .unwrap();
    assert_eq!(
        public_rust_sources(checkout.path()),
        Err("external: unexpected nested checkout".to_owned()),
    );
}

#[cfg(unix)]
#[test]
fn source_discovery_never_follows_public_links_into_private_sources() {
    let checkout = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir_all(checkout.path().join("src/murglar_backend")).unwrap();
    let private = checkout.path().join("src/murglar_backend/implementation");
    std::os::unix::fs::symlink(outside.path(), &private).unwrap();
    assert!(public_rust_sources(checkout.path()).unwrap().is_empty());
    let alias = checkout.path().join("src/public_alias");
    std::os::unix::fs::symlink(outside.path(), &alias).unwrap();
    assert!(public_rust_sources(checkout.path()).is_err());
}
