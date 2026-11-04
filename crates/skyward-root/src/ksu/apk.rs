//! Taking `ksud` out of a manager's APK.
//!
//! Some managers ship it as `lib/arm64-v8a/libksud.so`; SukiSU renames it, so
//! the other arm64 libraries are looked through for the one that is an
//! aarch64 program and names the parts of `boot-patch` it uses. The manager's
//! own JNI library is not taken for it.

use crate::backup::hex;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The library the managers that do not obfuscate it keep `ksud` in.
const PLAIN: &str = "lib/arm64-v8a/libksud.so";

/// The placement of the libraries an APK carries for a 64-bit arm phone.
const ARM64: &str = "lib/arm64-v8a/";

/// The `ksud` taken out of an APK, and the SHA-256 it had there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ksud {
    pub path: PathBuf,
    pub sha256: String,
}

/// Write the `ksud` in `apk` to `into/ksud`, where only root reads and runs
/// it.
pub fn extract(apk: &Path, into: &Path) -> Result<Ksud> {
    let mut zip = zip::ZipArchive::new(File::open(apk).with_context(|| format!("open {}", apk.display()))?)
        .with_context(|| format!("read the APK {}", apk.display()))?;
    let mut bytes = Vec::new();
    let plain = match zip.by_name(PLAIN) {
        Ok(mut entry) if entry.is_file() => {
            entry.read_to_end(&mut bytes).with_context(|| format!("read {PLAIN} in {}", apk.display()))?;
            true
        }
        _ => false,
    };
    if !plain {
        let mut found = None;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).with_context(|| format!("read the APK {}", apk.display()))?;
            let name = entry.name().to_string();
            if !entry.is_file() || !name.starts_with(ARM64) || !name.ends_with(".so") {
                continue;
            }
            let mut candidate = Vec::with_capacity(entry.size().min(32 << 20) as usize);
            entry.read_to_end(&mut candidate).with_context(|| format!("read {name} in {}", apk.display()))?;
            if looks_like_ksud(&candidate) {
                found = Some(candidate);
                break;
            }
        }
        bytes = found.with_context(|| format!("{} carries no ksud this recovery can run", apk.display()))?;
    }
    if !looks_like_ksud(&bytes) {
        anyhow::bail!("{} carries no ksud this recovery can run", apk.display());
    }
    let path = into.join("ksud");
    fs::write(&path, &bytes).with_context(|| format!("write {}", path.display()))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).with_context(|| format!("let {} be run", path.display()))?;
    Ok(Ksud { path, sha256: hex(&Sha256::digest(&bytes)) })
}

/// Whether `bytes` is an aarch64 program that is `ksud`, whatever its
/// library is called.
fn looks_like_ksud(bytes: &[u8]) -> bool {
    let elf = bytes.starts_with(b"\x7fELF") && bytes.get(4) == Some(&2) && bytes.get(18) == Some(&0xb7) && bytes.get(19) == Some(&0);
    elf && contains(bytes, b"/system/bin/linker64") && contains(bytes, b"boot-patch") && contains(bytes, b"out-name")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    /// An aarch64 ELF head with the strings `ksud` would have.
    fn ksud_bytes() -> Vec<u8> {
        let mut bytes = vec![0u8; 64];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[18] = 0xb7;
        bytes.extend_from_slice(b"/system/bin/linker64\0boot-patch\0out-name\0");
        bytes
    }

    fn apk(libs: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in libs {
            writer.start_file(*name, SimpleFileOptions::default().compression_method(CompressionMethod::Stored)).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn a_plain_libksud_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manager.apk");
        let bytes = ksud_bytes();
        fs::write(&path, apk(&[("lib/arm64-v8a/libksud.so", &bytes), ("lib/arm64-v8a/libkernelsu.so", b"jni")])).unwrap();
        let out = dir.path().join("tools");
        fs::create_dir(&out).unwrap();
        let found = extract(&path, &out).unwrap();
        assert_eq!(found.path, out.join("ksud"));
        assert_eq!(fs::read(&found.path).unwrap(), bytes);
        assert_eq!(found.sha256, hex(&Sha256::digest(&bytes)));
        assert_eq!(fs::metadata(&found.path).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn an_obfuscated_ksud_is_found_and_the_jni_library_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manager.apk");
        let bytes = ksud_bytes();
        fs::write(
            &path,
            apk(&[
                ("lib/arm64-v8a/libdukbknbamva.so", b"\x7fELF\x02\x01\x01\0jni"),
                ("lib/arm64-v8a/libkptools.so", &[b"\x7fELF\x02\x01\x01\0linker64 but no patch".as_slice()].concat()),
                ("lib/arm64-v8a/libfdbqiupsrx.so", &bytes),
            ]),
        )
        .unwrap();
        let out = dir.path().join("tools");
        fs::create_dir(&out).unwrap();
        assert_eq!(extract(&path, &out).unwrap().path, out.join("ksud"));
    }

    #[test]
    fn a_manager_with_no_ksud_is_turned_down() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manager.apk");
        fs::write(&path, apk(&[("lib/arm64-v8a/libkernelsu.so", b"jni")])).unwrap();
        fs::create_dir(dir.path().join("tools")).unwrap();
        assert!(extract(&path, &dir.path().join("tools")).is_err());
    }
}
