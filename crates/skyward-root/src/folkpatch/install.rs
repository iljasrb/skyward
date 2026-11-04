//! The run itself: what is checked before anything is written, the patch
//! `kptools` makes of a boot partition, and how the result is written and read
//! back.
//!
//! Nothing here runs the manager or any script of it: the pinned
//! `kptools` is started directly, what it makes is looked into before it is
//! written, and the key reaches it down a pipe rather than on a command line a
//! `ps` would show.

use crate::backup::{self, partition_size, Backup, Layout};
use crate::BOOT;
use super::{
    Failure, Installed, Key, Options, Outcome, Refusal, Stage, SuperKey, KPIMG, KPIMG_SHA256, KPIMG_VERSION, KPTOOLS,
    KPTOOLS_SHA256,
};
use anyhow::{ensure, Context, Result};
use skyward_install::image::{self, ImageKind, Ramdisk};
use skyward_vab::Slot;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What the run needs beside the key: the interpreter `kptools` is linked
/// against, and the libraries it asks for.
const TOOL_LIBS: [(&str, &str); 5] = [
    ("/system/bin/linker64", "linker64"),
    ("/system/lib64/libc.so", "libc"),
    ("/system/lib64/libm.so", "libm"),
    ("/system/lib64/libdl.so", "libdl"),
    ("/system/lib64/libz.so", "libz"),
];

/// Room left on the owner's storage after everything a run writes, so that the
/// last copy can be renamed and the manifest written beside it.
const MARGIN: u64 = 8 << 20;

/// Where a version 3 or 4 boot image keeps its kernel, and the header the
/// count of its bytes is read from.
const KERNEL_AT: u64 = 4096;
const HEADER: usize = 4096;

/// The footer a signed image ends with, which the bootloader looks for at the
/// end of the partition: 64 bytes whose first four are this magic.
const FOOTER: &[u8] = b"AVBf";
const FOOTER_LEN: u64 = 64;

/// Patch the boot partitions `options` names, or say why not. Every check runs
/// before the first write, and each slot is patched and written on its own, so
/// that what is needed on the storage at once stays small.
pub fn install(options: &Options, tell: &mut dyn FnMut(Stage)) -> Outcome {
    match Run::new(options, tell).and_then(|mut run| run.go()) {
        Ok(installed) => Outcome::Installed(installed),
        Err(Stopped::Refused(refusal)) => Outcome::Refused(refusal),
        Err(Stopped::Failed(failure)) => Outcome::Failed(*failure),
    }
}

/// How a run stops when it cannot go on.
enum Stopped {
    /// Nothing was written to a boot partition.
    Refused(Refusal),
    Failed(Box<Failure>),
}

/// How a step stops, before the run says what it had already done.
enum Stop {
    Refused(Refusal),
    Why(anyhow::Error),
}

impl From<anyhow::Error> for Stop {
    fn from(e: anyhow::Error) -> Stop {
        Stop::Why(e)
    }
}

