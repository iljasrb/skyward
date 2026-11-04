//! KernelSU-family root: a manager's own `ksud` patches the boot image, and
//! the same manager installs in Android to open what it patched.
//!
//! The family shares one way of patching; the managers differ in where their
//! release is published and what it is called. Nothing of a manager is run as
//! a script: its APK is kept as it is, and the `ksud` inside it is started
//! directly. The image it makes is looked into and read back before this
//! recovery trusts it, and the partition from before the write is kept whole.

pub mod apk;
pub mod installed;
pub mod release;
mod run;

pub use installed::Installed as InstalledManager;
pub use release::{Asset, Release};
pub use run::install;

use crate::backup;

use skyward_info::Battery;
use skyward_vab::Slot;
use std::path::PathBuf;

/// A manager of the KernelSU family, whose `ksud` patches the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    KernelSu,
    KernelSuNext,
    SukiSu,
    BakaSu,
}

impl Flavor {
    pub const ALL: [Flavor; 4] = [Flavor::KernelSu, Flavor::KernelSuNext, Flavor::SukiSu, Flavor::BakaSu];

    /// The GitHub repository the manager publishes its releases in.
    pub fn repo(self) -> &'static str {
        match self {
            Flavor::KernelSu => "tiann/KernelSU",
            Flavor::KernelSuNext => "KernelSU-Next/KernelSU-Next",
            Flavor::SukiSu => "SukiSU-Ultra/SukiSU-Ultra",
            Flavor::BakaSu => "Baka-SU/BakaSU",
        }
    }

    /// What the manager is called, the same in every language.
    pub fn name(self) -> &'static str {
        match self {
            Flavor::KernelSu => "KernelSU",
            Flavor::KernelSuNext => "KernelSU Next",
            Flavor::SukiSu => "SukiSU Ultra",
            Flavor::BakaSu => "BakaSU",
        }
    }

    /// The name its downloads and manifest lines use.
    pub fn id(self) -> &'static str {
        match self {
            Flavor::KernelSu => "kernelsu",
            Flavor::KernelSuNext => "kernelsu-next",
            Flavor::SukiSu => "sukisu",
            Flavor::BakaSu => "bakasu",
        }
    }

    /// The package names the manager installs under, the current one first.
    pub fn packages(self) -> &'static [&'static str] {
        match self {
            Flavor::KernelSu => &["me.weishu.kernelsu"],
            Flavor::KernelSuNext => &["com.rifsxd.ksunext", "com.kernelsu.next"],
            Flavor::SukiSu => &["com.sukisu.ultra", "io.github.sukisu.ultra"],
            Flavor::BakaSu => &["com.resukisu.resukisu", "com.baka.su"],
        }
    }

    /// The manager a package name belongs to.
    pub fn of_package(package: &str) -> Option<Flavor> {
        Flavor::ALL.into_iter().find(|flavor| flavor.packages().contains(&package))
    }
}

/// Which of a manager's releases is taken: its newest stable one, or its
/// newest one even while it is still a prerelease.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Stable,
    Latest,
}

/// Where the manager's APK comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Its release on GitHub, downloaded and checked against its SHA-256.
    Download,
    /// The manager installed in Android already.
    Installed,
}

