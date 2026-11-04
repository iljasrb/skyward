//! Writing a complete kernel boot image to the boot partitions: the image is
//! checked, the partitions from before the write are kept whole, and each
//! write is read back before the run says it is done.

use crate::backup::{self, Backup, Layout};
use crate::kernel::{Plugin, Stage};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use skyward_install::image::{self, ImageKind, Ramdisk};
use skyward_vab::Slot;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// A boot image's header is at most this long before its kernel starts.
const HEADER: usize = 4096;
/// Room left on the owner's storage after the copies, so the manifest can be
/// written beside them.
const MARGIN: u64 = 8 << 20;
const PARTITION: &str = "boot";

/// What a run is asked to do besides the phone it is on.
#[derive(Clone, Debug)]
pub struct Options {
    /// The phone's own name, as its configuration names it.
    pub device: String,
    /// The slot the phone started from.
    pub current: Slot,
    /// The slots to write, each once.
    pub slots: Vec<Slot>,
    /// The charge the recovery read, which a run will not write below.
    pub battery: skyward_info::Battery,
    /// The device's own kernel plugin.
    pub plugin: Plugin,
    /// The release the image came from.
    pub tag: String,
    /// The image's name in the release.
    pub asset: String,
    /// The downloaded image, checked against `sha256`.
    pub image: PathBuf,
    pub sha256: String,
    /// The manager's APK, downloaded with the image, where the release has
    /// one.
    pub manager: Option<PathBuf>,
    /// Why the manager was not downloaded, where it was not.
    pub manager_note: Option<String>,
    /// Where the phone's partitions are linked, as `/dev/block/by-name`.
    pub partition_dir: PathBuf,
    /// The owner's storage, as `/sdcard`, which the backups are saved in.
    pub storage: PathBuf,
    /// When the run was started, as `20261101-123456`.
    pub stamp: String,
}

/// A run that wrote a kernel to every slot it was asked to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub slots: Vec<Slot>,
    pub tag: String,
    /// The image's name in the release.
    pub image: String,
    pub sha256: String,
    /// Where the copies of the boot partitions are.
    pub backup_dir: PathBuf,
    /// Where the copies and the manifest are kept.
    pub run_dir: PathBuf,
    /// The manager's APK, kept for Android to install, where the release has
    /// one.
    pub manager: Option<PathBuf>,
    /// Why the manager was not kept, where it was not.
    pub manager_note: Option<String>,
}

/// A run that stopped with something to say, and what it had already written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub stage: Stage,
    pub slot: Option<Slot>,
    pub why: String,
    pub backups: Vec<Backup>,
    pub written: Vec<Slot>,
    pub run_dir: Option<PathBuf>,
}

