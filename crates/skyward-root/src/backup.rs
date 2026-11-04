//! What a run leaves behind: the whole of each boot partition as it was
//! before it was written, and the manifest that says what the run was and
//! where those copies are.
//!
//! The copies are of the partitions rather than of the images in them, so
//! `fastboot flash boot_a boot_a.img` puts back exactly what was there.

use crate::folkpatch::Stage;
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use skyward_vab::Slot;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// How much is read and written at a time.
const CHUNK: usize = 1 << 20;

/// What a run saves of a boot partition before patching it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backup {
    pub slot: Slot,
    /// The partition the copy is of, such as `boot` or `init_boot`.
    pub partition: String,
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

/// The file a slot's copy is kept in.
pub fn name(slot: Slot) -> String {
    format!("boot{}.img", slot.suffix())
}

/// The file a slot's copy of `partition` is kept in.
pub fn partition_name(partition: &str, slot: Slot) -> String {
    format!("{partition}{}.img", slot.suffix())
}

/// Where a run keeps what it makes: a folder of its own, never one an earlier
/// run wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The run's folder, as `/sdcard/Download/Skyward/FolkPatch/<stamp>`.
    pub dir: PathBuf,
    /// The folder of the copies of the boot partitions.
    pub backups: PathBuf,
}

impl Layout {
    /// Make a run's folder under `storage` for `stamp`, or the next name that
    /// is free when an earlier run already has that one.
    pub fn make(storage: &Path, stamp: &str) -> Result<Layout> {
        Layout::make_in(storage, "FolkPatch", stamp)
    }

    /// Make a run's folder under `storage` for `stamp` in `folder`, the
    /// folder of the root method it belongs to, or the next name that is free
    /// when an earlier run already has that one.
    pub fn make_in(storage: &Path, folder: &str, stamp: &str) -> Result<Layout> {
        let runs = storage.join("Download").join("Skyward").join(folder);
        fs::create_dir_all(&runs).with_context(|| format!("make {}", runs.display()))?;
        for n in 1.. {
            let name = match n {
                1 => stamp.to_string(),
                n => format!("{stamp}-{n}"),
            };
            let dir = runs.join(name);
            match fs::create_dir(&dir) {
                Ok(()) => {
                    let backups = dir.join("backup");
                    fs::create_dir(&backups).with_context(|| format!("make {}", backups.display()))?;
                    return Ok(Layout { dir, backups });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e).with_context(|| format!("make {}", dir.display())),
            }
        }
        unreachable!("a name is free or the loop ends")
    }
}

/// Save the whole of `source` as `slot`'s copy of `partition` in `into`,
/// reading it back before saying it is there. Nothing is left in the copy's
/// name until it is known good, so an interrupted run leaves a `.part` file
/// and no copy to mistake for one.
pub fn save(source: &Path, into: &Path, slot: Slot, tell: &mut dyn FnMut(Stage)) -> Result<Backup> {
    save_as(source, into, slot, crate::BOOT, &name(slot), tell)
}

/// Save the whole of `source` as `slot`'s copy of `partition` in `into`,
/// under `file`, reading it back before saying it is there.
pub fn save_as(source: &Path, into: &Path, slot: Slot, partition: &str, file: &str, tell: &mut dyn FnMut(Stage)) -> Result<Backup> {
    save_with(source, into, slot, partition, file, &mut |done, total| tell(Stage::BackingUp { slot, done, total }))
}