struct Run<'a> {
    options: &'a Options,
    tell: &'a mut dyn FnMut(Stage),
    stage: Stage,
    slot: Option<Slot>,
    /// The slots to patch, as the preflight found them.
    slots: Vec<Slot>,
    /// Where the run's own folder is, and the private one named after it.
    layout: Layout,
    private: PathBuf,
    /// The pinned tools, unpacked for this run and removed after it.
    tools: PathBuf,
    backups: Vec<Backup>,
    written: Vec<Slot>,
    key: Option<SuperKey>,
    key_path: Option<PathBuf>,
    kallsyms_all: bool,
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
                key_path: None,
            }))
        };
        if !options.storage.is_dir() {
            return Err(Stopped::Refused(Refusal::StorageClosed));
        }
        let layout = Layout::make(&options.storage, &options.stamp).map_err(failed)?;
        // The private folder is named after the run's own, so that two runs
        // in the same second cannot find each other's key.
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
            slots: Vec::new(),
            layout,
            private,
            tools,
            backups: Vec::new(),
            written: Vec::new(),
            key: None,
            key_path: None,
            kallsyms_all: true,
        })
    }

    fn go(&mut self) -> Result<Installed, Stopped> {
        self.told(Stage::Checking);
        self.unpack().map_err(|stop| self.stopped(stop))?;
        self.preflight().map_err(|stop| self.stopped(stop))?;
        self.saved().map_err(|stop| self.stopped(stop))?;
        for slot in &self.slots.clone() {
            self.told(Stage::Patching { slot: *slot });
            self.slot = Some(*slot);
            let patched = self.patch(*slot).map_err(|stop| self.stopped(stop))?;
            self.told(Stage::Verifying { slot: *slot });
            let size = self.verify(*slot, &patched).map_err(|stop| self.stopped(stop))?;
            self.told(Stage::Flashing { slot: *slot, done: 0, total: size });
            self.flash(*slot, &patched, size).map_err(|stop| self.stopped(stop))?;
            self.written.push(*slot);
        }
        let _ = fs::remove_dir_all(&self.tools);
        let key = self.key.clone().expect("the key was made before any write");
        Ok(Installed {
            slots: self.written.clone(),
            key,
            key_path: self.key_path.clone().expect("the key was saved before any write"),
            backup_dir: self.layout.backups.clone(),
            kallsyms_all: self.kallsyms_all,
        })
    }

    /// The tools, unpacked where only root reads them.
    fn unpack(&mut self) -> Result<(), Stop> {
        self.write_tool("kptools", KPTOOLS)?;
        self.write_tool("kpimg", KPIMG)?;
        Ok(())
    }

    fn write_tool(&self, name: &str, bytes: &[u8]) -> Result<(), Stop> {
        let path = self.tools.join(name);
        fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).with_context(|| format!("let {} be run", path.display()))?;
        Ok(())
    }

    /// Everything that can stop a run before a boot partition is written.
    fn preflight(&mut self) -> Result<(), Stop> {
        if std::env::consts::ARCH != "aarch64" {
            return Err(Stop::Refused(Refusal::NotAarch64));
        }
        for (path, name) in TOOL_LIBS {
            if !Path::new(path).exists() {
                return Err(Stop::Refused(Refusal::NoTools(format!("{name}, as {path}"))));
            }
        }
        self.slots = crate::slots(&self.options.slots).map_err(|_| Stop::Refused(Refusal::NoSlot))?;
        for slot in &self.slots.clone() {
            let device = image::device(&self.options.partition_dir, BOOT, *slot);
            if !device.exists() {
                return Err(Stop::Refused(Refusal::Missing(*slot)));
            }
            let mut image_bytes = File::open(&device).with_context(|| format!("open {}", device.display()))?;
            match self.kind(&mut image_bytes).map_err(Stop::Why)? {
                ImageKind::Boot { kernel: true, ramdisk: Ramdisk::Recovery } => {
                    return Err(Stop::Refused(Refusal::RecoveryRamdisk(*slot)));
                }
                ImageKind::Boot { kernel: true, .. } => {}
                _ => return Err(Stop::Refused(Refusal::NotBootImage(*slot))),
            }
            let shown = device.to_string_lossy().into_owned();
            let flags = self
                .kptools(&["-f", "-i", &shown])
                .map_err(|e| Stop::Refused(Refusal::NoTools(format!("kptools cannot read {}: {e:#}", device.display()))))?;
            let (kallsyms, all) = kallsyms(&flags);
            self.kallsyms_all &= all;
            if !kallsyms {
                return Err(Stop::Refused(Refusal::NoKallsyms));
            }
            if !self.options.repatch && self.patched(&mut image_bytes, *slot).map_err(Stop::Why)? {
                return Err(Stop::Refused(Refusal::AlreadyPatched(*slot)));
            }
        }
        if let Some(refusal) = skyward_install::battery_refusal(&self.options.battery) {
            return Err(Stop::Refused(match refusal {
                skyward_install::Refusal::Battery { level, charging } => Refusal::Battery { level, charging },
                other => return Err(Stop::Why(anyhow::anyhow!("{other:?}"))),
            }));
        }
        let storage = &self.options.storage;
        // A partition's own device says where its end is; metadata does not.
        let mut need = MARGIN;
        for slot in &self.slots.clone() {
            let device = image::device(&self.options.partition_dir, BOOT, *slot);
            need += partition_size(&device).map_err(Stop::Why)?;
        }
        let have = available(storage).map_err(Stop::Why)?;
        if need > have {
            return Err(Stop::Refused(Refusal::NoRoom { need, have }));
        }
        Ok(())
    }

    /// The copies of the boot partitions, then what has to be in the owner's
    /// storage before a boot partition is touched: the key, and the manifest
    /// that says where everything is.
    fn saved(&mut self) -> Result<(), Stop> {
        for slot in &self.slots.clone() {
            self.told(Stage::BackingUp { slot: *slot, done: 0, total: 0 });
            let device = image::device(&self.options.partition_dir, BOOT, *slot);
            let mut tell = |stage: Stage| (self.tell)(stage);
            let copy = backup::save(&device, &self.layout.backups, *slot, &mut tell).map_err(Stop::Why)?;
            self.backups.push(copy);
        }
        let key = match &self.options.key {
            Key::Generate => SuperKey::generate().context("make a key"),
            Key::Enter(key) => Ok(key.clone()),
        }
        .map_err(Stop::Why)?;
        self.key = Some(key.clone());
        let key_path = self.save_key(&key).map_err(Stop::Why)?;
        self.key_path = Some(key_path.clone());
        self.write_manifest(&key_path).map_err(Stop::Why)?;
        Ok(())
    }

    fn save_key(&mut self, key: &SuperKey) -> Result<PathBuf> {
        let path = self.private.join("superkey.txt");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("make {}", path.display()))?;
        file.write_all(key.expose().as_bytes()).with_context(|| format!("write {}", path.display()))?;
        file.sync_all().with_context(|| format!("flush {}", path.display()))?;
        let kept = fs::read(&path).with_context(|| format!("read {} back", path.display()))?;
        ensure!(kept == key.expose().as_bytes(), "{} does not hold the key", path.display());
        Ok(path)
    }

    fn write_manifest(&self, key_path: &Path) -> Result<()> {
        let layout = &self.layout;
        let facts = backup::Facts {
            device: &self.options.device,
            current: self.options.current,
            slots: &self.slots,
            kpimg_version: KPIMG_VERSION,
            kpimg_sha256: KPIMG_SHA256,
            kptools_sha256: KPTOOLS_SHA256,
            backups: &self.backups,
            key_path,
        };
        backup::write(&facts, &layout.dir).with_context(|| format!("write the manifest in {}", layout.dir.display()))?;
        Ok(())
    }

    /// Patch `slot`'s boot partition into a file, with the key down a pipe.
    fn patch(&mut self, slot: Slot) -> Result<PathBuf, Stop> {
        let key = self.key.as_ref().context("the key was made before any write")?;
        let source = image::device(&self.options.partition_dir, BOOT, slot);
        let out = self.tools.join(format!("patched{}.img", slot.suffix()));
        let (read, mut write) = pipe()?;
        let fd = read.as_raw_fd();
        write.write_all(key.expose().as_bytes()).context("put the key in the pipe")?;
        // Nothing else writes to the pipe, so the tools see the key and then
        // the end of it.
        drop(write);
        let started = Command::new(self.tools.join("kptools"))
            .args(["-p", "-i"])
            .arg(&source)
            .arg("-k")
            .arg(self.tools.join("kpimg"))
            .arg("-o")
            .arg(&out)
            .arg("--root-skey-fd")
            .arg(fd.to_string())
            .stdin(Stdio::null())
            .output();
        drop(read);
        let output = started.with_context(|| format!("run kptools on {}", source.display()))?;
        if !output.status.success() {
            return Err(Stop::Why(anyhow::anyhow!(
                "kptools failed on {}: {}",
                source.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(out)
    }

    /// Read the patch back before it is written: it is a boot image with a
    /// kernel in it, the tools call it patched, it fits, and it carries the AVB
    /// footer where the phone's bootloader looks for one.
    fn verify(&mut self, slot: Slot, patched: &Path) -> Result<u64, Stop> {
        let size = fs::metadata(patched).with_context(|| format!("find the size of {}", patched.display()))?.len();
        let source = image::device(&self.options.partition_dir, BOOT, slot);
        let room = partition_size(&source)?;
        if size == 0 || size > room {
            return Err(Stop::Why(anyhow::anyhow!("the patch is {size} bytes, more than the {room} its partition holds")));
        }
        let mut file = File::open(patched).with_context(|| format!("open {}", patched.display()))?;
        match self.kind(&mut file).map_err(Stop::Why)? {
            ImageKind::Boot { kernel: true, ramdisk: Ramdisk::Recovery } => {
                return Err(Stop::Refused(Refusal::RecoveryRamdisk(slot)));
            }
            ImageKind::Boot { kernel: true, .. } => {}
            _ => return Err(Stop::Why(anyhow::anyhow!("what kptools made of {} is no boot image with a kernel", source.display()))),
        }
        if !self.patched(&mut file, slot).map_err(Stop::Why)? {
            return Err(Stop::Why(anyhow::anyhow!("kptools does not call its own patch patched")));
        }
        if ends_signed(&mut File::open(&source).with_context(|| format!("open {}", source.display()))?)? {
            let mut made = File::open(patched).with_context(|| format!("open {}", patched.display()))?;
            if !ends_signed(&mut made)? {
                return Err(Stop::Why(anyhow::anyhow!("the AVB footer is not in the last bytes of the patch")));
            }
        }
        Ok(size)
    }

    fn flash(&mut self, slot: Slot, patched: &Path, size: u64) -> Result<(), Stop> {
        let device = image::device(&self.options.partition_dir, BOOT, slot);
        let mut file = File::open(patched).with_context(|| format!("open {}", patched.display()))?;
        let mut progress = |done: u64, total: u64| (self.tell)(Stage::Flashing { slot, done, total });
        image::flash(&mut file, size, std::slice::from_ref(&device), &mut progress)
            .with_context(|| format!("write {} and read it back", device.display()))?;
        Ok(())
    }

    /// What kind of image the open file starts with.
    fn kind(&self, file: &mut File) -> Result<ImageKind> {
        let mut head = vec![0u8; HEADER];
        let read = file.read(&mut head)?;
        head.truncate(read);
        image::detect(file, &head)?.with_context(|| "no image this recovery writes")
    }

    /// Whether KernelPatch says the kernel in `file` is patched already.
    fn patched(&self, file: &mut File, slot: Slot) -> Result<bool> {
        let kernel = self.tools.join(format!("kernel{}.img", slot.suffix()));
        fs::write(&kernel, kernel_of(file)?).with_context(|| format!("write {}", kernel.display()))?;
        let shown = kernel.to_string_lossy().into_owned();
        let list = self.kptools(&["-l", "-i", &shown]);
        let _ = fs::remove_file(&kernel);
        Ok(patched(&list.context("ask kptools what the kernel is")?).unwrap_or(false))
    }

    fn kptools(&self, args: &[&str]) -> Result<String> {
        let output = Command::new(self.tools.join("kptools"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .context("run kptools")?;
        ensure!(output.status.success(), "kptools failed: {}", String::from_utf8_lossy(&output.stderr).trim());
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn told(&mut self, stage: Stage) {
        self.stage = stage;
        (self.tell)(stage);
    }

    fn stopped(&self, stop: Stop) -> Stopped {
        // The tools last one run and are unpacked again by the next.
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
            key_path: self.key_path.clone(),
        }
    }
}

/// A pipe, whose reading end the tools take the key from.
fn pipe() -> Result<(File, File)> {
    let mut fds = [0i32; 2];
    ensure!(unsafe { libc::pipe(fds.as_mut_ptr()) } == 0, "make a pipe: {}", std::io::Error::last_os_error());
    Ok(unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) })
}

/// The kernel in the version 3 or 4 boot image `file` starts with.
fn kernel_of(file: &mut File) -> Result<Vec<u8>> {
    let mut header = [0u8; 44];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header).context("read the boot image's header")?;
    ensure!(header.starts_with(b"ANDROID!"), "no boot image header");
    let version = u32::from_le_bytes([header[40], header[41], header[42], header[43]]);
    ensure!(version >= 3, "a version {version} boot image, whose kernel is not laid out as this recovery expects");
    let size = u32::from_le_bytes([header[8], header[9], header[10], header[11]]) as usize;
    ensure!(size > 0, "the boot image holds no kernel");
    let mut kernel = vec![0u8; size];
    file.seek(SeekFrom::Start(KERNEL_AT))?;
    file.read_exact(&mut kernel).context("read the kernel")?;
    Ok(kernel)
}

/// Whether the last bytes of `file` are the AVB footer a bootloader looks for.
pub(crate) fn ends_signed(file: &mut File) -> Result<bool> {
    let len = file.seek(SeekFrom::End(0))?;
    if len < FOOTER_LEN {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-(FOOTER_LEN as i64)))?;
    let mut footer = [0u8; FOOTER_LEN as usize];
    file.read_exact(&mut footer)?;
    Ok(footer.starts_with(FOOTER))
}

/// Whether `kptools -l` says the kernel is patched, and nothing when it says
/// neither.
fn patched(list: &str) -> Option<bool> {
    list.lines().find_map(|line| {
        let value = line.strip_prefix("patched=")?;
        match value.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    })
}

/// Whether the kernel was built with `CONFIG_KALLSYMS`, which KernelPatch
/// needs, and with `CONFIG_KALLSYMS_ALL`, which some of what it runs does.
fn kallsyms(flags: &str) -> (bool, bool) {
    let set = |name: &str| flags.lines().any(|line| line.trim() == format!("{name}=y"));
    (set("CONFIG_KALLSYMS"), set("CONFIG_KALLSYMS_ALL"))
}

/// Whether `slot`'s boot image carries the patch already, as `kptools` reads
/// it. The tools are written under `private` for the length of the question
/// and taken away again; nothing on the phone is written.
pub fn patched_slot(partition_dir: &Path, slot: Slot, private: &Path) -> Result<bool> {
    let tools = private.join("probe");
    let made = (|| -> Result<()> {
        fs::create_dir_all(&tools).with_context(|| format!("make {}", tools.display()))?;
        for dir in [private, tools.as_path()] {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).with_context(|| format!("keep {} to root", dir.display()))?;
        }
        Ok(())
    })();
    if let Err(e) = made {
        let _ = fs::remove_dir_all(&tools);
        return Err(e);
    }
    let asked = (|| -> Result<bool> {
        let kptools = tools.join("kptools");
        fs::write(&kptools, KPTOOLS).with_context(|| format!("write {}", kptools.display()))?;
        fs::set_permissions(&kptools, fs::Permissions::from_mode(0o700)).with_context(|| format!("let {} be run", kptools.display()))?;
        let device = image::device(partition_dir, BOOT, slot);
        let mut image = File::open(&device).with_context(|| format!("open {}", device.display()))?;
        let kernel = tools.join("kernel.img");
        fs::write(&kernel, kernel_of(&mut image)?).with_context(|| format!("write {}", kernel.display()))?;
        let output = Command::new(&kptools)
            .args(["-l", "-i", &kernel.to_string_lossy()])
            .stdin(Stdio::null())
            .output()
            .context("ask kptools about the kernel")?;
        let list = String::from_utf8_lossy(&output.stdout);
        Ok(patched(&list).unwrap_or(false))
    })();
    let _ = fs::remove_dir_all(&tools);
    asked
}