/// Why a run did not start, or stopped before writing a partition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    NoSlot,
    Missing { slot: Slot, partition: &'static str },
    Battery { level: u8, charging: bool },
    NotBootImage { slot: Slot, partition: &'static str },
    RecoveryImage { slot: Slot, partition: &'static str },
    TooLarge { slot: Slot, partition: &'static str, image: u64, partition_size: u64 },
    NoRoom { need: u64, have: u64 },
    StorageClosed,
    Damaged,
    NoImage,
    NoRelease,
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed(Installed),
    Refused(Refusal),
    Failed(Failure),
}

enum Stop {
    Refused(Refusal),
    Why(anyhow::Error),
}

#[derive(Clone)]
struct Target {
    slot: Slot,
    device: PathBuf,
}

struct Run<'a> {
    options: &'a Options,
    tell: &'a mut dyn FnMut(Stage),
    stage: Stage,
    slot: Option<Slot>,
    layout: Layout,
    targets: Vec<Target>,
    backups: Vec<Backup>,
    written: Vec<Slot>,
    manager: Option<PathBuf>,
}

/// Install the downloaded kernel image, telling how far it has come.
pub fn install(options: &Options, tell: &mut dyn FnMut(Stage)) -> Outcome {
    let mut run = match Run::new(options, tell) {
        Ok(run) => run,
        Err(Stop::Refused(refusal)) => return Outcome::Refused(refusal),
        Err(Stop::Why(e)) => {
            return Outcome::Failed(Failure {
                stage: Stage::Checking,
                slot: None,
                why: format!("{e:#}"),
                backups: Vec::new(),
                written: Vec::new(),
                run_dir: None,
            })
        }
    };
    match run.go() {
        Ok(installed) => Outcome::Installed(installed),
        Err(stop) => run.stopped(stop),
    }
}

impl<'a> Run<'a> {
    fn new(options: &'a Options, tell: &'a mut dyn FnMut(Stage)) -> Result<Run<'a>, Stop> {
        if !options.storage.is_dir() {
            return Err(Stop::Refused(Refusal::StorageClosed));
        }
        let layout = Layout::make_in(&options.storage, &options.plugin.folder, &options.stamp).map_err(Stop::Why)?;
        Ok(Run {
            options,
            tell,
            stage: Stage::Checking,
            slot: None,
            layout,
            targets: Vec::new(),
            backups: Vec::new(),
            written: Vec::new(),
            manager: None,
        })
    }

    fn go(&mut self) -> Result<Installed, Stop> {
        self.told(Stage::Checking);
        self.preflight()?;
        self.saved()?;
        self.flash()?;
        self.told(Stage::Saving);
        self.write_manifest()?;
        Ok(Installed {
            slots: self.written.clone(),
            tag: self.options.tag.clone(),
            image: self.options.asset.clone(),
            sha256: self.options.sha256.clone(),
            backup_dir: self.layout.backups.clone(),
            run_dir: self.layout.dir.clone(),
            manager: self.manager.clone(),
            manager_note: self.options.manager_note.clone(),
        })
    }

    /// Everything that can stop a run before a partition is written.
    fn preflight(&mut self) -> Result<(), Stop> {
        let slots = crate::slots(&self.options.slots).map_err(|_| Stop::Refused(Refusal::NoSlot))?;
        let image = &self.options.image;
        if !image.is_file() {
            return Err(Stop::Refused(Refusal::NoImage));
        }
        let len = fs::metadata(image).map_err(|e| Stop::Why(e.into()))?.len();
        if hash_file(image).map_err(Stop::Why)? != self.options.sha256.to_ascii_lowercase() {
            return Err(Stop::Refused(Refusal::Damaged));
        }
        let mut file = File::open(image).with_context(|| format!("open {}", image.display())).map_err(Stop::Why)?;
        let mut head = vec![0u8; HEADER];
        let read = file.read(&mut head).map_err(|e| Stop::Why(e.into()))?;
        head.truncate(read);
        match image::detect(&mut file, &head).map_err(Stop::Why)? {
            Some(ImageKind::Boot { kernel: true, ramdisk: Ramdisk::Generic | Ramdisk::None }) => {}
            Some(ImageKind::Boot { ramdisk: Ramdisk::Recovery, .. }) => {
                return Err(Stop::Refused(Refusal::RecoveryImage { slot: slots[0], partition: PARTITION }))
            }
            _ => return Err(Stop::Refused(Refusal::NotBootImage { slot: slots[0], partition: PARTITION })),
        }
        let mut need = MARGIN;
        for slot in slots {
            let device = image::device(&self.options.partition_dir, PARTITION, slot);
            if !device.exists() {
                return Err(Stop::Refused(Refusal::Missing { slot, partition: PARTITION }));
            }
            let size = partition_size(&device).map_err(Stop::Why)?;
            if len > size {
                return Err(Stop::Refused(Refusal::TooLarge { slot, partition: PARTITION, image: len, partition_size: size }));
            }
            need += size;
            self.targets.push(Target { slot, device });
        }
        if let Some(refusal) = skyward_install::battery_refusal(&self.options.battery) {
            return Err(Stop::Refused(match refusal {
                skyward_install::Refusal::Battery { level, charging } => Refusal::Battery { level, charging },
                other => return Err(Stop::Why(anyhow::anyhow!("{other:?}"))),
            }));
        }
        let have = crate::folkpatch::install::available(&self.options.storage).map_err(Stop::Why)?;
        if need > have {
            return Err(Stop::Refused(Refusal::NoRoom { need, have }));
        }
        Ok(())
    }

    /// The copies of the boot partitions, before anything is written.
    fn saved(&mut self) -> Result<(), Stop> {
        for target in self.targets.clone() {
            self.slot = Some(target.slot);
            self.told(Stage::BackingUp { slot: target.slot, done: 0, total: 0 });
            let device = target.device.clone();
            let backups = self.layout.backups.clone();
            let slot = target.slot;
            let mut tell = |done, total| (self.tell)(Stage::BackingUp { slot, done, total });
            let copy = backup::save_with(&device, &backups, slot, PARTITION, &backup::name(slot), &mut tell).map_err(Stop::Why)?;
            self.backups.push(copy);
        }
        if let Some(manager) = &self.options.manager {
            let path = self.layout.dir.join("manager.apk");
            fs::copy(manager, &path)
                .with_context(|| format!("keep {} as {}", manager.display(), path.display()))
                .map_err(Stop::Why)?;
            // The download is kept private to root; Android must be able to
            // read the copy it installs from the owner's storage.
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
                .with_context(|| format!("let Android read {}", path.display()))
                .map_err(Stop::Why)?;
            self.manager = Some(path);
        }
        Ok(())
    }

    /// Write the image to each boot partition and read it back.
    fn flash(&mut self) -> Result<(), Stop> {
        let len = fs::metadata(&self.options.image).map_err(|e| Stop::Why(e.into()))?.len();
        for target in self.targets.clone() {
            self.slot = Some(target.slot);
            self.told(Stage::Flashing { slot: target.slot, done: 0, total: 0 });
            let mut file = File::open(&self.options.image)
                .with_context(|| format!("open {}", self.options.image.display()))
                .map_err(Stop::Why)?;
            let slot = target.slot;
            let device = target.device.clone();
            image::flash(&mut file, len, &[device], &mut |done, total| {
                (self.tell)(Stage::Flashing { slot, done, total })
            })
            .map_err(Stop::Why)?;
            self.written.push(target.slot);
        }
        Ok(())
    }

    /// The manifest beside the copies, saying what the run was and where the
    /// copies are.
    fn write_manifest(&self) -> Result<(), Stop> {
        let mut text = String::new();
        text.push_str(&format!("device = {}\n", self.options.device));
        text.push_str(&format!("current_slot = {}\n", self.options.current.letter()));
        let slots: Vec<&str> = self.written.iter().map(|slot| slot.letter()).collect();
        text.push_str(&format!("slots = {}\n", slots.join(", ")));
        text.push_str(&format!("kernel = {}\n", self.options.plugin.name));
        text.push_str(&format!("release = {}\n", self.options.tag));
        text.push_str(&format!("image = {}\n", self.options.asset));
        text.push_str(&format!("image_sha256 = {}\n", self.options.sha256));
        if self.manager.is_some() {
            text.push_str("manager = manager.apk\n");
        }
        for copy in &self.backups {
            let kept = copy.path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            text.push_str(&format!("backup{} = backup/{kept} {} {}\n", copy.slot.suffix(), copy.size, copy.sha256));
        }
        let path = self.layout.dir.join("manifest.txt");
        fs::write(&path, text).with_context(|| format!("write {}", path.display())).map_err(Stop::Why)?;
        Ok(())
    }

    fn told(&mut self, stage: Stage) {
        self.stage = stage;
        (self.tell)(stage);
    }

    fn stopped(&self, stop: Stop) -> Outcome {
        match stop {
            Stop::Refused(refusal) => {
                if self.backups.is_empty() {
                    let _ = fs::remove_dir_all(&self.layout.dir);
                }
                Outcome::Refused(refusal)
            }
            Stop::Why(e) => {
                let run_dir = if self.backups.is_empty() {
                    let _ = fs::remove_dir_all(&self.layout.dir);
                    None
                } else {
                    Some(self.layout.dir.clone())
                };
                Outcome::Failed(Failure {
                    stage: self.stage,
                    slot: self.slot,
                    why: format!("{e:#}"),
                    backups: self.backups.clone(),
                    written: self.written.clone(),
                    run_dir,
                })
            }
        }
    }
}

fn partition_size(device: &Path) -> Result<u64> {
    let mut file = File::open(device).with_context(|| format!("open {}", device.display()))?;
    file.seek(SeekFrom::End(0)).with_context(|| format!("find the size of {}", device.display()))
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer).with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(backup::hex(&digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use abootimg_oxide::{Header, HeaderV3, OsVersionPatch};
    use skyward_info::Battery;

    fn boot_image(kernel: u32) -> Vec<u8> {
        let header = Header::V3(HeaderV3 {
            kernel_size: kernel,
            ramdisk_size: 0,
            osversionpatch: OsVersionPatch(0),
            cmdline: Box::new([0; 1536]),
            v4_signature_size: Some(0),
        });
        let mut out = Vec::new();
        header.write(&mut out).unwrap();
        out.resize(4096, 0);
        out.resize(4096 + (kernel as usize).div_ceil(4096) * 4096, 0x4b);
        out
    }

    fn battery() -> Battery {
        Battery {
            level: 100,
            charge: skyward_info::Charge::Full,
            temp: None,
            health: None,
            cycles: None,
            design: None,
            full: None,
            voltage: None,
            quick: false,
        }
    }

    fn options(root: &Path, image: PathBuf, sha256: String) -> Options {
        let partitions = root.join("partitions");
        fs::create_dir_all(&partitions).unwrap();
        for name in ["boot_a", "boot_b"] {
            fs::write(partitions.join(name), vec![0xaa; 1 << 20]).unwrap();
        }
        Options {
            device: "bronco".into(),
            current: Slot::A,
            slots: vec![Slot::A, Slot::B],
            battery: battery(),
            plugin: Plugin {
                name: "Shinobu kernel".into(),
                subtitle: None,
                repo: "example/kernel".into(),
                asset: "kernel-{version}.img".into(),
                folder: "Shinobu".into(),
                note: None,
                manager_name: None,
            },
            tag: "v1".into(),
            asset: "kernel.img".into(),
            image,
            sha256,
            manager: None,
            manager_note: None,
            partition_dir: partitions,
            storage: root.join("storage"),
            stamp: "20261104-000000".into(),
        }
    }

    #[test]
    fn a_kernel_is_written_to_every_slot_with_the_copies_from_before() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("storage")).unwrap();
        let image = boot_image(4096);
        let path = dir.path().join("kernel.img");
        fs::write(&path, &image).unwrap();
        let sha = backup::hex(&Sha256::digest(&image));
        let options = options(dir.path(), path, sha.clone());
        let installed = match install(&options, &mut |_| {}) {
            Outcome::Installed(installed) => installed,
            other => panic!("unexpected outcome: {other:?}"),
        };
        assert_eq!(installed.slots, [Slot::A, Slot::B]);
        for name in ["boot_a", "boot_b"] {
            let got = fs::read(options.partition_dir.join(name)).unwrap();
            assert_eq!(&got[..image.len()], &image[..]);
            assert!(got[image.len()..].iter().all(|&b| b == 0xaa), "the rest of the partition is left as it was");
        }
        for name in ["boot_a.img", "boot_b.img"] {
            let copy = fs::read(installed.backup_dir.join(name)).unwrap();
            assert_eq!(copy.len(), 1 << 20);
            assert!(copy.iter().all(|&b| b == 0xaa));
        }
        let manifest = fs::read_to_string(installed.run_dir.join("manifest.txt")).unwrap();
        assert!(manifest.contains("release = v1"));
        assert!(manifest.contains("image_sha256 ="));
        assert!(manifest.contains("backup_a = backup/boot_a.img"));
    }

    #[test]
    fn the_kept_manager_is_readable_by_android() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("storage")).unwrap();
        let image = boot_image(4096);
        let path = dir.path().join("kernel.img");
        fs::write(&path, &image).unwrap();
        let sha = backup::hex(&Sha256::digest(&image));
        let mut options = options(dir.path(), path, sha);
        let source = dir.path().join("manager-source.apk");
        fs::write(&source, b"apk").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        options.manager = Some(source);
        let installed = match install(&options, &mut |_| {}) {
            Outcome::Installed(installed) => installed,
            other => panic!("unexpected outcome: {other:?}"),
        };
        let manager = installed.manager.expect("the manager is kept");
        assert_eq!(fs::metadata(manager).unwrap().permissions().mode() & 0o777, 0o644);
    }

    #[test]
    fn an_image_the_wrong_size_for_its_partition_is_refused_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("storage")).unwrap();
        let image = boot_image(2 << 20);
        let path = dir.path().join("kernel.img");
        fs::write(&path, &image).unwrap();
        let sha = backup::hex(&Sha256::digest(&image));
        let options = options(dir.path(), path, sha);
        assert_eq!(install(&options, &mut |_| {}), Outcome::Refused(Refusal::TooLarge {
            slot: Slot::A,
            partition: PARTITION,
            image: image.len() as u64,
            partition_size: 1 << 20,
        }));
        assert!(fs::read(options.partition_dir.join("boot_a")).unwrap().iter().all(|&b| b == 0xaa));
    }
}
