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
//! The first that both takes a file and reads it back is the one used, so the
//! settings survive a restart even when the user unlocks nothing.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

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

/// Read `file` in `folder` where any place holds it, best first.
pub fn read(folder: &str, file: &str) -> Result<Option<String>> {
    for root in PLACES {
        let path = self::folder(root, folder).join(file);
        if path.is_file() {
            return std::fs::read_to_string(&path).map(Some).with_context(|| format!("read {}", path.display()));
        }
    }
    Ok(None)
}

/// Write `text` as `file` in `folder`, in the first place that takes it, and
/// say which file that was.
pub fn write(folder: &str, file: &str, text: &str) -> Result<PathBuf> {
    let mut last = None;
    for root in PLACES {
        let dir = self::folder(root, folder);
        let Ok(()) = std::fs::create_dir_all(&dir) else { continue };
        let path = dir.join(file);
        match std::fs::write(&path, text) {
            Ok(()) => return Ok(path),
            Err(e) => last = Some(anyhow::Error::new(e).context(format!("write {}", path.display()))),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("nowhere to keep {folder}/{file}")))
}
