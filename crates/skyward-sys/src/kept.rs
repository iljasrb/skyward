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
//! the one read, and it is written back where it is when that place takes it,
//! so a boot where the best place was not mounted does not leave two copies
//! that disagree for good.
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

/// The places, in the order they are tried.
pub fn places() -> impl Iterator<Item = &'static str> {
    PLACES.into_iter()
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
    roots
        .iter()
        .filter_map(|root| {
            let path = root.join(folder).join(name);
            let written = fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            let text = fs::read_to_string(&path).ok()?;
            Some((written, path, text))
        })
        .max_by_key(|(written, ..)| *written)
        .map(|(_, path, text)| (path, text))
}

/// Write `text` as `folder/name`, taking the place `was` is in when it takes
/// it and the first that does otherwise; the file it wrote is returned.
pub fn write(folder: &str, name: &str, text: &str, was: Option<&Path>) -> Result<PathBuf> {
    write_in(&PLACES.map(Path::new), folder, name, text, was)
}

/// As [`write`], among `roots` given in the order they are tried.
pub fn write_in(roots: &[&Path], folder: &str, name: &str, text: &str, was: Option<&Path>) -> Result<PathBuf> {
    let mut places: Vec<PathBuf> = was.map(Path::to_path_buf).into_iter().collect();
    places.extend(roots.iter().map(|root| root.join(folder).join(name)));
    let mut last: Option<anyhow::Error> = None;
    for path in places {
        let Some(dir) = path.parent() else { continue };
        if fs::create_dir_all(dir).is_err() {
            continue;
        }
        match swap(&path, text) {
            Ok(()) => return Ok(path),
            Err(e) => last = Some(e.context(format!("write {}", path.display()))),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("nowhere to keep {folder}/{name}")))
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
        std::thread::sleep(std::time::Duration::from_millis(20));
        // The better place takes one later: it is the newer, so it is read.
        write_in(&roots, "settings", "values", "wlan = true\n", None).unwrap();
        let (path, text) = read_in(&roots, "settings", "values").expect("a file");
        assert_eq!(path, places[0].join("settings/values"));
        assert_eq!(text, "wlan = true\n");
    }

    #[test]
    fn a_file_goes_back_where_it_came_from() {
        let dir = tempfile::tempdir().unwrap();
        let places = roots(dir.path());
        let roots = as_refs(&places);
        let path = write_in(&roots, "settings", "values", "wlan = true\n", None).unwrap();
        assert_eq!(path, places[0].join("settings/values"), "the best place takes it");
        // A boot where that place cannot be written keeps the copy where it
        // was, rather than making a second one that disagrees.
        let elsewhere = places[1].join("settings/values");
        let again = write_in(&roots, "settings", "values", "wlan = false\n", Some(&elsewhere)).unwrap();
        assert_eq!(again, elsewhere);
        assert_eq!(fs::read_to_string(places[0].join("settings/values")).unwrap(), "wlan = true\n");
    }
}
