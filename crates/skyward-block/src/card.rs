//! The card in the phone's microSD tray: found among the kernel's disks by
//! the controller the tray hangs from, and shown read and written, for files
//! to be installed from it, backed up to it, and saved to.

use crate::disk::{self, Access, Disk};
use anyhow::Result;
use std::path::Path;

/// The cards in the tray that hold anything, by the names of their disks.
/// A controller with no card in the tray lists no disk, and a card taken out
/// while it is looked at is left out.
pub fn cards(host: &str) -> Result<Vec<Disk>> {
    cards_in(Path::new(disk::SYS_BLOCK), host)
}

fn cards_in(listed: &Path, host: &str) -> Result<Vec<Disk>> {
    disk::disks_in(listed, |device| disk::part_of(device, host))
}

/// Show `card` at `at`, read and written: the first of its partitions, or its
/// disk, that holds a filesystem the kernel reads. Gives the partition and
/// the filesystem shown.
pub fn mount(card: &Disk, at: &Path) -> Result<(String, &'static str)> {
    disk::mount(card, at, Access::ReadWrite)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::tests::{disk, root};
    use std::fs;

    #[test]
    fn only_the_trays_controller_gives_a_card() {
        let root = root("card");
        let tray = "platform/soc/8804000.sdhci/mmc_host/mmc0/mmc0:0001/block/mmcblk0";
        disk(&root, tray, 124_735_488, &[("mmcblk0p1", 124_731_392), ("mmcblk0p2", 4_096)]);
        // The phone's own storage, another card, and a controller whose name
        // only starts with the tray's are none of them the tray's.
        disk(&root, "platform/soc/1d84000.mmc/mmc_host/mmc1/mmc1:0001/block/mmcblk1", 1_024_000, &[]);
        disk(&root, "platform/soc/8804000.sdhci2/mmc_host/mmc2/mmc2:0001/block/mmcblk2", 2_000_000, &[]);
        let found = cards_in(&root.join("block"), "8804000.sdhci").unwrap();
        let card = Disk { disk: "mmcblk0".into(), parts: vec![("mmcblk0p1".into(), 124_731_392 * 512), ("mmcblk0p2".into(), 4_096 * 512)] };
        assert_eq!(found, [card], "a name is taken whole: 8804000.sdhci2 is another controller");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_controller_with_no_card_in_the_tray_lists_no_card() {
        let root = root("card-empty");
        fs::create_dir_all(root.join("block")).unwrap();
        assert_eq!(cards_in(&root.join("block"), "8804000.sdhci").unwrap(), []);
        fs::remove_dir_all(&root).unwrap();
    }
}
