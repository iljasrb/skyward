//! Writing a downloaded recovery image over the recovery partitions, the way
//! an install puts this one back: the same flashing code, which reads every
//! partition back and compares what it reads with what it wrote.

use anyhow::{Context, Result};
use skyward_install::image;
use skyward_vab::Slot;
use std::fs::File;
use std::path::{Path, PathBuf};

/// The partition every boot starts the recovery from.
const PARTITION: &str = "recovery";

/// Where the partitions are named, under the phone's by-name links.
const AT: &str = skyward_block::BY_NAME;

/// The recovery partitions to write: the slot the phone runs now, which is
/// the one it boots the recovery from, and the other slot's when it holds
/// this same recovery already — after a slot switch that one is what boots,
/// and an older build there would come back with it.
pub fn slots(active: Slot) -> Result<Vec<Slot>> {
    let skyward = |slot: Slot| -> Result<bool> {
        let path = image::device(Path::new(AT), PARTITION, slot);
        let mut file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        image::is_skyward(&mut file).with_context(|| format!("look into {}", path.display()))
    };
    let mut wanted = vec![active];
    if skyward(active.other())? {
        wanted.push(active.other());
    }
    Ok(wanted)
}

/// Write the image at `path` over the recovery partitions of `slots`, telling
/// `progress` how many bytes of the whole are done.
pub fn install(path: &Path, slots: &[Slot], progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let length = file.metadata().with_context(|| format!("look at {}", path.display()))?.len();
    let devices: Vec<PathBuf> = slots.iter().map(|&slot| image::device(Path::new(AT), PARTITION, slot)).collect();
    image::flash(&mut file, length, &devices, progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_running_slot_is_always_written() {
        // Without a phone there are no partitions to look into; what is
        // checked here is only which slots a phone that cannot be read gets.
        assert_eq!(PARTITION, "recovery");
        assert_eq!(image::device(Path::new(AT), PARTITION, Slot::B), Path::new(AT).join("recovery_b"));
    }
}
