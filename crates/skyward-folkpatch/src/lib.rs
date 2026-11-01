//! Installing FolkPatch's root onto the boot partition: the KernelPatch tools
//! it is patched with, the manager that opens it, and the SuperKey that is the
//! manager's to ask for.
//!
//! The manager's own recovery package is never run and no script inside it is
//! read: the pinned `kptools` is started here, and the image it makes is
//! written by this recovery. The key is made here or typed on the screen, kept
//! out of command lines and logs, and saved only where root can read it.

pub mod backup;
pub mod install;
pub mod key;

pub use install::install;
pub use key::{KeyFault, SuperKey};

use skyward_vab::Slot;
use std::fmt;
use std::path::PathBuf;

/// The partition patched, and the only one written. `init_boot`,
/// `vendor_boot`, `recovery`, the boot-control partitions and super are left
/// as they are.
pub const BOOT: &str = "boot";

/// The manager that opens what is patched in, and the release it comes from.
pub const MANAGER_PACKAGE: &str = "me.yuki.folk";
pub const MANAGER_VERSION: &str = "115404";

/// The KernelPatch release the tools are of.
pub const KPIMG_VERSION: &str = "0.13.9";

// The tools and the manager themselves, which the build fetched from their
// release and checked against the hash it pins; `MANAGER_FILE` names the
// package among them. Nothing is taken from the tree, so a clone carries no
// seventeen megabytes of them and an offline build uses what the cache has.
include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// What a run is asked to do besides the phone it is on.
#[derive(Clone, Debug)]
pub struct Options {
    /// The phone's own name, as its configuration names it.
    pub device: String,
    /// The slot the phone started from.
    pub current: Slot,
    /// The slots to patch, each once.
    pub slots: Vec<Slot>,
    /// The charge the recovery read, which a run will not write below.
    pub battery: skyward_info::Battery,
    /// The key to make the patch with.
    pub key: Key,
    /// Where the phone's partitions are linked, as `/dev/block/by-name`.
    pub partition_dir: PathBuf,
    /// The owner's storage, as `/sdcard`, which the manager is saved in.
    pub storage: PathBuf,
    /// Where only root reads what the run keeps, as
    /// `/data/local/tmp/skyward-folkpatch`.
    pub private: PathBuf,
    /// When the run was started, as `20261101-123456`.
    pub stamp: String,
}

/// Where the key a patch is made with comes from.
#[derive(Clone)]
pub enum Key {
    /// Made here, from `/dev/urandom`.
    Generate,
    /// Typed on the screen, and already validated.
    Enter(SuperKey),
}

/// The key is root-equivalent: a run says where it went, never what it is.
impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Generate => f.write_str("Key::Generate"),
            Key::Enter(key) => f.debug_tuple("Key::Enter").field(key).finish(),
        }
    }
}

/// How far a run has come, as the screen says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Reading the phone and the images before anything is written.
    Checking,
    /// Saving the manager in the owner's storage.
    Copying { done: u64, total: u64 },
    /// Saving the boot partition of a slot as it was.
    BackingUp { slot: Slot, done: u64, total: u64 },
    /// Patching a slot's image with `kptools`.
    Patching { slot: Slot },
    /// Reading the patch back before it is written.
    Verifying { slot: Slot },
    /// Writing a patched image and reading it back.
    Flashing { slot: Slot, done: u64, total: u64 },
}

/// How a run ended.
#[derive(Debug)]
pub enum Outcome {
    Installed(Installed),
    /// Nothing was written to a boot partition, for this reason.
    Refused(Refusal),
    Failed(Failure),
}

/// A run that patched every slot it was asked to.
#[derive(Debug)]
pub struct Installed {
    /// The slots patched, each once.
    pub slots: Vec<Slot>,
    /// The key the patch was made with, for the screen to show.
    pub key: SuperKey,
    /// Where only root reads the key.
    pub key_path: PathBuf,
    /// The manager, saved in the owner's storage.
    pub apk_path: PathBuf,
    /// Where the copies of the boot partitions are.
    pub backup_dir: PathBuf,
    /// Whether the kernel was built with `CONFIG_KALLSYMS_ALL`, which some
    /// modules need and FolkPatch runs without.
    pub kallsyms_all: bool,
}

/// A run that stopped with something to say, and what it had already written.
#[derive(Debug)]
pub struct Failure {
    /// The stage the run was in.
    pub stage: Stage,
    /// The slot being worked on, where one was.
    pub slot: Option<Slot>,
    /// Why it stopped, and what it was doing it to.
    pub why: String,
    /// The copies of the boot partitions made before it stopped.
    pub backups: Vec<backup::Backup>,
    /// The slots that may have been written, wholly or partly.
    pub written: Vec<Slot>,
    /// Where the key was saved, where one had been made already.
    pub key_path: Option<PathBuf>,
}

/// Why a run did not start, or stopped before writing a boot partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The phone has one slot, or no `boot` in the slots it was asked for.
    NoSlot,
    /// A slot the phone's table does not name.
    Missing(Slot),
    /// A phone that is not aarch64, whose boot images these tools cannot patch.
    NotAarch64,
    /// Too little charge to write a boot partition safely.
    Battery { level: u8, charging: bool },
    /// A boot partition that is not an Android boot image with a kernel in it.
    NotBootImage(Slot),
    /// A boot partition holding a recovery's ramdisk rather than the system's.
    RecoveryRamdisk(Slot),
    /// A boot image KernelPatch says is patched already.
    AlreadyPatched(Slot),
    /// A kernel built without `CONFIG_KALLSYMS`, which KernelPatch needs.
    NoKallsyms,
    /// A library or interpreter `kptools` needs that this recovery has not.
    NoTools(String),
    /// The owner's storage is closed, so the manager cannot be saved.
    StorageClosed,
    /// Not enough room for the manager, every backup, and a margin.
    NoRoom { need: u64, have: u64 },
    /// A key the manager would not take.
    Key(KeyFault),
}

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

    #[test]
    fn the_files_the_crate_carries_are_the_pinned_release() {
        assert_eq!(KPTOOLS.len(), 249_040);
        assert_eq!(KPIMG.len(), 430_816);
        assert_eq!(MANAGER.len(), 17_378_611);
        assert!(MANAGER.starts_with(b"PK"), "the manager is its package");
        assert!(MANAGER_FILE.ends_with(".apk"));
    }

    #[test]
    fn a_key_is_never_written_into_a_debug_line() {
        let key = SuperKey::validate("secret123").unwrap();
        let line = format!("{:?} {:?}", Key::Enter(key.clone()), key);
        assert!(!line.contains("secret123"), "{line}");
        assert!(line.contains("SuperKey"), "{line}");
    }
}
