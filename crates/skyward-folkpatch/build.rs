//! Fetch the FolkPatch release, once, and take the two KernelPatch tools out of
//! it, checking each against the hash this crate pins.
//!
//! The release is a package for the manager, and the tools sit in it; the
//! package itself is never embedded, saved or run, so what the compiler keeps
//! is `kptools` and `kpimg` alone.
//!
//! The download waits in the user's cache, so only the first build of a
//! machine pulls it; the files the compiler embeds are the ones found there,
//! and a byte changed in any of them stops the build rather than being
//! flashed onto a phone.
//!
//! `curl` does the fetching, as every machine this is built on has it, and
//! the decompression needs nothing but the `zip` crate.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use zip::ZipArchive;

/// The release the tools come in, and the SHA-256 that release gives it.
const PACKAGE: (&str, &str, &str) = (
    "FolkPatch_115404_0f3f42ba_on_main-release.apk",
    "https://github.com/LyraVoid/FolkPatch/releases/download/115404/FolkPatch_115404_0f3f42ba_on_main-release.apk",
    "1d29c147443ce0986e3d9f5241f9d377dc1e9c7a60ea51c165737748c5302d2f",
);

/// The tools inside it, and the SHA-256 of each.
const TOOLS: [(&str, &str, &str); 2] = [
    ("kptools", "lib/arm64-v8a/libkptools.so", "ea06a9a46dc1f210c71a61a9f07556fa9b739c327847c7ab16ccbb46fdb7631c"),
    ("kpimg", "assets/kpimg", "7a7aa4b930d1a9d684cbb426a3999f75cb2e2cead26673dcbbc5f70871f01141"),
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let cache = cache();
    fs::create_dir_all(&cache).unwrap_or_else(|e| panic!("make {}: {e}", cache.display()));
    let package = cached(PACKAGE.0, PACKAGE.1, PACKAGE.2, &cache);

    let mut assets = String::from("// Written by build.rs: the tools this crate carries, fetched and checked.\n\n");
    for (name, inside, wanted) in TOOLS {
        let path = unpack(&package, name, inside, wanted, &cache);
        assets.push_str(&format!(
            "/// `{name}`, as the release ships it.\npub const {}: &[u8] = include_bytes!({:?});\n",
            name.to_uppercase(),
            path.display().to_string()
        ));
        assets.push_str(&format!(
            "/// The SHA-256 of `{name}`.\npub const {}_SHA256: &str = {wanted:?};\n",
            name.to_uppercase()
        ));
    }
    fs::write(out.join("assets.rs"), assets).unwrap_or_else(|e| panic!("write assets.rs: {e}"));
}

/// Where a fetched file waits between builds, which `cargo clean` does not
/// touch.
fn cache() -> PathBuf {
    let root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .expect("neither XDG_CACHE_HOME nor HOME is set, so there is nowhere to keep the fetched assets");
    root.join("skyward").join("folkpatch")
}

/// The file `name` in `cache`, fetched from `url` the first time and taken
/// again only when what is there is not what `wanted` says it should be.
fn cached(name: &str, url: &str, wanted: &str, cache: &Path) -> PathBuf {
    let path = cache.join(name);
    if hash(&path).as_deref() == Some(wanted) {
        return path;
    }
    let part = cache.join(format!("{name}.part"));
    let status = Command::new("curl")
        .args(["--fail", "--location", "--silent", "--show-error", "--retry", "3", "--output"])
        .arg(&part)
        .arg(url)
        .status()
        .unwrap_or_else(|e| panic!("run curl to fetch {url}: {e}"));
    assert!(status.success(), "curl could not fetch {url}");
    assert_eq!(hash(&part).as_deref(), Some(wanted), "what {url} served is not the pinned file");
    fs::rename(&part, &path).unwrap_or_else(|e| panic!("put {} in place: {e}", path.display()));
    path
}

/// The tool `inside` the release, which is `name` here, unpacked into `cache`
/// and checked against `wanted`; the file is only there under its own name
/// once it is known good.
fn unpack(package: &Path, name: &str, inside: &str, wanted: &str, cache: &Path) -> PathBuf {
    let path = cache.join(name);
    if hash(&path).as_deref() == Some(wanted) {
        return path;
    }
    let file = fs::File::open(package).unwrap_or_else(|e| panic!("open {}: {e}", package.display()));
    let mut release = ZipArchive::new(file).unwrap_or_else(|e| panic!("read {} as a package: {e}", package.display()));
    let mut entry = release.by_name(inside).unwrap_or_else(|e| panic!("find {inside} in {}: {e}", package.display()));
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes).unwrap_or_else(|e| panic!("read {inside}: {e}"));
    assert_eq!(hex(&Sha256::digest(&bytes)), wanted, "{inside} is not the pinned tool");
    let mut written = fs::File::create(&path).unwrap_or_else(|e| panic!("make {}: {e}", path.display()));
    written.write_all(&bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    written.sync_all().unwrap_or_else(|e| panic!("flush {}: {e}", path.display()));
    path
}

/// The SHA-256 of the file at `path`, and none while there is no such file.
fn hash(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            return Some(hex(&digest.finalize()));
        }
        digest.update(&buffer[..read]);
    }
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}
