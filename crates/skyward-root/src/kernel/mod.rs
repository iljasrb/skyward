//! A kernel published for this phone, taken whole from its releases: its own
//! module and the ramdisk the release carries, written to the boot partition.
//!
//! The release is a complete Android boot image. Nothing of it is run as a
//! script: its SHA-256 is checked, the image is looked into before it is
//! written, and the partition from before the write is kept whole.

pub mod release;
mod install;

pub use install::{install, Failure, Installed, Options, Outcome, Refusal};

use skyward_vab::Slot;

/// A root method a device adds, from a kernel release of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plugin {
    /// The name the root page shows, such as `Shinobu kernel`.
    pub name: String,
    /// The line under it, where the device's configuration has one.
    pub subtitle: Option<String>,
    /// The GitHub repository the releases are published in.
    pub repo: String,
    /// The boot image's name in a release, with `{version}` standing for the
    /// project version in the repository's `sources.env`.
    pub asset: String,
    /// The folder under `Download/Skyward` the run's backups are kept in.
    pub folder: String,
    /// What the page says after the image is written, where the kernel
    /// carries its own manager.
    pub note: Option<String>,
    /// The manager's name, as the page shows it.
    pub manager_name: Option<String>,
}

/// What the newest release says about the kernel and the manager it carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Details {
    pub tag: String,
    pub kernel: Option<String>,
    pub manager: Option<String>,
}

/// How far a run has come, as the screen says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Asking GitHub for the newest release.
    Finding,
    /// Downloading the kernel image.
    Downloading { done: u64, total: u64 },
    /// Downloading the manager that goes with the kernel.
    DownloadingManager { done: u64, total: u64 },
    /// Reading the image before anything is written.
    Checking,
    /// Saving a slot's boot partition as it was.
    BackingUp { slot: Slot, done: u64, total: u64 },
    /// Writing the image to a slot's boot partition and reading it back.
    Flashing { slot: Slot, done: u64, total: u64 },
    /// Keeping the manifest beside the copies.
    Saving,
}
