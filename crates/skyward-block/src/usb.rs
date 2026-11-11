//! USB drives plugged into the phone's port: found among the kernel's
//! disks by the USB bus they hang from, and shown read only for the files
//! on them to be read.

use crate::disk::{self, Access, Disk};
use anyhow::Result;
use std::path::Path;

/// The USB drives plugged in that hold anything, by the names of their
/// disks. A drive pulled out while it is looked at is left out.
pub fn drives() -> Result<Vec<Disk>> {
    drives_in(Path::new(disk::SYS_BLOCK))
}

fn drives_in(listed: &Path) -> Result<Vec<Disk>> {
    disk::disks_in(listed, on_usb)
}

/// Whether a disk's device hangs from a USB bus, `usb1` and the like, as a
/// drive plugged into the port does; the phone's own storage hangs from
/// its UFS controller.
fn on_usb(device: &Path) -> bool {
    device.components().any(|part| match part {
        std::path::Component::Normal(name) => name
            .to_str()
            .and_then(|n| n.strip_prefix("usb"))
            .is_some_and(|bus| !bus.is_empty() && bus.bytes().all(|b| b.is_ascii_digit())),
        _ => false,
    })
}

/// Show `drive` read only at `at`: the first of its partitions, or its
/// disk, that holds a filesystem the kernel reads. Gives the partition and
/// the filesystem shown.
pub fn mount(drive: &Disk, at: &Path) -> Result<(String, &'static str)> {
    disk::mount(drive, at, Access::ReadOnly)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::tests::{disk, root};
    use std::fs;

    #[test]
    fn only_disks_on_a_usb_bus_that_hold_something_are_drives() {
        let root = root("usb");
        disk(&root, "platform/soc/1d84000.ufshc/host0/target0:0:0/0:0:0:0/block/sda", 61_440_000, &[("sda1", 8), ("sda2", 2_048)]);
        disk(&root, "virtual/block/loop0", 2_048, &[]);
        let usb = "platform/soc/a600000.ssusb/a600000.dwc3/xhci-hcd.0.auto/usb1";
        disk(&root, &format!("{usb}/1-1/1-1:1.0/host1/target1:0:0/1:0:0:0/block/sdg"), 60_437_492, &[("sdg2", 60_000_000), ("sdg1", 409_600)]);
        disk(&root, &format!("{usb}/1-2/1-2:1.0/host2/target2:0:0/2:0:0:0/block/sdh"), 0, &[]);
        let found = drives_in(&root.join("block")).unwrap();
        let sdg = Disk { disk: "sdg".into(), parts: vec![("sdg1".into(), 409_600 * 512), ("sdg2".into(), 60_000_000 * 512)] };
        assert_eq!(found, [sdg], "the phone's storage, a loop device and an empty card reader are left out");
        fs::remove_dir_all(&root).unwrap();
    }
}