/// Save the whole of `source` as `slot`'s copy of `partition` in `into`,
/// under `file`, telling how far it has come in bytes; shared by the root
/// methods, whose runs say their progress in their own words.
pub fn save_with(
    source: &Path,
    into: &Path,
    slot: Slot,
    partition: &str,
    file: &str,
    tell: &mut dyn FnMut(u64, u64),
) -> Result<Backup> {
    let path = into.join(file);
    let part = path.with_extension("img.part");
    let mut reading = File::open(source).with_context(|| format!("open {}", source.display()))?;
    // A block device tells no size to metadata; it does say where its end is,
    // which is the whole partition.
    let total = reading.seek(SeekFrom::End(0)).with_context(|| format!("find the size of {}", source.display()))?;
    reading.seek(SeekFrom::Start(0)).with_context(|| format!("rewind {}", source.display()))?;
    let mut written = File::options()
        .write(true)
        .create_new(true)
        .open(&part)
        .with_context(|| format!("make {}", part.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    let mut done = 0;
    loop {
        let read = reading.read(&mut buffer).with_context(|| format!("read {}", source.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        written.write_all(&buffer[..read]).with_context(|| format!("write {}", part.display()))?;
        done += read as u64;
        tell(done, total);
    }
    written.sync_all().with_context(|| format!("flush {}", part.display()))?;
    drop(written);
    let sha256 = hex(&digest.finalize());

    // Read the copy back in the same chunks it was written in, so that a
    // partition's whole size is never held in memory at once: what is kept
    // is what is read, not what was meant to be written.
    let mut kept = File::open(&part).with_context(|| format!("read {} back", part.display()))?;
    let mut again = Sha256::new();
    let mut size = 0;
    loop {
        let read = kept.read(&mut buffer).with_context(|| format!("read {} back", part.display()))?;
        if read == 0 {
            break;
        }
        again.update(&buffer[..read]);
        size += read as u64;
    }
    ensure!(size == done, "{} holds {size} bytes, not the {done} read from {}", part.display(), source.display());
    ensure!(hex(&again.finalize()) == sha256, "{} is not what {} holds", part.display(), source.display());
    fs::rename(&part, &path).with_context(|| format!("put {} in place of {}", part.display(), path.display()))?;
    if let Some(dir) = path.parent() {
        let _ = File::open(dir).and_then(|d| d.sync_all());
    }
    Ok(Backup { slot, partition: partition.to_string(), path, size: total, sha256 })
}

/// What a run's manifest says: the phone, the slots, the pinned files it wrote
/// and where it saved them. The key is not among them, and neither is a hash
/// of it.
pub struct Facts<'a> {
    /// The phone's own name, as its configuration names it.
    pub device: &'a str,
    /// The slot the phone started from.
    pub current: Slot,
    /// The slots patched.
    pub slots: &'a [Slot],
    pub kpimg_version: &'a str,
    pub kpimg_sha256: &'a str,
    pub kptools_sha256: &'a str,
    /// The copies made, in the order they were made.
    pub backups: &'a [Backup],
    /// Where only root reads the key.
    pub key_path: &'a Path,
}

/// The manifest as it is kept beside the copies.
pub fn manifest(facts: &Facts) -> String {
    let slots: Vec<&str> = facts.slots.iter().map(|s| s.letter()).collect();
    let mut text = String::new();
    text.push_str(&format!("device = {}\n", facts.device));
    text.push_str(&format!("current_slot = {}\n", facts.current.letter()));
    text.push_str(&format!("slots = {}\n", slots.join(", ")));
    text.push_str(&format!("kpimg_version = {}\n", facts.kpimg_version));
    text.push_str(&format!("kpimg_sha256 = {}\n", facts.kpimg_sha256));
    text.push_str(&format!("kptools_sha256 = {}\n", facts.kptools_sha256));
    text.push_str(&format!("key_file = {}\n", facts.key_path.display()));
    for backup in facts.backups {
        let kept = backup.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        text.push_str(&format!("backup{} = backup/{kept} {} {}\n", backup.slot.suffix(), backup.size, backup.sha256));
    }
    text
}

/// Write `facts` as the run's manifest, and say where it went.
pub fn write(facts: &Facts, run: &Path) -> Result<PathBuf> {
    let path = run.join("manifest.txt");
    fs::write(&path, manifest(facts)).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// A digest as the manifest and the screen write it.
pub(crate) fn hex(digest: &[u8]) -> String {
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The size of the partition `device` is.
pub(crate) fn partition_size(device: &Path) -> Result<u64> {
    let mut file = File::open(device).with_context(|| format!("open {}", device.display()))?;
    file.seek(SeekFrom::End(0)).with_context(|| format!("find the size of {}", device.display()))
}

/// The SHA-256 of the file at `path`.
pub(crate) fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = file.read(&mut buffer).with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex(&digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn source(dir: &Path, bytes: &[u8]) -> PathBuf {
        let path = dir.join("boot_a");
        fs::write(&path, bytes).unwrap();
        path
    }

    fn quiet() -> impl FnMut(Stage) {
        |_| {}
    }

    #[test]
    fn a_copy_is_the_whole_partition_and_is_named_for_its_slot() {
        let dir = tempfile::tempdir().unwrap();
        let bytes: Vec<u8> = (0..(3 * CHUNK + 7)).map(|n| n as u8).collect();
        let source = source(dir.path(), &bytes);
        let into = dir.path().join("backup");
        fs::create_dir(&into).unwrap();
        let mut told = 0;
        let backup = save(&source, &into, Slot::A, &mut |_| told += 1).unwrap();
        assert_eq!(backup.path, into.join("boot_a.img"));
        assert_eq!(name(Slot::B), "boot_b.img");
        assert_eq!(backup.size, bytes.len() as u64);
        assert_eq!(fs::read(&backup.path).unwrap(), bytes, "the copy is the partition whole");
        assert_eq!(backup.sha256, hex(&Sha256::digest(&bytes)));
        assert!(!into.join("boot_a.img.part").exists(), "nothing is left in its place");
        assert!(told > 1, "the screen hears how far it has come");
    }

    #[test]
    fn a_run_gets_a_folder_no_earlier_run_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let first = Layout::make(dir.path(), "20261101-123456").unwrap();
        let second = Layout::make(dir.path(), "20261101-123456").unwrap();
        assert_eq!(first.dir, dir.path().join("Download/Skyward/FolkPatch/20261101-123456"));
        assert_eq!(second.dir, dir.path().join("Download/Skyward/FolkPatch/20261101-123456-2"));
        assert!(first.backups.is_dir() && second.backups.is_dir());
    }

    #[test]
    fn the_manifest_says_what_the_run_was_and_where_it_saved_the_copies() {
        let dir = tempfile::tempdir().unwrap();
        let backups = dir.path().join("run/backup");
        fs::create_dir_all(&backups).unwrap();
        let source = source(dir.path(), b"a boot image");
        let backup = save(&source, &backups, Slot::A, &mut quiet()).unwrap();
        let key_path = dir.path().join("run/superkey.txt");
        let facts = Facts {
            device: "nezha",
            current: Slot::A,
            slots: &[Slot::A, Slot::B],
            kpimg_version: crate::folkpatch::KPIMG_VERSION,
            kpimg_sha256: "7a7a",
            kptools_sha256: "ea06",
            backups: std::slice::from_ref(&backup),
            key_path: &key_path,
        };
        let text = manifest(&facts);
        for wanted in [
            "device = nezha",
            "current_slot = A",
            "slots = A, B",
            crate::folkpatch::KPIMG_VERSION,
            "backup_a = backup/boot_a.img 12 ",
            &backup.sha256,
        ] {
            assert!(text.contains(wanted), "{wanted} is not in:\n{text}");
        }
        assert!(text.contains(&key_path.display().to_string()), "the key file is pointed at");
        let key = crate::folkpatch::SuperKey::validate("hidden123").unwrap();
        assert!(!text.contains(key.expose()), "and the key itself is not in it");
        write(&facts, &dir.path().join("run")).unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("run/manifest.txt")).unwrap(), text);
    }
}
