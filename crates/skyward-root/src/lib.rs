//! The root methods Skyward can put on a phone: FolkPatch's APatch, the
//! KernelSU family, and the kernels a device's own configuration adds.

pub mod backup;
pub mod folkpatch;
pub mod kernel;
pub mod ksu;

use skyward_vab::Slot;

/// The partition patched, and the only one written. `init_boot`,
/// `vendor_boot`, `recovery`, the boot-control partitions and super are left
/// as they are.
pub const BOOT: &str = "boot";

/// The slots `slots` names, each once, in the order they were named.
pub fn slots(slots: &[Slot]) -> anyhow::Result<Vec<Slot>> {
    let mut kept: Vec<Slot> = Vec::with_capacity(slots.len());
    for slot in slots {
        if !kept.contains(slot) {
            kept.push(*slot);
        }
    }
    anyhow::ensure!(!kept.is_empty(), "no slot to patch");
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_named_twice_is_patched_once() {
        assert_eq!(slots(&[Slot::A, Slot::B, Slot::A]).unwrap(), vec![Slot::A, Slot::B]);
        assert_eq!(slots(&[Slot::B]).unwrap(), vec![Slot::B]);
    }

    #[test]
    fn no_slot_named_is_no_run_at_all() {
        assert!(slots(&[]).is_err());
    }
}