/// How far a run has come, as the screen says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Reading the phone and the image before anything is written.
    Checking,
    /// Asking the manager's releases which one to take.
    Finding,
    /// Downloading the manager's APK.
    Downloading { done: u64, total: u64 },
    /// Taking `ksud` out of the manager's APK.
    Extracting,
    /// Saving the partition of a slot as it was.
    BackingUp { slot: Slot, partition: &'static str, done: u64, total: u64 },
    /// Patching a slot's image with `ksud`.
    Patching { slot: Slot, partition: &'static str },
    /// Reading the patch back before it is written.
    Verifying { slot: Slot, partition: &'static str },
    /// Writing a patched image and reading it back.
    Flashing { slot: Slot, partition: &'static str, done: u64, total: u64 },
    /// Keeping a copy of the manager beside the backups.
    SavingManager,
}

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
    pub battery: Battery,
    /// The manager whose `ksud` patches the image, and the release it is of.
    pub flavor: Flavor,
    pub tag: String,
    /// The KMI whose module `ksud` embeds, where the kernel's own version
    /// names another one.
    pub kmi: Option<String>,
    /// The manager's APK, downloaded and checked, or the one installed in
    /// Android.
    pub apk: PathBuf,
    /// The SHA-256 the APK must have, where it came from a release.
    pub apk_sha256: Option<String>,
    /// Where the phone's partitions are linked, as `/dev/block/by-name`.
    pub partition_dir: PathBuf,
    /// The owner's storage, as `/sdcard`, which the backups are saved in.
    pub storage: PathBuf,
    /// Where only root reads what the run keeps, as
    /// `/data/local/tmp/skyward-ksu`.
    pub private: PathBuf,
    /// When the run was started, as `20261101-123456`.
    pub stamp: String,
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
    /// The copies of the partitions made before it stopped.
    pub backups: Vec<backup::Backup>,
    /// The slots that may have been written, wholly or partly.
    pub written: Vec<Slot>,
    /// The run's folder, where it has one.
    pub run_dir: Option<PathBuf>,
}

/// Why a run did not start, or stopped before writing a partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The phone has one slot, or no slot in the ones it was asked for.
    NoSlot,
    /// A slot whose partition the phone's table does not name.
    Missing { slot: Slot, partition: &'static str },
    /// A phone that is not aarch64, whose `ksud` cannot run.
    NotAarch64,
    /// Too little charge to write a boot partition safely.
    Battery { level: u8, charging: bool },
    /// A partition that is not an Android boot image.
    NotBootImage { slot: Slot, partition: &'static str },
    /// A boot partition holding a recovery's ramdisk rather than the system's.
    RecoveryRamdisk { slot: Slot, partition: &'static str },
    /// The manager's APK carries no `ksud` this recovery can run, or it
    /// would not start.
    NoKsud(String),
    /// The manager's APK does not have the SHA-256 its release published.
    Damaged,
    /// The owner's storage is closed, so the copies cannot be saved.
    StorageClosed,
    /// Not enough room for the copies, the manager and a margin.
    NoRoom { need: u64, have: u64 },
    /// The manager has no release of the tier asked for. BakaSU, for one,
    /// publishes only prereleases.
    NoRelease { flavor: Flavor, tier: Tier },
    /// The manager was to come from Android, where none is installed.
    NoManager,
}

/// A run that patched every slot it was asked to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The slots patched, each once.
    pub slots: Vec<Slot>,
    /// The partition each slot's patch went to, in the same order.
    pub partitions: Vec<&'static str>,
    pub flavor: Flavor,
    /// The manager's release, or the package name where it came from Android.
    pub tag: String,
    /// The manager's APK, kept in the run's folder for Android to install.
    pub apk: PathBuf,
    pub apk_sha256: String,
    pub ksud_sha256: String,
    /// Where the copies of the partitions are.
    pub backup_dir: PathBuf,
    /// Where the copies and the manager are kept.
    pub run_dir: PathBuf,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Installed(Installed),
    /// Nothing was written to a boot partition, for this reason.
    Refused(Refusal),
    Failed(Failure),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_manager_is_found_by_the_package_it_installs_under() {
        for flavor in Flavor::ALL {
            for package in flavor.packages() {
                assert_eq!(Flavor::of_package(package), Some(flavor));
            }
        }
        assert_eq!(Flavor::of_package("com.example.other"), None);
    }

    #[test]
    fn the_managers_are_asked_in_their_own_repositories() {
        let repos: Vec<&str> = Flavor::ALL.iter().map(|f| f.repo()).collect();
        assert_eq!(
            repos,
            ["tiann/KernelSU", "KernelSU-Next/KernelSU-Next", "SukiSU-Ultra/SukiSU-Ultra", "Baka-SU/BakaSU"]
        );
    }
}
