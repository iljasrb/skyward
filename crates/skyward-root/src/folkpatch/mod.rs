//! FolkPatch's APatch: the KernelPatch tools it is patched with, and the
//! SuperKey that opens what is patched in.
//!
//! Only the patch is made here. The manager that uses it is the owner's to
//! install in Android, from its own release; nothing of it is carried, saved
//! or run. The pinned `kptools` is started here, and the image it makes is
//! written by this recovery. The key is made here or typed on the screen, kept
//! out of command lines and logs, and saved only where root can read it.

pub mod install;
pub mod key;

pub use install::{install, patched_slot};
pub use key::{KeyFault, SuperKey};

use crate::backup;
use anyhow::{Context, Result};
use skyward_vab::Slot;
use std::fmt;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The KernelPatch release the tools are of.
pub const KPIMG_VERSION: &str = "0.13.9";

// The tools themselves, taken from the FolkPatch release and checked against
// the hashes pinned here. They are carried rather than fetched, so a build
// needs no network and every clone builds the same bytes.
pub const KPTOOLS: &[u8] = include_bytes!("../../assets/kptools");
pub const KPTOOLS_SHA256: &str = "ea06a9a46dc1f210c71a61a9f07556fa9b739c327847c7ab16ccbb46fdb7631c";
pub const KPIMG: &[u8] = include_bytes!("../../assets/kpimg");
pub const KPIMG_SHA256: &str = "7a7aa4b930d1a9d684cbb426a3999f75cb2e2cead26673dcbbc5f70871f01141";

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
    /// Patch an image that carries the patch already, which the user asked for
    /// with a swipe of their own.
    pub repatch: bool,
    /// Where the phone's partitions are linked, as `/dev/block/by-name`.
    pub partition_dir: PathBuf,
    /// The owner's storage, as `/sdcard`, which the backups are saved in.
    pub storage: PathBuf,
    /// Where only root reads what the run keeps, as
    /// `/data/local/tmp/skyward-root`.
    pub private: PathBuf,
    /// When the run was started, as `20261101-123456`.
    pub stamp: String,
}

/// Where the key a patch is made with comes from.
#[derive(Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Installed(Installed),
    /// Nothing was written to a boot partition, for this reason.
    Refused(Refusal),
    Failed(Failure),
}

/// A run that patched every slot it was asked to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The slots patched, each once.
    pub slots: Vec<Slot>,
    /// The key the patch was made with, for the screen to show.
    pub key: SuperKey,
    /// Where only root reads the key.
    pub key_path: PathBuf,
    /// Where the copies of the boot partitions are.
    pub backup_dir: PathBuf,
    /// Whether the kernel was built with `CONFIG_KALLSYMS_ALL`, which some
    /// modules need and FolkPatch runs without.
    pub kallsyms_all: bool,
}

/// A run that stopped with something to say, and what it had already written.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// The owner's storage is closed, so the backups cannot be saved.
    StorageClosed,
    /// Not enough room for every backup, and a margin.
    NoRoom { need: u64, have: u64 },
    /// A key `kptools` would not take.
    Key(KeyFault),
}

/// The key of the most recent run whose key is still on this phone, from the
/// folders only root reads and from copies the user chose to keep beside the
/// the owner's storage.
///
/// A stock update replaces the boot image, and with it the hash of the key, so
/// this is what lets a phone patched again after one keep the key it had.
pub fn last_key(private: &Path, storage: &Path) -> Option<SuperKey> {
    let runs = [private.to_path_buf(), storage.join("Download").join("Skyward").join("FolkPatch")];
    let mut newest: Option<(SystemTime, SuperKey)> = None;
    for root in runs {
        let Ok(entries) = fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let path = entry.path().join("superkey.txt");
            let Some(written) = fs::metadata(&path).and_then(|m| m.modified()).ok() else { continue };
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let Ok(key) = SuperKey::validate(text.trim()) else { continue };
            // Of two written in the same second the copy in the owner's storage is
            // the last of them, the folders being scanned in that order.
            if newest.as_ref().is_none_or(|(at, _)| written >= *at) {
                newest = Some((written, key));
            }
        }
    }
    newest.map(|(_, key)| key)
}

/// Keep `key` in the folder of the run that made it, in the owner's storage, where
/// the user can read it in Android — and where any app that can read internal
/// storage can read it too, which is the whole reason it is not kept there by
/// itself.
pub fn share_key(run: &Path, key: &SuperKey) -> Result<PathBuf> {
    let path = run.join("superkey.txt");
    fs::write(&path, key.expose()).with_context(|| format!("write {}", path.display()))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).with_context(|| format!("open {} to reading", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_files_the_crate_carries_are_the_pinned_release() {
        assert_eq!(KPTOOLS.len(), 249_040);
        assert_eq!(KPIMG.len(), 430_816);
    }

    #[test]
    fn the_key_of_the_newest_run_is_the_one_offered_again() {
        let dir = tempfile::tempdir().unwrap();
        let (private, storage) = (dir.path().join("private"), dir.path().join("storage"));
        let run = |root: &Path, stamp: &str, key: &str| {
            let at = root.join(stamp);
            fs::create_dir_all(&at).unwrap();
            fs::write(at.join("superkey.txt"), key).unwrap();
            at
        };
        assert_eq!(last_key(&private, &storage), None, "no run yet");
        run(&private, "20261101-120000", "older123");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let newer = run(&private, "20261101-130000", "newer456");
        assert_eq!(last_key(&private, &storage).unwrap().expose(), "newer456");
        // A copy the user kept in their own storage comes later still.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let shared = run(&storage.join("Download/Skyward/FolkPatch"), "20261101-140000", "shared789");
        assert_eq!(last_key(&private, &storage).unwrap().expose(), "shared789");
        // A file that is no key at all is passed over.
        fs::write(newer.join("superkey.txt"), "taken").unwrap();
        assert_eq!(last_key(&private, &storage).unwrap().expose(), "shared789");
        let _ = shared;
    }

    #[test]
    fn a_key_the_user_keeps_in_their_storage_can_be_read_by_android() {
        let dir = tempfile::tempdir().unwrap();
        let key = SuperKey::validate("abcd1234").unwrap();
        let path = share_key(dir.path(), &key).unwrap();
        assert_eq!(path, dir.path().join("superkey.txt"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "abcd1234");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o644);
    }

    #[test]
    fn a_key_is_never_written_into_a_debug_line() {
        let key = SuperKey::validate("secret123").unwrap();
        let line = format!("{:?} {:?}", Key::Enter(key.clone()), key);
        assert!(!line.contains("secret123"), "{line}");
        assert!(line.contains("SuperKey"), "{line}");
    }
}
