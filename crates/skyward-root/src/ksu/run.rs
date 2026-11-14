//! The run itself: taking `ksud` from the manager, saving the partitions as
//! they were, patching them, and reading each patch back before it is
//! written.
//!
//! Nothing of the manager is run as a script. `ksud` is started directly,
//! with the image given to it rather than the partition, so the patch is
//! looked into before this recovery writes anything.

use super::apk::{self, Ksud};
use super::{Failure, Installed, Options, Outcome, Refusal, Stage};
use crate::backup::{self, hash_file, hex, partition_size, Backup, Layout};
use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};
use skyward_install::image::{self, ImageKind, Ramdisk};
use skyward_vab::Slot;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The folder a run's copies and manager are kept in, under `Download/Skyward`.
const FOLDER: &str = "KSU";

/// Room left on the owner's storage after everything a run writes.
const MARGIN: u64 = 8 << 20;

/// Where a boot image's header is, and how much of it is read to tell what
/// kind of image it is.
const HEADER: usize = 4096;

/// The footer a signed image ends with: 64 bytes whose first four are this.
const FOOTER: &[u8] = b"AVBf";
const FOOTER_LEN: u64 = 64;

/// Patch the partitions of `options`' slots, or say why not. Every check runs
/// before the first write, and each slot is patched and written on its own.
pub fn install(options: &Options, tell: &mut dyn FnMut(Stage)) -> Outcome {
    match Run::new(options, tell).and_then(|mut run| run.go()) {
        Ok(installed) => Outcome::Installed(installed),
        Err(Stopped::Refused(refusal)) => Outcome::Refused(refusal),
        Err(Stopped::Failed(failure)) => Outcome::Failed(*failure),
    }
}

enum Stopped {
    /// Nothing was written to a boot partition.
    Refused(Refusal),
    Failed(Box<Failure>),
}

enum Stop {
    Refused(Refusal),
    Why(anyhow::Error),
}

impl From<anyhow::Error> for Stop {
    fn from(e: anyhow::Error) -> Stop {
        Stop::Why(e)
    }
}

/// One slot's target: the partition chosen and the link to it.
#[derive(Clone)]
struct Target {
    slot: Slot,
    partition: &'static str,
    device: PathBuf,
}

struct Run<'a> {
    options: &'a Options,
    tell: &'a mut dyn FnMut(Stage),
    stage: Stage,
    slot: Option<Slot>,
    targets: Vec<Target>,
    layout: Layout,
    private: PathBuf,
    tools: PathBuf,
    ksud: Option<Ksud>,
    kmi: Option<String>,
    backups: Vec<Backup>,
    written: Vec<Slot>,
    apk: Option<(PathBuf, String, u64)>,
}

