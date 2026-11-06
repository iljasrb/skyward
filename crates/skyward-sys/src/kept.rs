//! The little the recovery has to remember between boots: the settings the
//! user changes, and the network the WLAN joined.
//!
//! Settings are kept where the device config says: the vendor persist
//! partition, mounted where the phone's own table mounts it, which opens
//! before the data partition and no reset wipes, or the data partition,
//! which opens once the user answers the decrypt page. The data partition is
//! the default; the vendor one is for a phone whose recovery needs its
//! settings before that page is answered. The WLAN's networks are always
//! kept on the data partition, because they hold a password and persist is
//! not encrypted.
//!
//! A place is used only while it is mounted. A folder that is not a mount
//! point lives on the ramdisk and forgets what is written in it.
//!
//! A write is never half done: what is written goes to a temporary file
//! beside the real one, is flushed to the flash, and then takes its place in
//! one step, so a phone switched off mid-write keeps the copy from before.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// The vendor persist partition, mounted where the phone's own table mounts
/// it; none where the table names no persist partition.
pub fn vendor_root() -> Option<PathBuf> {
    let entry = skyward_block::fstab::partition("persist").ok().flatten()?;
    Some(PathBuf::from(entry.mount_point))
}

/// The data partition, which opens only once the user answers the decrypt
/// page.
pub const DATA: &str = "/data";

/// Whether `root` is a mount point rather than a folder on the ramdisk.
pub fn mounted(root: impl AsRef<Path>) -> bool {
    skyward_block::mount::is_mount_point(root.as_ref()).unwrap_or(false)
}

/// The recovery's folder on `root`.
const SKYWARD: &str = "skyward";

/// The folder `name` in the recovery's folder on `root`.
pub fn folder(root: impl AsRef<Path>, name: &str) -> PathBuf {
    root.as_ref().join(SKYWARD).join(name)
}

/// What `folder/name` holds; none while there is none.
pub fn read(root: impl AsRef<Path>, folder: &str, name: &str) -> Option<String> {
    fs::read_to_string(root.as_ref().join(SKYWARD).join(folder).join(name)).ok()
}

/// Write `text` as `folder/name`; the file it wrote is returned.
pub fn write(root: impl AsRef<Path>, folder: &str, name: &str, text: &str) -> Result<PathBuf> {
    let path = root.as_ref().join(SKYWARD).join(folder).join(name);
    let dir = path.parent().expect("a kept file is in a folder");
    fs::create_dir_all(dir).with_context(|| format!("make {}", dir.display()))?;
    // Nothing secret is kept here, but a partition the phone's own calibration
    // lives on is no place for a folder anyone may write in.
    use std::os::unix::fs::PermissionsExt;
    for folder in [Some(dir), dir.parent()].into_iter().flatten() {
        let _ = fs::set_permissions(folder, fs::Permissions::from_mode(0o755));
    }
    swap(&path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// Write `text` as `folder/name` for the recovery alone: the folder is
/// closed to everyone else, and so is the file. The file it wrote is
/// returned.
pub fn write_secret(root: impl AsRef<Path>, folder: &str, name: &str, text: &str) -> Result<PathBuf> {
    let path = root.as_ref().join(SKYWARD).join(folder).join(name);
    let dir = path.parent().expect("a kept file is in a folder");
    fs::create_dir_all(dir).with_context(|| format!("make {}", dir.display()))?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).with_context(|| format!("keep {} to the recovery", dir.display()))?;
    swap(&path, text).with_context(|| format!("write {}", path.display()))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).with_context(|| format!("keep {} to the recovery", path.display()))?;
    Ok(path)
}

/// Take `folder/name` away, if it is there.
pub fn remove(root: impl AsRef<Path>, folder: &str, name: &str) -> Result<()> {
    let path = root.as_ref().join(SKYWARD).join(folder).join(name);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("remove {}", path.display())),
    }
}

/// Put `text` in `path` whole: written beside it, flushed, and then moved
/// over it in one step.
fn swap(path: &Path, text: &str) -> Result<()> {
    let beside = path.with_extension("writing");
    {
        let mut file = fs::File::create(&beside).with_context(|| format!("make {}", beside.display()))?;
        use std::io::Write;
        file.write_all(text.as_bytes()).with_context(|| format!("write {}", beside.display()))?;
        file.sync_all().with_context(|| format!("flush {}", beside.display()))?;
    }
    fs::rename(&beside, path).with_context(|| format!("put {} in place of {}", beside.display(), path.display()))?;
    // The move itself is what has to outlive the phone going off.
    if let Some(dir) = path.parent() {
        let _ = fs::File::open(dir).and_then(|d| d.sync_all());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_that_is_not_a_mount_point_is_not_a_place() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!mounted(dir.path().to_str().unwrap()), "a folder on the rootfs forgets on the next boot");
    }

    #[test]
    fn a_file_is_replaced_whole_and_nothing_is_left_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("values");
        swap(&path, "brightness = 0.5\n").unwrap();
        swap(&path, "brightness = 0.7\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "brightness = 0.7\n", "the newer one is there whole");
        assert!(!path.with_extension("writing").exists(), "and the file it was written in is gone");
    }

    #[test]
    fn a_secret_is_kept_to_the_recovery_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = write_secret(dir.path(), "cloud", "values", "password = \"secret\"\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "password = \"secret\"\n");
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        remove(dir.path(), "cloud", "values").unwrap();
        assert!(!path.exists());
        remove(dir.path(), "cloud", "values").unwrap();
    }

    #[test]
    fn a_file_is_read_where_it_was_written() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        assert_eq!(read(root, "settings", "values"), None, "nothing written yet");
        let path = write(root, "settings", "values", "wlan = true\n").unwrap();
        assert_eq!(path, dir.path().join("skyward/settings/values"));
        assert_eq!(read(root, "settings", "values"), Some("wlan = true\n".to_string()));
        write(root, "settings", "values", "wlan = false\n").unwrap();
        assert_eq!(read(root, "settings", "values").unwrap(), "wlan = false\n", "the newer one is read");
    }
}
