//! The little the recovery has to remember between boots: the settings the
//! user changes, and the network the WLAN joined.
//!
//! Settings are kept where the device config says: the vendor persist
//! partition, which opens before the data partition and no reset wipes, or
//! the data partition, which opens once the user answers the decrypt page.
//! The WLAN's networks are always kept on the data partition, because they
//! hold a password and persist is not encrypted.
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

/// The vendor partition the phone keeps its calibration in.
pub const VENDOR: &str = "/mnt/vendor/persist";

/// The data partition, which opens only once the user answers the decrypt
/// page.
pub const DATA: &str = "/data";

/// Whether `root` is a mount point rather than a folder on the ramdisk.
pub fn mounted(root: &str) -> bool {
    skyward_block::mount::is_mount_point(Path::new(root)).unwrap_or(false)
}

/// The recovery's folder on `root`.
const SKYWARD: &str = "skyward";

/// The folder `name` in the recovery's folder on `root`.
pub fn folder(root: &str, name: &str) -> PathBuf {
    Path::new(root).join(SKYWARD).join(name)
}

/// Where `folder/name` is, and what it holds; none while there is none.
pub fn read(root: &str, folder: &str, name: &str) -> Option<(PathBuf, String)> {
    let path = Path::new(root).join(SKYWARD).join(folder).join(name);
    let text = fs::read_to_string(&path).ok()?;
    Some((path, text))
}

/// Write `text` as `folder/name`; the file it wrote is returned.
pub fn write(root: &str, folder: &str, name: &str, text: &str) -> Result<PathBuf> {
    let path = Path::new(root).join(SKYWARD).join(folder).join(name);
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
    fn a_file_is_read_where_it_was_written() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        assert_eq!(read(root, "settings", "values"), None, "nothing written yet");
        let path = write(root, "settings", "values", "wlan = true\n").unwrap();
        assert_eq!(path, dir.path().join("skyward/settings/values"));
        assert_eq!(read(root, "settings", "values"), Some((path, "wlan = true\n".to_string())));
        write(root, "settings", "values", "wlan = false\n").unwrap();
        assert_eq!(read(root, "settings", "values").unwrap().1, "wlan = false\n", "the newer one is read");
    }
}