impl<'a> Run<'a> {
    fn new(options: &'a Options, tell: &'a mut dyn FnMut(Stage)) -> Result<Run<'a>, Stopped> {
        let failed = |e: anyhow::Error| {
            Stopped::Failed(Box::new(Failure {
                stage: Stage::Checking,
                slot: None,
                why: format!("{e:#}"),
                backups: Vec::new(),
                written: Vec::new(),
                run_dir: None,
            }))
        };
        if !options.storage.is_dir() {
            return Err(Stopped::Refused(Refusal::StorageClosed));
        }
        let layout = Layout::make_in(&options.storage, FOLDER, &options.stamp).map_err(failed)?;
        let private = options.private.join(layout.dir.file_name().unwrap_or_default());
        let tools = private.join("tools");
        let made = (|| -> Result<()> {
            fs::create_dir_all(&tools).with_context(|| format!("make {}", tools.display()))?;
            fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).with_context(|| format!("keep {} to root", private.display()))?;
            fs::set_permissions(&tools, fs::Permissions::from_mode(0o700)).with_context(|| format!("keep {} to root", tools.display()))?;
            Ok(())
        })();
        if let Err(e) = made {
            let _ = fs::remove_dir_all(&layout.dir);
            let _ = fs::remove_dir_all(&private);
            return Err(failed(e));
        }
        Ok(Run {
            options,
            tell,
            stage: Stage::Checking,
            slot: None,
            targets: Vec::new(),
            layout,
            private,
            tools,
            ksud: None,
            kmi: None,
            backups: Vec::new(),
            written: Vec::new(),
            apk: None,
        })
    }

    fn go(&mut self) -> Result<Installed, Stopped> {
        self.told(Stage::Checking);
        self.extract().map_err(|stop| self.stopped(stop))?;
        self.preflight().map_err(|stop| self.stopped(stop))?;
        self.saved().map_err(|stop| self.stopped(stop))?;
        for target in self.targets.clone() {
            self.slot = Some(target.slot);
            self.told(Stage::Patching { slot: target.slot, partition: target.partition });
            let patched = self.patch(&target).map_err(|stop| self.stopped(stop))?;
            self.told(Stage::Verifying { slot: target.slot, partition: target.partition });
            let size = self.verify(&target, &patched).map_err(|stop| self.stopped(stop))?;
            self.told(Stage::Flashing { slot: target.slot, partition: target.partition, done: 0, total: size });
            self.flash(&target, &patched, size).map_err(|stop| self.stopped(stop))?;
            self.written.push(target.slot);
        }
        let _ = fs::remove_dir_all(&self.tools);
        let (apk, apk_sha256, _) = self.apk.clone().expect("the manager was kept before any write");
        Ok(Installed {
            slots: self.targets.iter().map(|t| t.slot).collect(),
            partitions: self.targets.iter().map(|t| t.partition).collect(),
            flavor: self.options.flavor,
            tag: self.options.tag.clone(),
            apk,
            apk_sha256,
            ksud_sha256: self.ksud.as_ref().expect("ksud was taken out before any write").sha256.clone(),
            backup_dir: self.layout.backups.clone(),
            run_dir: self.layout.dir.clone(),
        })
    }

    /// Take `ksud` out of the manager and see that it runs.
    fn extract(&mut self) -> Result<(), Stop> {
        self.told(Stage::Extracting);
        let ksud = apk::extract(&self.options.apk, &self.tools)
            .map_err(|e| Stop::Refused(Refusal::NoKsud(format!("{e:#}"))))?;
        let output = Command::new(&ksud.path).arg("--version").stdin(Stdio::null()).output();
        match output {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                return Err(Stop::Refused(Refusal::NoKsud(format!(
                    "ksud --version: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ))));
            }
            Err(e) => return Err(Stop::Refused(Refusal::NoKsud(format!("run ksud: {e}")))),
        }
        self.ksud = Some(ksud);
        Ok(())
    }

    /// Everything that can stop a run before a partition is written.
    fn preflight(&mut self) -> Result<(), Stop> {
        if std::env::consts::ARCH != "aarch64" {
            return Err(Stop::Refused(Refusal::NotAarch64));
        }
        let apk = &self.options.apk;
        if !apk.is_file() {
            return Err(Stop::Why(anyhow::anyhow!("the manager's APK, {}, is not there", apk.display())));
        }
        let sha256 = hash_file(apk)?;
        if let Some(expected) = &self.options.apk_sha256
            && !sha256.eq_ignore_ascii_case(expected) {
                return Err(Stop::Refused(Refusal::Damaged));
            }
        self.kmi = self.options.kmi.clone().or_else(kmi);
        let slots = crate::slots(&self.options.slots).map_err(|_| Stop::Refused(Refusal::NoSlot))?;
        let mut need = MARGIN + fs::metadata(apk).map(|m| m.len()).unwrap_or(0);
        let mut biggest = 0;
        for slot in slots {
            let (partition, device) = self.target(slot);
            if !device.exists() {
                return Err(Stop::Refused(Refusal::Missing { slot, partition }));
            }
            let mut file = File::open(&device).with_context(|| format!("open {}", device.display()))?;
            match self.kind(&mut file).map_err(Stop::Why)? {
                ImageKind::Boot { ramdisk: Ramdisk::Recovery, .. } => {
                    return Err(Stop::Refused(Refusal::RecoveryRamdisk { slot, partition }));
                }
                ImageKind::Boot { .. } => {}
                _ => return Err(Stop::Refused(Refusal::NotBootImage { slot, partition })),
            }
            let size = partition_size(&device)?;
            need += size;
            biggest = biggest.max(size);
            self.targets.push(Target { slot, partition, device });
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
        // The patched image is written among the tools, on the data partition.
        let apk_size = fs::metadata(apk).map(|m| m.len()).unwrap_or(0);
        let private_need = MARGIN + apk_size * 2 + biggest;
        let private_have = crate::folkpatch::install::available(&self.tools).map_err(Stop::Why)?;
        if private_need > private_have {
            return Err(Stop::Refused(Refusal::NoRoom { need: private_need, have: private_have }));
        }
        Ok(())
    }

    /// The partition a slot's patch goes to: `init_boot` where the phone has
    /// it, as the manager itself does, and `boot` otherwise. Android 12
    /// phones with an `init_boot` still patch `boot`.
    fn target(&self, slot: Slot) -> (&'static str, PathBuf) {
        let init = image::device(&self.options.partition_dir, "init_boot", Some(slot));
        let skip_init = self.kmi.as_deref().is_some_and(|kmi| kmi.starts_with("android12-"));
        if init.exists() && !skip_init {
            ("init_boot", init)
        } else {
            ("boot", image::device(&self.options.partition_dir, "boot", Some(slot)))
        }
    }

    /// The copies of the partitions, the manager beside them, and the
    /// manifest that says what the run was.
    fn saved(&mut self) -> Result<(), Stop> {
        for target in self.targets.clone() {
            self.told(Stage::BackingUp { slot: target.slot, partition: target.partition, done: 0, total: 0 });
            let mut tell = |done, total| {
                (self.tell)(Stage::BackingUp { slot: target.slot, partition: target.partition, done, total });
            };
            let copy = backup::save_with(
                &target.device,
                &self.layout.backups,
                target.slot,
                target.partition,
                &backup::partition_name(target.partition, target.slot),
                &mut tell,
            )
            .map_err(Stop::Why)?;
            self.backups.push(copy);
        }
        self.told(Stage::SavingManager);
        self.apk = Some(self.save_manager().map_err(Stop::Why)?);
        self.write_manifest().map_err(Stop::Why)?;
        Ok(())
    }

    /// Keep the manager's APK in the run's folder, where Android can install
    /// it once the phone starts.
    fn save_manager(&self) -> Result<(PathBuf, String, u64)> {
        let source = &self.options.apk;
        let path = self.layout.dir.join("manager.apk");
        let part = path.with_extension("apk.part");
        let mut reading = File::open(source).with_context(|| format!("open {}", source.display()))?;
        let mut writing = File::options()
            .write(true)
            .create_new(true)
            .open(&part)
            .with_context(|| format!("make {}", part.display()))?;
        let mut digest = Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        let mut size = 0;
        loop {
            let read = reading.read(&mut buffer).with_context(|| format!("read {}", source.display()))?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
            writing.write_all(&buffer[..read]).with_context(|| format!("write {}", part.display()))?;
            size += read as u64;
        }
        writing.sync_all().with_context(|| format!("flush {}", part.display()))?;
        let sha256 = hex(&digest.finalize());
        fs::rename(&part, &path).with_context(|| format!("put {} in place of {}", part.display(), path.display()))?;
        ensure!(fs::metadata(&path).map(|m| m.len()).unwrap_or(0) == size, "{} is not the whole manager", path.display());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).with_context(|| format!("let Android read {}", path.display()))?;
        Ok((path, sha256, size))
    }

    fn write_manifest(&self) -> Result<()> {
        let mut text = String::new();
        text.push_str(&format!("device = {}\n", self.options.device));
        text.push_str(&format!("current_slot = {}\n", self.options.current.letter()));
        let slots: Vec<&str> = self.targets.iter().map(|t| t.slot.letter()).collect();
        text.push_str(&format!("slots = {}\n", slots.join(", ")));
        text.push_str(&format!("manager_flavor = {}\n", self.options.flavor.id()));
        text.push_str(&format!("manager_release = {}\n", self.options.tag));
        if let Some((path, sha256, size)) = &self.apk {
            let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            text.push_str(&format!("manager_apk = {name} {size} {sha256}\n"));
        }
        if let Some(ksud) = &self.ksud {
            text.push_str(&format!("ksud_sha256 = {}\n", ksud.sha256));
        }
        for target in &self.targets {
            text.push_str(&format!("partition{} = {}\n", target.slot.suffix(), target.partition));
        }
        for copy in &self.backups {
            let kept = copy.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            text.push_str(&format!("backup{} = backup/{kept} {} {}\n", copy.slot.suffix(), copy.size, copy.sha256));
        }
        let path = self.layout.dir.join("manifest.txt");
        fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    /// Patch a slot's image into a file with `ksud`.
    fn patch(&self, target: &Target) -> Result<PathBuf, Stop> {
        let ksud = self.ksud.as_ref().context("ksud was taken out before any write")?;
        let out = self.tools.join("out");
        fs::create_dir_all(&out).with_context(|| format!("make {}", out.display()))?;
        let name = format!("patched-{}{}.img", target.partition, target.slot.suffix());
        let mut command = Command::new(&ksud.path);
        command
            .args(["boot-patch", "--boot"])
            .arg(&target.device)
            .arg("--out")
            .arg(&out)
            .arg("--out-name")
            .arg(&name)
            .arg("--partition")
            .arg(target.partition);
        if let Some(kmi) = &self.kmi {
            command.arg("--kmi").arg(kmi);
        }
        let output = command.stdin(Stdio::null()).output().with_context(|| format!("run ksud on {}", target.device.display()))?;
        if !output.status.success() {
            let said = [String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)]
                .iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(": ");
            return Err(Stop::Why(anyhow::anyhow!("ksud failed on {}: {said}", target.device.display())));
        }
        let path = out.join(name);
        if !path.is_file() {
            return Err(Stop::Why(anyhow::anyhow!("ksud wrote no image to {}", path.display())));
        }
        Ok(path)
    }

    /// Read the patch back before it is written: it is a boot image, it fits
    /// its partition, and it keeps the AVB footer the bootloader looks for.
    fn verify(&self, target: &Target, patched: &Path) -> Result<u64, Stop> {
        let size = fs::metadata(patched).with_context(|| format!("find the size of {}", patched.display()))?.len();
        let room = partition_size(&target.device)?;
        if size == 0 || size > room {
            return Err(Stop::Why(anyhow::anyhow!("the patch is {size} bytes, more than the {room} its partition holds")));
        }
        let mut file = File::open(patched).with_context(|| format!("open {}", patched.display()))?;
        match self.kind(&mut file).map_err(Stop::Why)? {
            ImageKind::Boot { ramdisk: Ramdisk::Recovery, .. } => {
                return Err(Stop::Refused(Refusal::RecoveryRamdisk { slot: target.slot, partition: target.partition }));
            }
            ImageKind::Boot { .. } => {}
            _ => {
                return Err(Stop::Why(anyhow::anyhow!(
                    "what ksud made of {} is no boot image",
                    target.device.display()
                )));
            }
        }
        if ends_signed(&mut File::open(&target.device).with_context(|| format!("open {}", target.device.display()))?)? {
            let mut made = File::open(patched).with_context(|| format!("open {}", patched.display()))?;
            if !ends_signed(&mut made)? {
                return Err(Stop::Why(anyhow::anyhow!("the AVB footer is not in the last bytes of the patch")));
            }
        }
        Ok(size)
    }

    fn flash(&mut self, target: &Target, patched: &Path, size: u64) -> Result<(), Stop> {
        let mut file = File::open(patched).with_context(|| format!("open {}", patched.display()))?;
        let mut progress =
            |done, total| (self.tell)(Stage::Flashing { slot: target.slot, partition: target.partition, done, total });
        image::flash(&mut file, size, std::slice::from_ref(&target.device), &mut progress)
            .with_context(|| format!("write {} and read it back", target.device.display()))?;
        Ok(())
    }

    /// What kind of image the open file starts with.
    fn kind(&self, file: &mut File) -> Result<ImageKind> {
        let mut head = vec![0u8; HEADER];
        let read = file.read(&mut head)?;
        head.truncate(read);
        image::detect(file, &head)?.with_context(|| "no image this recovery writes")
    }

    fn told(&mut self, stage: Stage) {
        self.stage = stage;
        (self.tell)(stage);
    }

    fn stopped(&self, stop: Stop) -> Stopped {
        let _ = fs::remove_dir_all(&self.tools);
        match stop {
            Stop::Refused(refusal) => {
                self.discard();
                Stopped::Refused(refusal)
            }
            Stop::Why(e) => Stopped::Failed(Box::new(self.failure(e))),
        }
    }

    /// A run that stopped before copying anything gives its folders back, so
    /// a refusal leaves nothing behind and the next try gets the same stamp.
    fn discard(&self) {
        if !self.backups.is_empty() {
            return;
        }
        let _ = fs::remove_dir_all(&self.layout.dir);
        let _ = fs::remove_dir_all(&self.private);
    }

    fn failure(&self, e: anyhow::Error) -> Failure {
        Failure {
            stage: self.stage,
            slot: self.slot,
            why: format!("{e:#}"),
            backups: self.backups.clone(),
            written: self.written.clone(),
            run_dir: Some(self.layout.dir.clone()),
        }
    }
}

/// Whether the last bytes of `file` are the AVB footer a bootloader looks for.
fn ends_signed(file: &mut File) -> Result<bool> {
    let len = file.seek(SeekFrom::End(0))?;
    if len < FOOTER_LEN {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-(FOOTER_LEN as i64)))?;
    let mut footer = [0u8; FOOTER_LEN as usize];
    file.read_exact(&mut footer)?;
    Ok(footer.starts_with(FOOTER))
}

/// The KMI of the running kernel, as the manager names it: `android13-5.10`,
/// from the release the kernel announces.
fn kmi() -> Option<String> {
    let release = fs::read_to_string("/proc/sys/kernel/osrelease").ok()?;
    parse_kmi(&release).or_else(|| fs::read_to_string("/proc/version").ok().and_then(|v| parse_kmi(&v)))
}

/// The `androidNN-X.Y` in a kernel version such as `6.1.75-android14-11`.
fn parse_kmi(version: &str) -> Option<String> {
    let android = version.find("android")? + "android".len();
    let (number, _) = digits(version, android)?;
    for (index, ch) in version.char_indices() {
        if !ch.is_ascii_digit() {
            continue;
        }
        let (major, at) = digits(version, index)?;
        if version[at..].starts_with('.') {
            let (minor, _) = digits(version, at + 1)?;
            return Some(format!("android{number}-{major}.{minor}"));
        }
    }
    None
}

fn digits(text: &str, at: usize) -> Option<(u64, usize)> {
    let end = text[at..].find(|c: char| !c.is_ascii_digit()).map_or(text.len(), |n| at + n);
    if end == at {
        return None;
    }
    text[at..end].parse().ok().map(|n| (n, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kmi_is_read_the_way_the_manager_names_it() {
        assert_eq!(parse_kmi("5.10.198-android13-8-g1e2f3").as_deref(), Some("android13-5.10"));
        assert_eq!(parse_kmi("6.1.75-android14-11-gabcdef").as_deref(), Some("android14-6.1"));
        assert_eq!(parse_kmi("4.19.191-android12-9").as_deref(), Some("android12-4.19"));
        assert_eq!(parse_kmi("6.6.30-custom").as_deref(), None, "no Android version");
        assert_eq!(parse_kmi("android14").as_deref(), None, "no kernel version");
    }

    #[test]
    fn a_footer_is_only_found_where_the_last_sixty_four_bytes_are_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut footer = FOOTER.to_vec();
        footer.resize(FOOTER_LEN as usize, 0);
        let signed = dir.path().join("signed.img");
        fs::write(&signed, [b"ANDROID!".as_slice(), &[0u8; 128], &footer].concat()).unwrap();
        assert!(ends_signed(&mut File::open(&signed).unwrap()).unwrap());
        let unsigned = dir.path().join("unsigned.img");
        fs::write(&unsigned, vec![0u8; 128]).unwrap();
        assert!(!ends_signed(&mut File::open(&unsigned).unwrap()).unwrap());
    }
}