/// The bytes left on the filesystem `dir` is on.
pub(crate) fn available(dir: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).context("the storage path")?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    ensure!(unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } == 0, "ask the size of {}", dir.display());
    let stats = unsafe { stats.assume_init() };
    Ok(stats.f_bavail * stats.f_frsize)
}

/// The SHA-256 of `bytes`, as the manifest and the screen write it.
#[cfg(test)]
fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    crate::backup::hex(&sha2::Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kernel_the_tools_call_patched_is_read_as_patched() {
        let list = "[kernel]\nbanner=Linux version 5.10\npatched=true\n[kpimg]\nversion=0xd09\n";
        assert_eq!(patched(list), Some(true));
        assert_eq!(patched("patched=false\n"), Some(false));
        assert_eq!(patched("banner=Linux\n"), None, "an answer this recovery does not know is no answer");
    }

    #[test]
    fn a_kernel_without_every_symbol_is_told_apart_from_one_without_symbols() {
        assert_eq!(kallsyms("CONFIG_KALLSYMS=y\nCONFIG_KALLSYMS_ALL=y\n"), (true, true));
        assert_eq!(kallsyms("CONFIG_KALLSYMS=y\nCONFIG_KALLSYMS_ALL=n\n"), (true, false));
        assert_eq!(kallsyms("CONFIG_KALLSYMS=n\n"), (false, false));
        assert_eq!(kallsyms(""), (false, false));
    }

    #[test]
    fn the_kernel_is_taken_from_where_a_boot_image_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("boot.img");
        let mut bytes = vec![0u8; (KERNEL_AT as usize) + 8];
        bytes[..8].copy_from_slice(b"ANDROID!");
        bytes[8..12].copy_from_slice(&8u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&4u32.to_le_bytes());
        bytes[KERNEL_AT as usize..].copy_from_slice(b"a kernel");
        fs::write(&path, &bytes).unwrap();
        assert_eq!(kernel_of(&mut File::open(&path).unwrap()).unwrap(), b"a kernel");
    }

    #[test]
    fn a_footer_is_only_found_where_the_last_sixty_four_bytes_are_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut footer = FOOTER.to_vec();
        footer.resize(FOOTER_LEN as usize, 0);
        let signed = dir.path().join("signed.img");
        fs::write(&signed, [b"ANDROID!".as_slice(), &[0u8; 128], &footer].concat()).unwrap();
        assert!(ends_signed(&mut File::open(&signed).unwrap()).unwrap());
        // The magic at the very end is not a footer: a real one is 64 bytes,
        // its magic at the start of them.
        let trailing = dir.path().join("trailing.img");
        fs::write(&trailing, [b"ANDROID!".as_slice(), &[0u8; 128], FOOTER].concat()).unwrap();
        assert!(!ends_signed(&mut File::open(&trailing).unwrap()).unwrap());
        let unsigned = dir.path().join("unsigned.img");
        fs::write(&unsigned, vec![0u8; 128]).unwrap();
        assert!(!ends_signed(&mut File::open(&unsigned).unwrap()).unwrap());
    }

    #[test]
    fn the_sha_of_a_pinned_file_is_the_one_the_build_checked() {
        assert_eq!(sha256(KPTOOLS), KPTOOLS_SHA256);
        assert_eq!(sha256(KPIMG), KPIMG_SHA256);
    }
}
