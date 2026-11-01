//! The little the recovery has to remember between boots: the settings the
//! user changes, and the network the WLAN joined.
//!
//! The places it can be kept, best first, are the phone's own:
//!
//! - `/mnt/vendor/persist`, the partition the phone keeps its calibration in,
//!   which the recovery mounts as it brings the phone up and no reset wipes;
//! - the data partition, which opens only once the user answers the decrypt
//!   page;
//! - the owner's storage inside it, which waits on their screen lock as well.
//!
//! Which one is used is decided by what is there: the file newest written is
//! the one read, and every place that takes a write gets it, so a place the
//! last write could not reach is brought up to date by the next instead of
//! keeping a copy that disagrees for good.
//!
//! The WLAN's networks are kept on the encrypted data partition only, because
//! they hold a password and persist is not encrypted.
//!
//! A write is never half done: what is written goes to a temporary file
//! beside the real one, is flushed to the flash, and then takes its place in
//! one step, so a phone switched off mid-write keeps the copy from before.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Where a folder may be made, best first.
const PLACES: [&str; 3] = ["/mnt/vendor/persist/skyward", "/data/skyward", "/sdcard/Skyward"];

/// The data partition, which is encrypted and opens once the user answers
/// the decrypt page.
pub const DATA: &str = "/data";

/// The places, in the order they are tried.
pub fn places() -> impl Iterator<Item = &'static str> {
    PLACES.into_iter()
}

/// Whether `root` is a mount point rather than a folder on the ramdisk.
pub fn mounted(root: &str) -> bool {
    skyward_block::mount::is_mount_point(Path::new(root)).unwrap_or(false)
}

/// The folder `name` under `root`.
pub fn folder(root: &str, name: &str) -> PathBuf {
    Path::new(root).join(name)
}

/// Where `folder/name` last was, the file of them written most recently, and
/// what it holds; none while no place holds one.
pub fn read(folder: &str, name: &str) -> Option<(PathBuf, String)> {
    read_in(&PLACES.map(Path::new), folder, name)
}

/// As [`read`], among `roots` given in the order they are tried.
pub fn read_in(roots: &[&Path], folder: &str, name: &str) -> Option<(PathBuf, String)> {
    let mut best: Option<(SystemTime, PathBuf, String)> = None;
    for root in roots {
        let path = root.join(folder).join(name);
        let Some(text) = fs::read_to_string(&path).ok() else { continue };
        let written = fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
        // Two written in the same moment came from one write that went to
        // both, and the first place is the one to trust of them.
        if best.as_ref().is_none_or(|(at, ..)| written > *at) {
            best = Some((written, path, text));
        }
    }
    best.map(|(_, path, text)| (path, text))
}

/// Write `text` as `folder/name` in every place that takes it, the one `was`
/// is in first; the best place it went to is returned.
pub fn write(folder: &str, name: &str, text: &str, was: Option<&Path>) -> Result<PathBuf> {
    write_in(&PLACES.map(Path::new), folder, name, text, was)
}

/// As [`write`], among `roots` given in the order they are tried.
pub fn write_in(roots: &[&Path], folder: &str, name: &str, text: &str, was: Option<&Path>) -> Result<PathBuf> {
    // The better place is written last, so that of two written one after the
    // other the read, which trusts the newest, trusts the better one. A place
    // the last write could not reach is written first of the roots and the
    // next one takes the write instead.
    let mut places: Vec<PathBuf> = roots.iter().rev().map(|root| root.join(folder).join(name)).collect();
    if let Some(was) = was {
        if !places.iter().any(|path| path == was) {
            places.insert(0, was.to_path_buf());
        }
    }
    let mut written: Option<PathBuf> = None;
    let mut last: Option<anyhow::Error> = None;
    for path in places {
        let Some(dir) = path.parent() else { continue };
        if fs::create_dir_all(dir).is_err() {
            continue;
        }
        // Nothing secret is kept here, but a partition the phone's own
        // calibration lives on is no place for a folder anyone may write in.
        use std::os::unix::fs::PermissionsExt;
        for folder in [Some(dir), dir.parent()].into_iter().flatten() {
            let _ = fs::set_permissions(folder, fs::Permissions::from_mode(0o755));
        }
        // Every place that takes it gets it, so a place the last write could
        // not reach is caught up here instead of left with a copy the read
        // may then trust. One place refusing it is not a failure while
        // another takes it.
        match swap(&path, text) {
            Ok(()) => {
                written = Some(path);
            }
            Err(e) => last = Some(e.context(format!("write {}", path.display()))),
        }
    }
    written.ok_or_else(|| last.unwrap_or_else(|| anyhow::anyhow!("nowhere to keep {folder}/{name}")))
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

    fn roots(dir: &Path) -> Vec<PathBuf> {
        (0..2).map(|n| dir.join(format!("place{n}"))).collect()
    }

    fn as_refs(paths: &[PathBuf]) -> Vec<&Path> {
        paths.iter().map(PathBuf::as_path).collect()
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
    fn the_copy_written_most_recently_is_the_one_read() {
        let dir = tempfile::tempdir().unwrap();
        let places = roots(dir.path());
        let roots = as_refs(&places);
        assert_eq!(read_in(&roots, "settings", "values"), None, "nothing written yet");
        write_in(&roots[1..], "settings", "values", "wlan = false\n", None).unwrap();
        // The better place is the first of them, and the write reaches the
        // worse one first, so the better one is the newest the read sees.
        write_in(&roots, "settings", "values", "wlan = true\n", None).unwrap();
        let (path, text) = read_in(&roots, "settings", "values").expect("a file");
        assert_eq!(path, places[0].join("settings/values"));
        assert_eq!(text, "wlan = true\n");
    }

    #[test]
    fn every_place_that_takes_it_gets_it() {
        let dir = tempfile::tempdir().unwrap();
        let places = roots(dir.path());
        let roots = as_refs(&places);
        let path = write_in(&roots, "settings", "values", "wlan = true\n", None).unwrap();
        assert_eq!(path, places[0].join("settings/values"), "the better place is the one answered with");
        for place in &places {
            assert_eq!(fs::read_to_string(place.join("settings/values")).unwrap(), "wlan = true\n", "{} holds it too", place.display());
        }
    }

    #[test]
    fn a_place_the_last_write_could_not_reach_is_caught_up_by_the_next() {
        let dir = tempfile::tempdir().unwrap();
        let places = roots(dir.path());
        // A boot where the better place cannot be written puts the copy in
        // the next one and goes on.
        fs::write(&places[0], "not a folder").unwrap();
        let roots = as_refs(&places);
        let elsewhere = places[1].join("settings/values");
        assert_eq!(write_in(&roots, "settings", "values", "wlan = false\n", None).unwrap(), elsewhere);
        // A boot that can write it reaches both, so the copy the read would
        // otherwise trust is no longer the older of the two.
        fs::remove_file(&places[0]).unwrap();
        write_in(&roots, "settings", "values", "wlan = true\n", Some(&elsewhere)).unwrap();
        for place in &places {
            assert_eq!(fs::read_to_string(place.join("settings/values")).unwrap(), "wlan = true\n", "{} has it", place.display());
        }
    }
}
