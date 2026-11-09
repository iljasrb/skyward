use crate::adbwifi;
use crate::backup;
use crate::decrypt::{self, Decryptor};
use crate::extract;
use crate::filework;
use crate::haptics::Haptics;
use crate::relabel;
use crate::input::{monotonic_ms, Event, Inputs};
use crate::merge;
use crate::partwork;
use crate::pty;
use crate::readings::{Reading, Readings};
use crate::secure;
use crate::services;
use crate::settings;
use crate::systems;
use crate::updates;
use crate::wipe::{self, Eraser, Step};
use crate::wlan;
use anyhow::{anyhow, ensure, Context, Result};
use skyward_block::fsck::Fixed;
use skyward_block::{fstab, mount, probe};
use skyward_config::{DeviceConfig, Service};
use skyward_crypto::data::{Closed, Opened};
use skyward_install::asked::{self, Needs};
use skyward_install::{Detach, Installer, Options};
use skyward_sys::bcb::{self, Work, REFORMATS};
use skyward_sys::control::{Control, Request};
use skyward_sys::Power;
use skyward_files::Ownership;
use skyward_block::disk::Disk;
use skyward_block::usb;
use skyward_net::download;
use skyward_net::http::Servers;
use skyward_net::github::{Build, Newest};
use skyward_net::rom;
use skyward_net::wlan::{Power as WlanPower, Stage as Joining, Wlan};
use skyward_sys::sleep::{Sleeper, Slept};
use skyward_sys::uevent;
use skyward_systems::Who;
use skyward_ui::app::{
    App, AdbWifi, Asked, BackupAt, BackupEnd, BackupEntry, BackupRefusal, ClashChoice, Data, DownloadEnd, Effect,
    EraseStop, FileKind, FileOp, FileRow, FileTask, FsAction, FsRow, Have, Held, ItemEnd, ItemState, JobEnd, Keys, MountFailure, Part, PartKind,
    PartStop, Place, RenameRefused, RomState, Screen, Shot, SkywardEnd, SkywardStage, Target, Medium, UsbLink, Wipe,
    DATA_OPENING, DATA_PARTITION, INSTALL_UNFINISHED, SD_CARD, USB_DRIVE, rom_downloaded,
};
use crate::charging;
use skyward_ui::settings::Keeper;
use skyward_ui::surface::kms::KmsSurface;
use skyward_ui::{Canvas, Fonts, Lang, Surface};
use skyward_vab::{Merge, Standing};
use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long init may take to hand the USB port to another function: the
/// program behind the new one has to start and get ready first.
const USB_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the page shows that the work the system asked for is done
/// before the phone starts the system or powers off.
const LEAVE_AFTER_MS: u64 = 1500;

/// How long after the screen starts a touch panel may still come up; with
/// none by then, the pages are worked with the keys, their light shown.
const TOUCH_WAIT: Duration = Duration::from_secs(5);

/// How long the first page takes to come up out of the dark at the start.
const APPEAR: Duration = Duration::from_millis(400);

/// How long a panel gone dark rests, ready to light again at once, before
/// the display turns off; the phone's own system keeps its panel dozing
/// about as long after the screen goes off.
const REST: Duration = Duration::from_secs(5);

/// Where logs saved to the temporary folder go, in the recovery service's
/// own folder, and where in internal storage the others do.
const TMP_LOGS: &str = "/skyward/run/logs";
const STORAGE_LOGS: &str = "Skyward/logs";

/// The folder of internal storage the system saves its own screenshots in,
/// which the gallery shows as its screenshots, and where in the recovery
/// service's own folder a screenshot waits for internal storage to open.
const SCREENSHOTS: &str = "DCIM/Screenshots";
const HELD_SCREENSHOTS: &str = "/skyward/run/screenshots";

/// The panel backlight under /sys/class/backlight.
struct Backlight {
    path: PathBuf,
    /// The raw level at the top of the slider: the panel's normal maximum.
    top: u32,
    /// The raw level last written; at first the one the bootloader left.
    level: u32,
}

impl Backlight {
    fn open() -> Result<Self> {
        let mut dirs: Vec<PathBuf> = fs::read_dir("/sys/class/backlight")
            .context("read /sys/class/backlight")?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<_>>()?;
        dirs.sort();
        // Qualcomm panels expose panel0-backlight; take it when present.
        let dir = dirs
            .iter()
            .find(|d| d.file_name().is_some_and(|n| n == "panel0-backlight"))
            .or(dirs.first())
            .ok_or_else(|| anyhow!("no backlight device"))?
            .clone();
        let read = |name: &str| -> Result<u32> {
            fs::read_to_string(dir.join(name))
                .with_context(|| format!("read {}/{name}", dir.display()))?
                .trim()
                .parse()
                .with_context(|| format!("parse {name}"))
        };
        let top = normal_max(read("max_brightness")?)?;
        let level = read("brightness")?;
        Ok(Backlight { path: dir.join("brightness"), top, level })
    }

    /// Light the panel at `level` of its normal maximum, or turn it off
    /// with `None`. The file is written only when the raw level changes.
    fn show(&mut self, level: Option<f32>) -> Result<()> {
        let raw = level.map_or(0, |v| ((v.clamp(0.0, 1.0) * self.top as f32).round() as u32).max(1));
        if raw != self.level {
            fs::write(&self.path, raw.to_string()).with_context(|| format!("write {}", self.path.display()))?;
            eprintln!("backlight {raw}");
            self.level = raw;
        }
        Ok(())
    }
}

/// How a panel's node in the device tree may give its normal maximum: the
/// property holding it, and the one holding the whole range on the same
/// scale. The 13 Ultra's panel gives both on a finer scale of its own that
/// spans the same light as the backlight's range; the 17 Ultra's gives them
/// on the backlight's own scale. The panels of the Xiaomi 13 and 15, of the
/// 14 and the K80 series and one of the K90's give it only as the level
/// their factory tests drive the panel to, on the backlight's own scale:
/// the 13 Ultra's panels give that level as well, and it comes to the same
/// half of their range as the normal maximum they give.
const NORMAL_MAX: [(&str, &str); 3] = [
    ("mi,normal-max-brightness-clone", "mi,max-brightness-clone"),
    ("mi,max-brightness-normal", "qcom,mdss-dsi-bl-max-level"),
    ("mi,mdss-dsi-fac-bl-max-level", "qcom,mdss-dsi-bl-max-level"),
];

/// The raw backlight level, out of `max`, at the panel's normal maximum.
/// Above it the panel runs in high brightness mode, which the stock system
/// keeps for strong sunlight and its slider does not reach. The panel the
/// display driver booted with gives that maximum in the device tree.
fn normal_max(max: u32) -> Result<u32> {
    let param = "/sys/module/msm_drm/parameters/dsi_display0";
    let boot = fs::read_to_string(param).with_context(|| format!("read {param}"))?;
    // The panel's node, then its configuration after a colon, as in
    // "qcom,mdss_dsi_m1_42_02_0a_dsc_cmd:"; the driver takes no name
    // without the colon either.
    let (name, _) = boot.trim().split_once(':').with_context(|| format!("{param} names no panel: {boot:?}"))?;
    let soc = Path::new("/proc/device-tree/soc");
    let node = fs::read_dir(soc)
        .with_context(|| format!("read {}", soc.display()))?
        .map(|e| e.map(|e| e.path().join(name)))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .find(|p| p.is_dir())
        .ok_or_else(|| anyhow!("panel {name} is not in the device tree"))?;
    let cell = |prop: &str| -> Result<u32> {
        let path = node.join(prop);
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let cell: [u8; 4] = bytes.as_slice().try_into().map_err(|_| anyhow!("{} is not one cell", path.display()))?;
        Ok(u32::from_be_bytes(cell))
    };
    let Some((normal, full)) = NORMAL_MAX.iter().find(|(normal, _)| node.join(normal).exists()) else {
        // A panel whose device tree keeps no brightness table, as Motorola's
        // own panels do not: the backlight's maximum is the slider's top.
        return Ok(max);
    };
    let (normal, full) = (cell(normal)?, cell(full)?);
    ensure!(normal > 0 && normal <= full, "panel {name}: normal maximum {normal} out of {full}");
    Ok((f64::from(max) * f64::from(normal) / f64::from(full)).round() as u32)
}

/// Leave recovery for the chosen target.
fn power(target: Target) -> Result<()> {
    skyward_sys::power(match target {
        Target::System => Power::System,
        Target::Recovery => Power::Recovery,
        Target::Fastboot => Power::Fastboot,
        Target::Bootloader => Power::Bootloader,
        Target::Edl => Power::Edl,
        Target::PowerOff => Power::Off,
    })
}

/// File transfer: the service offering internal storage to a computer over
/// USB, whether it offers the storage now, and the zone, by its place in
/// the settings, it tells the times of files in.
struct Transfer {
    service: skyward_mtp::Service,
    offered: bool,
    zone: usize,
}

impl Transfer {
    /// Serve file transfer; the port offers it from when it next rests.
    fn start(cfg: &DeviceConfig, info: &skyward_info::DeviceInfo, zone: usize) -> Result<Transfer> {
        let device = skyward_mtp::Device {
            manufacturer: cfg.device.brand.clone(),
            model: info.model.clone(),
            version: format!("Skyward {}", skyward_info::VERSION),
            serial: skyward_sys::prop("ro.serialno").unwrap_or_default(),
            name: info.model.clone(),
            battery: Box::new(|| skyward_info::battery_now().ok().flatten().map(|b| b.level)),
        };
        let service = skyward_mtp::Service::start(device, skyward_ui::fmt::rules(zone).clone())?;
        skyward_sys::usb::set_mtp(true);
        Ok(Transfer { service, offered: false, zone })
    }

    /// Offer internal storage, named for the computer in `lang`, or take it
    /// back. Once this returns, nothing the computer had under way in it
    /// goes on.
    fn offer(&mut self, offer: bool, lang: Lang) -> Result<()> {
        if offer != self.offered {
            let description = skyward_ui::fmt::storage_name(lang).into();
            let storage = offer.then(|| skyward_mtp::Storage { top: PathBuf::from(skyward_crypto::data::SDCARD), description });
            self.service.offer(storage)?;
            self.offered = offer;
        }
        Ok(())
    }

    /// Stop serving: a port at rest goes back to adb alone first.
    fn stop(self, rests: bool) -> Result<()> {
        skyward_sys::usb::set_mtp(false);
        let switched = if rests { skyward_sys::usb::switch("adb", USB_TIMEOUT) } else { Ok(()) };
        let stopped = self.service.stop();
        switched.and(stopped)
    }
}

/// Start file transfer where its switch is on, saying on the page why where
/// it does not.
fn start_transfer(cfg: &DeviceConfig, info: &skyward_info::DeviceInfo, app: &mut App, zone: usize) -> Option<Transfer> {
    if !app.state.file_transfer {
        return None;
    }
    match Transfer::start(cfg, info, zone) {
        Ok(t) => Some(t),
        Err(e) => {
            let _ = skyward_sys::kmsg(&format!("file transfer did not start: {e:#}"));
            app.file_transfer_failed(false, &skyward_ui::fmt::transfer_failed(app.state.lang, &format!("{e:#}")), monotonic_ms());
            None
        }
    }
}

/// Give the port up for work that takes it for itself: file transfer stops,
/// rather than only letting internal storage go, because a service left
/// running binds the function it serves and the port reconfigures under the
/// work that took it -- a package on its way to an install ends with the
/// computer's stream cut.
fn stop_transfer(transfer: &mut Option<Transfer>) -> Result<()> {
    match transfer.take() {
        Some(t) => t.stop(false),
        None => Ok(()),
    }
}

/// Take internal storage back from file transfer, if it is on, before work
/// that erases or rewrites the data partition, or installs; whether that
/// work can start, which the page is told otherwise.
fn withhold(transfer: &mut Option<Transfer>, app: &mut App, now: u64) -> Result<bool> {
    let Some(t) = transfer else { return Ok(true) };
    match t.offer(false, app.state.lang) {
        Ok(()) => Ok(true),
        Err(e) => {
            skyward_sys::kmsg(&format!("file transfer kept internal storage: {e:#}"))?;
            app.report(&skyward_ui::fmt::transfer_kept(app.state.lang, &format!("{e:#}")), now);
            Ok(false)
        }
    }
}

/// Whether the phone's fastbootd runs, as init tells it, and how far a
/// computer has the port, as the USB controller tells it.
fn fastboot_status() -> Result<(bool, UsbLink)> {
    let running = skyward_sys::prop("init.svc.fastbootd").as_deref() == Some("running");
    Ok((running, usb_link()?))
}

/// Light the panel, as on a cable going in or out, when the battery as it
/// `was` and as it `is` disagree on whether a charger or a computer powers
/// the phone.
fn plug(app: &mut App, was: &Option<skyward_info::Battery>, is: &Option<skyward_info::Battery>, now: u64) -> Result<()> {
    let plugged = |b: &Option<skyward_info::Battery>| b.as_ref().is_some_and(|b| b.plugged);
    if plugged(was) != plugged(is) {
        skyward_sys::kmsg(if plugged(is) { "a charger or a computer powers the phone" } else { "the phone runs on its battery" })?;
        app.plugged(now, plugged(is));
    }
    Ok(())
}

/// How far a computer has the USB port, as the port's controller tells.
fn usb_link() -> Result<UsbLink> {
    let controller = skyward_sys::prop("sys.usb.controller").context("init names no USB controller")?;
    let path = Path::new("/sys/class/udc").join(controller).join("state");
    let state = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(match state.trim() {
        "configured" => UsbLink::Connected,
        "not attached" => UsbLink::Unplugged,
        _ => UsbLink::Connecting,
    })
}

/// What a request from misc shows on the pages that answer it.
fn asked(request: &bcb::Request) -> Asked {
    Asked {
        reason: request.reason.clone(),
        shutdown_after: request.shutdown_after,
        unhandled: match &request.work {
            Work::Unsupported(what) => Some(what.clone()),
            _ => None,
        },
    }
}

/// A folder being read on a thread of its own, so that a slow one never
/// holds the pages.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ListingTask {
    Files(String),
    Place(Place, String),
}

struct Listing {
    rx: mpsc::Receiver<(ListingTask, Result<Vec<FileRow>, String>, i64)>,
}

/// Start reading `task`'s folder, replacing a listing still under way: its
/// answer is no longer wanted, and the thread it runs on is left to end.
fn start_listing(listing: &mut Option<Listing>, task: ListingTask) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let work = task.clone();
    std::thread::Builder::new().name("folder-listing".into()).spawn(move || {
        let files = match &work {
            ListingTask::Files(folder) => read_folder(Path::new(folder)),
            ListingTask::Place(place, folder) => read_folder(&in_place(*place, folder)),
        }
        .map_err(|e| format!("{e:#}"));
        let _ = tx.send((work, files, unix_now().unwrap_or(0)));
    })?;
    *listing = Some(Listing { rx });
    Ok(())
}

/// Where `path`, from the top of `place`, is: internal storage is the
/// owner's shared storage, where the data partition shows it.
fn in_place(place: Place, path: &str) -> PathBuf {
    let top = match place {
        Place::Storage => Path::new(skyward_crypto::data::SDCARD),
        Place::UsbDrive => Path::new(USB_DRIVE),
        Place::SdCard => Path::new(SD_CARD),
    };
    if path.is_empty() {
        top.to_path_buf()
    } else {
        top.join(path)
    }
}

/// Save the logs chosen into a folder of their own in `into`, named by when
/// they were saved in the zone the settings chose: the recovery's own, with
/// what update_engine wrote while it last installed an update and what the
/// last install script wrote, and the kernel's. Each file is owned as a
/// copy into the folder would be. Gives the folder.
fn export_logs(recovery: bool, kernel: bool, into: &Path, zone: usize) -> Result<PathBuf> {
    let there = into.ancestors().find(|p| p.exists()).context("the root is always there")?;
    let ownership = ownership(there)?;
    let folder = into.join(skyward_ui::fmt::stamp(unix_now()?, zone));
    skyward_files::make_folders(&folder, ownership)?;
    let save = |name: &str, contents: &[u8]| -> Result<()> {
        let path = folder.join(name);
        let mut file = skyward_files::make_file(&path, ownership)?;
        file.write_all(contents).with_context(|| format!("write {}", path.display()))?;
        file.sync_all().with_context(|| format!("write {}", path.display()))
    };
    if recovery {
        save("recovery.log", &fs::read(skyward_sys::LOG).with_context(|| format!("read {}", skyward_sys::LOG))?)?;
        for (name, path) in [("update_engine.log", skyward_install::ENGINE_LOG), ("install-script.log", skyward_install::SCRIPT_LOG)] {
            match fs::read(path) {
                Ok(log) => save(name, &log)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("read {path}")),
            }
        }
    }
    if kernel {
        save("kernel.log", skyward_sys::kernel_log()?.as_bytes())?;
    }
    Ok(folder)
}

/// Save `frame`, what the panel shows, as a picture named for when it was
/// taken: into the gallery's screenshots where internal storage is open, as
/// `storage` says, and otherwise into the recovery's own memory until it
/// opens. Each is made as the system makes what it saves there.
fn save_screenshot(frame: &Canvas, storage: bool, zone: usize) -> Result<Shot> {
    let (folder, shot) = match storage {
        true => (Path::new(skyward_crypto::data::SDCARD).join(SCREENSHOTS), Shot::Album),
        false => (PathBuf::from(HELD_SCREENSHOTS), Shot::Held),
    };
    let there = folder.ancestors().find(|p| p.exists()).context("the root is always there")?;
    let ownership = ownership(there)?;
    skyward_files::make_folders(&folder, ownership)?;
    let path = folder.join(skyward_ui::fmt::screenshot_name(unix_ms_now()?, zone));
    let file = skyward_files::make_file(&path, ownership)?;
    let mut out = std::io::BufWriter::new(&file);
    frame.write_png(&mut out).with_context(|| format!("write {}", path.display()))?;
    out.flush().with_context(|| format!("write {}", path.display()))?;
    drop(out);
    file.sync_all().with_context(|| format!("write {}", path.display()))?;
    skyward_sys::kmsg(&format!("screenshot saved as {}", path.display()))?;
    Ok(shot)
}

/// Move the screenshots held while internal storage was closed into the
/// gallery's, now that it is open: each keeps its name, and is made there
/// as the system makes what it saves.
fn keep_held_screenshots() -> Result<()> {
    let held = Path::new(HELD_SCREENSHOTS);
    let shots = match fs::read_dir(held) {
        Ok(entries) => entries.map(|e| e.map(|e| e.path())).collect::<std::io::Result<Vec<_>>>().with_context(|| format!("read {}", held.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("read {}", held.display())),
    };
    if shots.is_empty() {
        return Ok(());
    }
    let album = Path::new(skyward_crypto::data::SDCARD).join(SCREENSHOTS);
    let there = album.ancestors().find(|p| p.exists()).context("the root is always there")?;
    let ownership = ownership(there)?;
    skyward_files::make_folders(&album, ownership)?;
    let stop = std::sync::atomic::AtomicBool::new(false);
    skyward_files::move_into(&shots, &album, skyward_files::Clash::KeepBoth, ownership, &stop, &mut |_| {})?;
    skyward_sys::kmsg(&format!("{} screenshots held in {} moved into {}", shots.len(), held.display(), album.display()))
}

/// The folder backups go in and are found in, in `place`.
fn backups_in(place: Place) -> PathBuf {
    in_place(place, skyward_backup::FOLDER)
}

/// The backups in `place`: each folder in its backups folder, with what its
/// manifest records or why that cannot be read. A place with no backups
/// folder has none.
fn backups_at(place: Place) -> Result<Vec<BackupEntry>> {
    let folder = backups_in(place);
    let entries = match fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("read {}", folder.display())),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("read {}", folder.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.file_type().with_context(|| format!("look at {}", entry.path().display()))?.is_dir() {
            continue;
        }
        let held = skyward_backup::Manifest::read(&entry.path()).map(|m| held(&m)).map_err(|e| format!("{e:#}"));
        found.push(BackupEntry { place, name, held });
    }
    Ok(found)
}

/// What a backup's manifest records, as the pages show it.
fn held(m: &skyward_backup::Manifest) -> Held {
    Held {
        made: m.made,
        size: m.size(),
        parts: m.parts(),
        device: m.device.clone(),
        system: m.system.clone(),
        slot: skyward_vab::Slot::from_suffix(&m.slot),
    }
}

/// Look for backups in the places asked, the restore page showing what is
/// found and saying why a place could not be looked in.
fn find_backups(app: &mut App, storage: bool, usb: bool, sdcard: bool, now: u64) {
    let mut found = Vec::new();
    for (place, look) in [(Place::Storage, storage), (Place::UsbDrive, usb), (Place::SdCard, sdcard)] {
        if look {
            match backups_at(place) {
                Ok(mut there) => found.append(&mut there),
                Err(e) => app.report(&format!("{e:#}"), now),
            }
        }
    }
    app.set_backups(found);
}

/// Delete the backup in `folder` of `place`; a USB drive, shown read-only,
/// is made writable for it and read-only again after.
fn delete_backup(place: Place, folder: &Path) -> Result<()> {
    let drive = Path::new(USB_DRIVE);
    if place == Place::UsbDrive {
        skyward_block::mount::remount_writable(drive, true)?;
    }
    let deleted = fs::remove_dir_all(folder).with_context(|| format!("delete {}", folder.display()));
    if place == Place::UsbDrive {
        let read_only = skyward_block::mount::remount_writable(drive, false);
        deleted?;
        return read_only;
    }
    deleted
}

/// How backing up or restoring ended, as its page tells it.
fn backup_end(end: backup::End) -> BackupEnd {
    match end {
        backup::End::Saved(folder) => {
            BackupEnd::Saved(folder.file_name().expect("a backup's folder has a name").to_string_lossy().into_owned())
        }
        backup::End::Restored { unmade } => BackupEnd::Restored { unmade: unmade.iter().map(|p| p.display().to_string()).collect() },
        backup::End::Stopped => BackupEnd::Stopped,
        backup::End::Refused(refusal) => BackupEnd::Refused(match refusal {
            backup::Refusal::Room { needed, free } => BackupRefusal::Room { needed, free },
            backup::Refusal::Update(state) => BackupRefusal::Update(state),
            backup::Refusal::OtherPhone(device) => BackupRefusal::OtherPhone(device),
            backup::Refusal::Resized(name) => BackupRefusal::Resized(name),
            backup::Refusal::DataReset => BackupRefusal::DataReset,
        }),
        backup::End::Failed(why) => BackupEnd::Failed(why),
    }
}

/// Now, in Unix seconds.
fn unix_now() -> Result<i64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH).context("the clock is before 1970")?.as_secs() as i64)
}

/// Now, in Unix milliseconds.
fn unix_ms_now() -> Result<i64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH).context("the clock is before 1970")?.as_millis() as i64)
}

/// A medium plugged in, and whether it is shown at its place or why it could
/// not be.
struct Plugged {
    disk: String,
    shown: Result<(), String>,
}

/// Where a removable place is shown, and what its findings are logged as.
fn medium_at(place: Place) -> &'static Path {
    Path::new(match place {
        Place::UsbDrive => USB_DRIVE,
        Place::SdCard => SD_CARD,
        Place::Storage => unreachable!("internal storage is not a medium"),
    })
}

fn medium_name(place: Place) -> &'static str {
    match place {
        Place::UsbDrive => "USB drive",
        Place::SdCard => "SD card",
        Place::Storage => unreachable!("internal storage is not a medium"),
    }
}

/// The disks `place`'s medium may be, as the kernel lists them. A phone whose
/// config names no controller for the card has no tray to look in.
fn medium_disks(place: Place, controller: Option<&str>) -> Result<Vec<Disk>> {
    match place {
        Place::UsbDrive => usb::drives(),
        Place::SdCard => match controller {
            Some(host) => skyward_block::card::cards(host),
            None => Ok(Vec::new()),
        },
        Place::Storage => unreachable!("internal storage is not a medium"),
    }
}

/// Show `disk` at `place`: read only on a drive, read and written on the card.
fn medium_mount(place: Place, disk: &Disk) -> Result<(String, &'static str)> {
    match place {
        Place::UsbDrive => usb::mount(disk, medium_at(place)),
        Place::SdCard => skyward_block::card::mount(disk, medium_at(place)),
        Place::Storage => unreachable!("internal storage is not a medium"),
    }
}

/// Look for the medium at `place`: one gone is taken away from where it
/// was shown, even with files on it still open, and one found in two looks
/// running is shown, the kernel having listed its partitions by then. `seen`
/// is a disk found by the look before that is not shown yet. What the pages
/// are told comes back when it changed.
fn look_for_medium(
    place: Place,
    controller: Option<&str>,
    plugged: &mut Option<Plugged>,
    seen: &mut Option<String>,
) -> Result<Option<Option<Medium>>> {
    let found = medium_disks(place, controller)?.into_iter().find(Disk::ready);
    if let Some(p) = plugged {
        if found.is_some_and(|d| d.disk == p.disk) {
            return Ok(None);
        }
        if p.shown.is_ok() {
            skyward_block::mount::umount_detach(medium_at(place))?;
        }
        skyward_sys::kmsg(&format!("{} {} is gone", medium_name(place), p.disk))?;
        *plugged = None;
        *seen = None;
        return Ok(Some(None));
    }
    let Some(disk) = found else {
        *seen = None;
        return Ok(None);
    };
    if seen.as_deref() != Some(disk.disk.as_str()) {
        *seen = Some(disk.disk.clone());
        return Ok(None);
    }
    let shown = match medium_mount(place, &disk) {
        Ok((part, filesystem)) => {
            skyward_sys::kmsg(&format!("{} {part} shown at {} as {filesystem}", medium_name(place), medium_at(place).display()))?;
            Ok(())
        }
        Err(e) => {
            skyward_sys::kmsg(&format!("{} {} not shown: {e:#}", medium_name(place), disk.disk))?;
            Err(format!("{e:#}"))
        }
    };
    let told = match &shown {
        Ok(()) => Medium::Shown,
        Err(why) => Medium::Unreadable(why.clone()),
    };
    *plugged = Some(Plugged { disk: disk.disk, shown });
    *seen = None;
    Ok(Some(Some(told)))
}

/// What a choice on the clash page asks of the work on files.
fn clash_rule(choice: ClashChoice) -> skyward_files::Clash {
    match choice {
        ClashChoice::Replace => skyward_files::Clash::Replace,
        ClashChoice::Skip => skyward_files::Clash::Skip,
        ClashChoice::KeepBoth => skyward_files::Clash::KeepBoth,
    }
}

/// What a copy or move into the folder `into` gives the entries it makes.
/// On the data partition each takes the owner, group and label the folder
/// gives its own, as the system makes them there; in shared storage, the
/// folder's permissions as well. Elsewhere each keeps what it had.
fn ownership(into: &Path) -> Result<Ownership> {
    use skyward_crypto::data;
    let at = Path::new(data::MOUNT);
    if !skyward_block::mount::is_mount_point(at)? {
        return Ok(Ownership::Kept);
    }
    let here = fs::metadata(into).with_context(|| format!("look at {}", into.display()))?;
    if here.dev() != fs::metadata(at).with_context(|| format!("look at {}", at.display()))?.dev() {
        return Ok(Ownership::Kept);
    }
    let real = fs::canonicalize(into).with_context(|| format!("find {}", into.display()))?;
    let shared = [data::SHARED_STORAGE, data::SDCARD].iter().any(|s| real.starts_with(s));
    Ok(if shared { Ownership::Shared } else { Ownership::Folder })
}

/// The page's name for a part erasing goes through.
fn part(step: Step) -> Part {
    match step {
        Step::Data => Part::Data,
        Step::Cache => Part::Cache,
        Step::Metadata => Part::Metadata,
        Step::AppData => Part::AppData,
        Step::Media => Part::Media,
        Step::AppCaches => Part::AppCaches,
        Step::SystemCaches => Part::SystemCaches,
        Step::Art => Part::Art,
        Step::Logs => Part::Logs,
        Step::Updates => Part::Updates,
    }
}

/// The part of the phone a choice of the wipe page other than everything
/// erases.
fn step(choice: Wipe) -> Step {
    match choice {
        Wipe::AppData => Step::AppData,
        Wipe::Media => Step::Media,
        Wipe::AppCaches => Step::AppCaches,
        Wipe::SystemCaches => Step::SystemCaches,
        Wipe::Art => Step::Art,
        Wipe::Cache => Step::Cache,
        Wipe::Logs => Step::Logs,
        Wipe::Updates => Step::Updates,
        Wipe::All => unreachable!("everything is a factory reset, not a part of one"),
    }
}

/// How erasing stopped, as its page tells it.
fn erase_stop(stop: wipe::Stop) -> EraseStop {
    match stop {
        wipe::Stop::Refused(wipe::Refusal::Update(hold)) => EraseStop::Update(hold),
        wipe::Stop::Refused(wipe::Refusal::NotRebuilt) => EraseStop::NotRebuilt,
        wipe::Stop::Refused(wipe::Refusal::NoCache) => EraseStop::NoCache,
        wipe::Stop::Closed(why) => EraseStop::Closed(why),
        wipe::Stop::Locked(users) => EraseStop::Locked(users),
        wipe::Stop::Failed(step, e) => EraseStop::Failed(part(step), format!("{e:#}")),
    }
}

/// Start erasing all user data, its page following it; why it cannot
/// start ends it at once, on the same page. `asked` when the system asked
/// for it; `reformat` the filesystem every part is made afresh as, where
/// one was asked for.
fn factory_reset(app: &mut App, keep_memtag_mode: bool, asked: bool, reformat: Option<String>) -> Result<Eraser> {
    match wipe::factory_reset(reformat) {
        Ok(plan) => {
            app.erase_started(true, asked, plan.steps().into_iter().map(part).collect());
            Eraser::start(move |tell| plan.carry_out(keep_memtag_mode, tell))
        }
        Err(stop) => {
            app.erase_started(true, asked, Vec::new());
            Eraser::stopped(stop)
        }
    }
}

/// Start erasing all user data for rescue, the installing page saying so
/// meanwhile: as a factory reset erases it, the memory tagging asked of the
/// bootloader forgotten too.
fn rescue_wipe(app: &mut App) -> Result<Eraser> {
    app.rescue_wiping();
    match wipe::factory_reset(None) {
        Ok(plan) => Eraser::start(move |tell| plan.carry_out(false, tell)),
        Err(stop) => Eraser::stopped(stop),
    }
}

/// Start erasing parts on their own, its page following it: parts of the
/// data partition, and the cache partition, made afresh as `reformat`
/// where the system asked for that filesystem; `logs` are the phone's own
/// log folders. `asked` when the system asked for it.
fn parts_wipe(app: &mut App, steps: Vec<Step>, logs: &[String], asked: bool, reformat: Option<String>) -> Result<Eraser> {
    match wipe::parts(steps, logs, reformat) {
        Ok(parts) => {
            app.erase_started(false, asked, parts.steps().into_iter().map(part).collect());
            Eraser::start(move |tell| parts.carry_out(tell))
        }
        Err(stop) => {
            app.erase_started(false, asked, Vec::new());
            Eraser::stopped(stop)
        }
    }
}

/// The reasons the bootloader gives for a start that followed the kernel
/// stopping, as Android's own recovery lists them; it does not start an
/// install the system asked for before such a start.
const PANICS: [&str; 2] = ["kernel_panic", "Panic"];

/// Whether the phone came to the recovery after its kernel stopped.
pub fn panicked() -> bool {
    skyward_sys::prop("ro.boot.bootreason").is_some_and(|reason| PANICS.iter().any(|p| p.eq_ignore_ascii_case(&reason)))
}

/// An install the system asked for that waits for the data partition to
/// open, or for the user whose own files hold the package to unlock them.
struct Pending {
    package: asked::Asked,
    needs: Needs,
    retry: bool,
}

/// Start installing the package the system asked for.
fn install_asked(cfg: &DeviceConfig, app: &App, pending: Pending) -> Result<Installer> {
    if pending.needs == Needs::Cache {
        let cache = wipe::cache()?.context("the phone has no cache partition")?;
        let at = Path::new(&cache.mount_point);
        if !mount::is_mount_point(at)? {
            fs::create_dir_all(at).with_context(|| format!("make {}", at.display()))?;
            mount::mount_ro(Path::new(&fstab::device(&cache)?), at)?;
        }
    }
    Installer::asked(pending.package, pending.retry, detach(cfg.services.clone()), install_options(app))
}

/// Where a reboot rescue asked for goes: the target, and whether it is
/// rescue again, which the recovery is told in misc.
fn rescue_target(target: &str) -> (Target, bool) {
    match target {
        "bootloader" => (Target::Bootloader, false),
        "fastboot" => (Target::Fastboot, false),
        "recovery" => (Target::Recovery, false),
        "rescue" => (Target::Recovery, true),
        // As minadbd has it, anything else starts the system.
        _ => (Target::System, false),
    }
}

/// How a partition's work ended, as its page tells it.
fn item_end(item: partwork::Item) -> ItemEnd {
    match item {
        partwork::Item::Erased => ItemEnd::Erased,
        partwork::Item::Unshared => ItemEnd::Unshared,
        partwork::Item::NotShared(filesystem) => ItemEnd::NotShared(filesystem),
        partwork::Item::Fixed(Fixed::Clean) => ItemEnd::Clean,
        partwork::Item::Fixed(Fixed::Fixed) => ItemEnd::Fixed,
        partwork::Item::Made => ItemEnd::Made,
        // What the program said shows below.
        partwork::Item::Fixed(Fixed::Failed) => ItemEnd::Failed(None),
        partwork::Item::Failed(why) => ItemEnd::Failed(Some(why)),
    }
}

/// Why work on whole partitions stopped, as its page tells it.
fn part_stop(stop: partwork::Stop) -> PartStop {
    match stop {
        partwork::Stop::Refused(refusal) => PartStop::Refused(refusal),
        partwork::Stop::Unmounted(base) => PartStop::Unmounted(base),
        partwork::Stop::Failed(why) => PartStop::Failed(why),
    }
}

/// The partitions the filesystem page lists, in this order, where the
/// phone's table has them: the data partition, /metadata and the cache
/// partition. Each comes with what it holds now, its size, and the other
/// filesystems the table lists for it that the recovery makes. The data
/// partition reads only through its decryption, so what it holds is known
/// only once it is open; until then, any of the table's filesystems may be
/// it, and where the table lists one, it is that. /metadata keeps the key
/// the data partition opens with, which making it afresh would lose, so it
/// is offered no other.
fn read_filesystems() -> Result<Vec<FsRow>> {
    let mut rows = Vec::new();
    for (name, point) in [(DATA_PARTITION, "/data"), ("metadata", "/metadata"), ("cache", "/cache")] {
        let entries = fstab::read(point)?;
        let Some(entry) = entries.first() else { continue };
        let device = fstab::device(entry)?;
        let size = skyward_block::size_bytes(&device)?;
        let mut listed: Vec<String> = Vec::new();
        for e in &entries {
            if REFORMATS.contains(&e.filesystem.as_str()) && !listed.contains(&e.filesystem) {
                listed.push(e.filesystem.clone());
            }
        }
        let filesystem = match name {
            DATA_PARTITION => mount::visible(Path::new(point))?.map(|v| v.filesystem),
            _ => probe::filesystem(Path::new(&device))?,
        };
        let others = match (name, &filesystem) {
            ("metadata", _) => Vec::new(),
            (_, Some(held)) => listed.into_iter().filter(|fs| fs != held).collect(),
            (_, None) if listed.len() > 1 => listed,
            (_, None) => Vec::new(),
        };
        rows.push(FsRow { name: name.to_string(), mount_point: point.to_string(), filesystem, size, others });
    }
    Ok(rows)
}

/// The filesystem page's partition `name` for its work: the data partition,
/// or the table's first entry for another.
fn fs_part(name: &str) -> Result<partwork::Part> {
    if name == DATA_PARTITION {
        return Ok(partwork::Part::Data);
    }
    let point = format!("/{name}");
    let entry = fstab::read(&point)?.into_iter().next().with_context(|| format!("the phone's table has no {point}"))?;
    Ok(partwork::Part::Entry(entry))
}

/// What the pages act on: a touch or a key, what a program asked on the
/// recovery socket, or a USB drive plugged in or pulled out.
enum Incoming {
    /// The page shown first, which may ask for work as it shows.
    Arrived,
    Input(Event),
    Asked(Request),
    /// A removable place's medium found or gone: where it is, and what is
    /// there now.
    Medium(Place, Option<Medium>),
}

/// What a request on the recovery socket comes to. adbd asks for fastboot
/// while the slot's partitions are up, which fastbootd cannot work with, so
/// the phone restarts into fastboot as from the reboot page; fastbootd asks
/// for the recovery it runs in. A request that finds the recovery already
/// where it asks to be, as when the keys left fastboot first, needs nothing.
fn answer(request: Request, fastboot: bool) -> Vec<Effect> {
    match (request, fastboot) {
        (Request::Fastboot, false) => vec![Effect::Reboot(Target::Fastboot)],
        (Request::Recovery, true) => vec![Effect::EnterRecovery],
        _ => Vec::new(),
    }
}

/// Where fastbootd asked the phone to go, from what it left in the
/// bootloader message: the recovery it runs in for a `reboot recovery`, and
/// the bootloader or fastboot itself for the other two. A message that asks
/// for none of them -- an empty one, or whatever the phone booted with --
/// leaves the phone where it is.
fn left_for(asked: &bcb::Bcb) -> Option<Target> {
    match asked.command.as_str() {
        "" => None,
        "bootonce-bootloader" => Some(Target::Bootloader),
        "boot-fastboot" => Some(Target::Fastboot),
        _ => Some(Target::Recovery),
    }
}

/// The time of day for the status bar, in the zone the settings chose.
fn clock(zone: usize) -> Result<String> {
    Ok(skyward_ui::fmt::clock(unix_now()?, zone))
}

/// Tell the kernel log how the data partition was mounted, where opening
/// it mounted it: the check made first, in full where it found something,
/// and the device it is read through.
fn log_mounted(mounted: &Opened) -> Result<()> {
    if let Some(checked) = &mounted.checked {
        skyward_sys::kmsg(&format!("the check of {} ended with {}", mounted.mapped.display(), checked.status))?;
        if checked.status != 0 {
            for line in checked.said.lines() {
                skyward_sys::kmsg(line)?;
            }
        }
    }
    skyward_sys::kmsg(&format!("{} holds {} at {}", mounted.mapped.display(), mounted.filesystem, skyward_crypto::data::MOUNT))
}

/// Show what the data partition still keeps closed: the first user whose
/// own files wait on a screen lock they set themselves, with the control
/// that lock is answered with, or nothing. Users whose credential the phone
/// made up open behind the owner and are never asked; one that did not
/// open goes to the kernel log with why.
fn show_closed(app: &mut App, closed: &[Closed]) -> Result<()> {
    use skyward_crypto::user::Lock;
    skyward_sys::kmsg(&format!("the users whose own files are still closed: {:?}", closed.iter().map(|c| c.user).collect::<Vec<_>>()))?;
    for c in closed {
        if let Some(why) = &c.why {
            skyward_sys::kmsg(&format!("the files of user {} stay closed: {why}", c.user))?;
        }
    }
    let waiting = closed.iter().find(|c| !c.made_up).map(|c| {
        let keys = match c.lock {
            Lock::PATTERN => Keys::Pattern,
            Lock::PIN => Keys::Digits,
            _ => Keys::Letters,
        };
        (c.user, keys)
    });
    app.data_waiting(waiting);
    Ok(())
}

/// What the file manager lists for a folder: its entries, folders and
/// links to folders first, then by name ignoring case, each folder with how
/// many entries it holds. An entry gone by the time it is looked at, as
/// processes in /proc go, is left out.
fn read_folder(path: &Path) -> Result<Vec<FileRow>> {
    let gone = |e: &std::io::Error| e.kind() == std::io::ErrorKind::NotFound;
    let mut rows = Vec::new();
    for entry in fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
        let full = entry.with_context(|| format!("read {}", path.display()))?.path();
        let meta = match fs::symlink_metadata(&full) {
            Ok(meta) => meta,
            Err(e) if gone(&e) => continue,
            Err(e) => return Err(e).with_context(|| format!("stat {}", full.display())),
        };
        let kind = if meta.file_type().is_symlink() {
            let target = fs::read_link(&full).with_context(|| format!("read the link {}", full.display()))?;
            // A link to nothing leads to no folder.
            let folder = match fs::metadata(&full) {
                Ok(to) => to.is_dir(),
                Err(e) if gone(&e) => false,
                Err(e) => return Err(e).with_context(|| format!("follow {}", full.display())),
            };
            FileKind::Link { target: target.to_string_lossy().into_owned(), folder }
        } else if meta.is_dir() {
            // A folder that may not be read shows that in place of a count.
            let items = match fs::read_dir(&full) {
                Ok(inside) => Some(inside.count()),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => None,
                Err(e) if gone(&e) => continue,
                Err(e) => return Err(e).with_context(|| format!("read {}", full.display())),
            };
            FileKind::Folder { items }
        } else if meta.is_file() {
            FileKind::File
        } else {
            FileKind::Special
        };
        let name = full.file_name().with_context(|| format!("{} has no name", full.display()))?.to_string_lossy().into_owned();
        rows.push(FileRow { name, kind, size: meta.len(), modified: meta.mtime(), mode: meta.mode(), uid: meta.uid(), gid: meta.gid() });
    }
    rows.sort_by_key(|r| (!matches!(r.kind, FileKind::Folder { .. } | FileKind::Link { folder: true, .. }), r.name.to_lowercase()));
    Ok(rows)
}

/// The slot's logical partitions and how each shows now, for the mount page.
/// A mounted partition is in use when a program holds it, or when one of
/// `services` this phone's build of the hardware runs is run from it: those
/// are started again from there whenever they end.
fn read_mounts(services: &[Service]) -> Result<Vec<skyward_ui::app::MountRow>> {
    use skyward_ui::app::MountRow;
    let slot = skyward_vab::current_slot()?;
    let shown = skyward_vab::mounted_partitions(slot)?;
    let holders = skyward_block::mount::holders()?;
    let props = skyward_info::installed_props(slot)?;
    let services: Vec<&Service> = services.iter().filter(|s| s.runs_on(&props)).collect();
    let mut rows = Vec::new();
    for base in skyward_vab::slot_bases(slot)? {
        let point = skyward_vab::mount_point(&base);
        let name = skyward_vab::partition_name(&base, slot);
        let mounted = shown.iter().find(|m| m.partition == name);
        let mut using = BTreeSet::new();
        if let Some(m) = mounted {
            using.extend(holders.get(&skyward_block::devnum(&m.device)?).into_iter().flatten().cloned());
            for s in services.iter().filter(|s| !s.oneshot && s.uses(&point)) {
                let program = Path::new(&s.path).file_name().with_context(|| format!("{} names no program", s.path))?;
                using.insert(program.to_string_lossy().into_owned());
            }
        }
        rows.push(MountRow {
            base,
            point: point.display().to_string(),
            mounted: mounted.is_some(),
            holders: using.into_iter().collect(),
        });
    }
    Ok(rows)
}

/// What the phone's boot control holds of the slots, or why it cannot be
/// asked.
fn read_slots() -> Result<skyward_vab::BootSlots, String> {
    let read = || -> Result<skyward_vab::BootSlots> {
        let (binder, hardware) = (skyward_binder::driver::Binder::open()?, skyward_binder::driver::Binder::hardware()?);
        let control = skyward_vab::boot::BootControl::open(&binder, &hardware)?;
        control.slots()
    };
    read().map_err(|e| format!("{e:#}"))
}

/// Have the bootloader start `slot` next and give what the boot control
/// holds of the slots then, unless an update is in flight: it set the
/// slot it needs itself.
fn set_slot(slot: skyward_vab::Slot) -> Result<skyward_vab::BootSlots, String> {
    let set = || -> Result<skyward_vab::BootSlots> {
        let state = skyward_vab::snapshot_state()?;
        ensure!(state == skyward_vab::SnapshotState::None, "an update is in flight ({state:?}), and it set the slot it needs");
        // The second system starts only from the slot its boot set was put
        // in, and the installed system's goes back there first.
        skyward_systems::install::ensure_installed()?;
        let (binder, hardware) = (skyward_binder::driver::Binder::open()?, skyward_binder::driver::Binder::hardware()?);
        let control = skyward_vab::boot::BootControl::open(&binder, &hardware)?;
        control.set_active(slot)?;
        skyward_sys::kmsg(&format!("slot {} starts next", slot.letter()))?;
        control.slots()
    };
    set().map_err(|e| format!("{e:#}"))
}

/// What has to happen before an update writes over the installed system's
/// partitions: the files the phone's own programs run from stay in memory,
/// and the booted slot's partitions come off.
fn detach(services: Vec<Service>) -> Detach {
    Box::new(move || {
        let slot = skyward_vab::current_slot()?;
        let mounted = skyward_vab::mounted_partitions(slot)?;
        let busy = services::pin(&services, &mounted)?;
        skyward_vab::release_partitions(slot, &busy)
    })
}

/// What an install does besides installing, as the install page's switches
/// are set: puts Skyward back into the recovery partitions it writes over,
/// on a phone that has them, and checks a zip's signature before its
/// install script runs.
fn install_options(app: &App) -> Options {
    Options { keep: app.state.keep_recovery && app.state.recovery_partitions, signed: app.state.check_signature }
}

/// How often the WLAN page scans while it shows.
const SCAN_EVERY: Duration = Duration::from_secs(10);

/// How long adbd is given to hold the port after init is asked to restart it.
const ADB_WAIT: Duration = Duration::from_secs(3);

fn wlan_worker(w: &Option<wlan::Worker>) -> Result<&wlan::Worker> {
    w.as_ref().context("the phone's WLAN is not brought up")
}

/// Where `build`'s image is downloaded to, in the recovery's own folder,
/// which is in memory: it is written over the recovery once whole.
fn downloaded_build(build: &Build) -> PathBuf {
    Path::new(skyward_sys::RUN).join(&build.name)
}

/// What internal storage holds of the ROM's package.
fn rom_have(app: &App, release: &skyward_net::rom::Release) -> Have {
    have_at(app, Path::new(&rom_downloaded(release)))
}

/// What internal storage holds at `path`; nothing while it cannot be read.
fn have_at(app: &App, path: &Path) -> Have {
    if !app.state.data.storage_open() {
        return Have::Nothing;
    }
    let size = |p: &Path| match fs::metadata(p) {
        Ok(m) => Some(m.len()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            let _ = skyward_sys::kmsg(&format!("read {}: {e}", p.display()));
            None
        }
    };
    match (size(path), size(&download::partial(path))) {
        (Some(whole), _) => Have::Whole(whole),
        (None, Some(part)) => Have::Part(part),
        (None, None) => Have::Nothing,
    }
}

/// Wait until a device, the installer or the recovery socket has something,
/// or `timeout_ms` passes.
fn wait(fds: &[i32], timeout_ms: i32) -> Result<()> {
    let mut polls: Vec<libc::pollfd> = fds.iter().map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
    let n = unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, timeout_ms) };
    if n < 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            return Err(err).context("poll input devices");
        }
    }
    Ok(())
}

/// Run the pages. What `request`, read from misc, asks for decides the
/// first of them; asking for nothing, they start at `start`.
pub fn run(cfg: &DeviceConfig, font: &Path, mono: &Path, request: bcb::Request, start: Screen, dump: Option<&Path>) -> Result<()> {
    if cfg.display.density == 0 {
        return Err(anyhow!("{}: [display] density is required to lay out the UI", cfg.device.codename));
    }
    let mut info = skyward_info::collect(cfg)?;
    let fonts = Fonts::load(font, 0, mono)?;
    let mut surface = KmsSurface::open("/dev/dri/card0")?;
    let (w, h) = surface.size();
    eprintln!("display {}", surface.describe());
    let mut inputs = Inputs::open(w, h, cfg.touch.writes.clone())?;
    let started = Instant::now();
    let mut touch_known = false;
    // adbd and fastbootd ask over it to go over to fastboot and back.
    let control = Control::open()?;
    let mut haptics = cfg.haptics.as_ref().map(Haptics::open).transpose()?;
    let mut backlight = Backlight::open()?;
    let first = match &request.work {
        Work::Fastboot => Screen::Fastboot,
        Work::PromptWipeData => Screen::CantStart,
        Work::Unsupported(_) => Screen::Unhandled,
        Work::Menu
        | Work::Exit
        | Work::Update(_)
        | Work::WipeData { .. }
        | Work::WipeCache { .. }
        | Work::WipeAb { .. }
        | Work::Sideload { .. }
        | Work::Rescue
        | Work::UnshareBlocks
        | Work::SwitchSystem { .. }
        | Work::SystemStopped => start,
    };
    let mut app = App::new(first, w, h, cfg.display.density as f32 / 160.0);
    app.set_screen_corners(cfg.display.corner_radius as f32);
    // The switch the phone's kernel takes for charging, and the database the
    // phone's own system keeps its limit in, where the configuration names
    // them.
    let mut charging = cfg.charging.as_ref().map(|c| charging::Switch::new(&c.switch));
    app.state.charging_available = charging.is_some();
    let charge_db = cfg.charging.as_ref().and_then(|c| c.system.clone());
    let charge_cache = cfg.charging.as_ref().and_then(|c| c.cache.clone());
    // The kernels this phone's own configuration adds to the root page; a
    // phone without any keeps the two methods every build carries.
    #[cfg(feature = "roots")]
    {
        let plugins = cfg
            .root_methods
            .iter()
            .map(|method| match method {
                skyward_config::RootMethod::Kernel { name, subtitle, repo, asset, folder, note, manager_name } => skyward_root::kernel::Plugin {
                    name: name.clone(),
                    subtitle: subtitle.clone(),
                    repo: repo.clone(),
                    asset: asset.clone(),
                    folder: folder.clone(),
                    note: note.clone(),
                    manager_name: manager_name.clone(),
                },
            })
            .collect();
        app.set_kernel_plugins(plugins);
    }
    app.state.lang = Lang::En;
    // The home page tells of a restart into the slot the system does not
    // run from now: an update or a switch left for the next boot.
    if info.slot.is_some() {
        app.set_boot_slots(read_slots());
    }
    app.set_asked(asked(&request));
    // Opening the data partition, or a user's own files in it, under way,
    // and the users whose own files are still closed once it is open.
    let mut decryptor: Option<Decryptor> = None;
    let mut closed: Vec<Closed> = Vec::new();
    let mut arriving = Some(Incoming::Arrived);
    // The mount page shows the partitions as they are when it opens and
    // after each change; why they cannot be read is the page's to say.
    let mounts = || read_mounts(&cfg.services).map_err(|e| format!("{e:#}"));

    let mut screen = Canvas::new(w, h);
    // A page sliding over another draws on three screens' worth of canvas
    // besides the panel's: made ready now, the first slide finds them so.
    Canvas::keep_ready(w, h, 3);
    let mut shown_clock = String::new();
    // The readings are taken again on a thread of their own; those taken
    // here as work ends say how it left the phone, and readings started
    // before them are out of date.
    let readings = Readings::start(cfg)?;
    let mut read_at = Instant::now();
    let mut installer: Option<Installer> = None;
    // APatch's root, whose run writes boot partitions of its own.
    #[cfg(feature = "roots")]
    let mut apatch_worker: Option<crate::apatch::Worker> = None;
    // A KernelSU-family manager's run, which downloads its APK and writes
    // boot partitions of its own.
    #[cfg(feature = "roots")]
    let mut ksu_worker: Option<crate::ksu::Worker> = None;
    // A device's own kernel, whose release is downloaded and written whole.
    #[cfg(feature = "roots")]
    let mut kernel_worker: Option<crate::kernel::Worker> = None;
    // The newest kernel release's version, read for its page.
    #[cfg(feature = "roots")]
    let mut kernel_checker: Option<crate::kernel::Checker> = None;
    #[cfg(feature = "roots")]
    let mut kernel_checker_at = 0;
    #[cfg(feature = "roots")]
    let mut kernel_checker_expired = false;
    // Whether the panel's frame memory has been blacked out for an off
    // panel; cleared once a real frame is shown again.
    let mut blanked = false;
    // When the panel gone dark began to rest.
    let mut rested_at: Option<Instant> = None;
    // Started for fastboot, the port goes to the phone's own fastbootd. The
    // page says whether it came up and how far a computer has the port;
    // why it did not come up goes to the kernel log and on screen.
    let mut fastboot = first == Screen::Fastboot;
    let mut port_at = Instant::now();
    // The settings kept from the starts before are back before the first
    // frame, and before file transfer starts with its switch and the time
    // zone. The pages speak the language chosen then, or else the one the
    // system named with the work it asks for, or the one the installed
    // system is built to start in, or else the one they last spoke.
    let kept = settings::read()?;
    let locale = request.locale.as_deref().or(info.locale.as_deref());
    let named = locale.map(Lang::for_locale).transpose()?;
    app.take_settings(kept.settings(), named);
    // The limit the system kept when the data partition was last open, which
    // is all there is until it opens again.
    if let Some(cache) = &charge_cache {
        if let Some(limit) = charging::cached(Path::new(cache)) {
            app.set_charge_limit(Some(limit));
        }
    }
    let mut keeper = Keeper::new(&kept);
    let mut wlan_wanted = kept.settings().is_some_and(|s| s.wlan);
    skyward_sys::kmsg(&format!(
        "the pages speak {:?}, {}, and the locale named is {}",
        app.state.lang,
        if app.state.language_chosen { "the language chosen" } else { "with none chosen" },
        locale.unwrap_or("none")
    ))?;
    // File transfer runs while its switch is on, and offers internal
    // storage while that is open; one that does not start says why on
    // screen, and the recovery carries on.
    let zone_now = app.state.time_zone;
    let mut transfer: Option<Transfer> = if matches!(request.work, Work::Sideload { .. } | Work::Rescue) {
        // The computer is here to send a package, not to read storage: file
        // transfer would reconfigure the port under the install.
        None
    } else {
        start_transfer(cfg, &info, &mut app, zone_now)
    };
    // A sideload has the port, which goes back to rest once it ends.
    let mut sideloading = false;
    if fastboot {
        if let Err(e) = skyward_sys::usb::switch("fastboot", USB_TIMEOUT) {
            skyward_sys::kmsg(&format!("fastbootd has not taken the port: {e:#}"))?;
            app.report(&format!("{e:#}"), monotonic_ms());
        }
        let (running, link) = fastboot_status()?;
        app.set_fastboot(running, link);
    }
    // Work the request asks for that goes on keeps the request in misc,
    // so a restart halfway comes back to it, until it is done or the user
    // leaves it for somewhere else.
    let misc = bcb::misc();
    // The bootloader message as the fastboot page found it. fastbootd cannot
    // reboot the phone itself: for the computer's `reboot recovery`,
    // `bootloader` and `fastboot` it writes the request there and hands the
    // port back, and init stops it as the port goes through "none". What it
    // wrote while the page showed is therefore still in misc, and is carried
    // out as the page ends.
    let mut fastboot_found = match fastboot {
        true => Some(bcb::read(&misc)?),
        false => None,
    };
    let mut kept = request.work.lasts();
    // The wipe page offers the cache partition where the phone's table has
    // one; a table that does not read leaves it out, and the page still
    // works.
    app.state.cache_partition = match wipe::cache() {
        Ok(cache) => cache.is_some(),
        Err(e) => {
            skyward_sys::kmsg(&format!("whether the phone has a cache partition is not known: {e:#}"))?;
            false
        }
    };
    app.state.phone_logs = cfg.wipe.logs.len();
    // The controller the card tray hangs from, on a phone whose config names
    // one: the pages offer the card only there, and the watcher has a tray to
    // look in.
    let tray = cfg.sdcard.as_ref().map(|card| card.host.clone());
    app.state.sdcard_supported = tray.is_some();
    // Skyward lives in a recovery partition of each slot where the phone
    // has them, and the install page then offers to keep it there. Such a
    // phone holds a second system where it also has boot and init_boot
    // partitions in both slots, which a switch writes.
    let by_name = Path::new(skyward_block::BY_NAME);
    let both = |base: &str| -> Result<bool> { Ok(by_name.join(format!("{base}_a")).try_exists()? && by_name.join(format!("{base}_b")).try_exists()?) };
    app.state.recovery_partitions = both("recovery")?;
    app.state.dual_boot = app.state.recovery_partitions && both("boot")? && both("init_boot")?;
    // The second system's work; the systems are read at once, for the home
    // page to name the one that starts, once a switch cut off before is
    // finished.
    let mut systems_worker = match app.state.dual_boot {
        true => Some(systems::Worker::start(cfg.device.os_version_prop.clone())?),
        false => None,
    };
    // A switch the system asked for, under way; then the phone starts the
    // system switched to.
    let mut switch_asked = false;
    // A switch went through and the phone is restarting into the system.
    let mut switched = false;
    // fastbootd may write the boot partitions, which the installed system's
    // boot set goes back into first, and finds /metadata as it was; why the
    // boot set could not go back is shown, and fastboot goes on, as the way
    // left to repair the phone.
    if fastboot && app.state.dual_boot {
        let metadata = Path::new("/metadata");
        let mounted = mount::is_mount_point(metadata)?;
        let put_back = skyward_systems::install::ensure_installed();
        if !mounted && mount::is_mount_point(metadata)? {
            mount::umount(metadata)?;
        }
        if let Err(e) = put_back {
            skyward_sys::kmsg(&format!("the installed system's boot set did not go back before fastboot: {e:#}"))?;
            app.report(&format!("{e:#}"), monotonic_ms());
        }
    }
    let mut eraser: Option<Eraser> = None;
    // Erasing the system asked for leaves on its own once it is done, as
    // does erasing an update asked for, sideloaded with a restart after it;
    // what the user chose waits for them.
    let mut erase_leaves = false;
    // Erasing all user data rescue asked for, whose end goes back to rescue
    // rather than to a page of its own.
    let mut rescue_erasing = false;
    // Where rescue asked the phone to leave for, once it lets USB go.
    let mut rescue_leaves: Option<(Target, bool)> = None;
    // The install the system asked for, until the data partition it is in
    // opens, or the user whose own files hold it unlocks them.
    let mut pending: Option<Pending> = None;
    // Work on whole partitions under way, and whether the system asked for
    // it before the recovery started.
    let mut part_worker: Option<partwork::Worker> = None;
    match &request.work {
        Work::WipeData { reformat } => {
            eraser = Some(factory_reset(&mut app, request.keep_memtag_mode, true, reformat.clone())?);
            erase_leaves = true;
        }
        Work::WipeCache { reformat } => {
            eraser = Some(parts_wipe(&mut app, vec![Step::Cache], &cfg.wipe.logs, true, reformat.clone())?);
            erase_leaves = true;
        }
        Work::Sideload { .. } => {
            app.start_sideload();
            installer = Some(Installer::start(detach(cfg.services.clone()), install_options(&app))?);
            sideloading = true;
        }
        Work::Rescue => {
            app.start_rescue();
            installer = Some(Installer::rescue(detach(cfg.services.clone()), install_options(&app))?);
            sideloading = true;
        }
        Work::Update(update) => {
            app.start_asked(update.security, update.package.clone());
            let retry = update.retry > 0;
            if !retry && panicked() {
                skyward_sys::kmsg("the phone came here after its kernel stopped, so the update asked for is not installed")?;
                app.install(skyward_install::Event::Finished(skyward_install::Outcome::Refused(skyward_install::Refusal::Panicked)), monotonic_ms());
                bcb::clear(&misc)?;
                kept = false;
            } else {
                let (package, needs) = asked::locate(&update.package, app.state.cache_partition);
                skyward_sys::kmsg(&format!("the update asked for is {package:?}, which needs {needs:?}"))?;
                let wanted = Pending { package, needs, retry };
                match needs {
                    Needs::Data | Needs::User(_) => {
                        app.asked_opening();
                        decryptor = Some(Decryptor::start(decrypt::Job::Open)?);
                        pending = Some(wanted);
                    }
                    Needs::Nothing | Needs::Cache => installer = Some(install_asked(cfg, &app, wanted)?),
                }
            }
        }
        Work::WipeAb { package_size } => {
            app.part_work_started(PartKind::Disable, true);
            part_worker = Some(partwork::Worker::start(partwork::Task::Disable { package_size: *package_size })?);
        }
        Work::UnshareBlocks => {
            let slot = skyward_vab::current_slot()?.context("not an A/B phone")?;
            app.part_work_started(PartKind::Unshare, true);
            part_worker = Some(partwork::Worker::start(partwork::Task::Unshare { slot })?);
        }
        // The installed system is asked for by a word, and the second by
        // its id.
        Work::SwitchSystem { to } => match systems_worker.as_mut() {
            Some(worker) => {
                app.switch_asked(if to == "installed" { Who::Installed } else { Who::Other });
                worker.send(systems::Task::SwitchNamed(to.clone()));
                switch_asked = true;
            }
            // A phone that cannot hold a second system has none to switch
            // to, and the request is done with.
            None => {
                skyward_sys::kmsg(&format!("asked to switch to the system {to:?}, which this phone cannot hold"))?;
                bcb::clear(&misc)?;
                kept = false;
            }
        },
        // The second system's start stopped and brought the recovery up to
        // say why; it still starts next, unless the user switches.
        Work::SystemStopped => {
            let why = request.reason.clone().unwrap_or_default();
            skyward_sys::kmsg(&format!("the second system's start stopped: {why}"))?;
            if app.state.dual_boot {
                app.system_stopped(why);
            }
        }
        Work::Menu | Work::Exit | Work::Fastboot | Work::PromptWipeData | Work::Unsupported(_) => {}
    }
    // A switch asked for finishes one cut off before itself; fastboot leaves
    // /metadata to fastbootd until the recovery's pages come up.
    if let (Some(worker), false, false) = (systems_worker.as_mut(), switch_asked, fastboot) {
        worker.send(systems::Task::Resume);
    }
    // The port came up with adb alone; at rest it offers file transfer
    // beside it.
    if transfer.is_some() && !fastboot && !sideloading {
        if let Err(e) = skyward_sys::usb::switch(skyward_sys::usb::resting(), USB_TIMEOUT) {
            skyward_sys::kmsg(&format!("the port did not take file transfer: {e:#}"))?;
            app.report(&format!("{e:#}"), monotonic_ms());
        }
    }
    // Once the work the system asked for is done, the page shows it for a
    // moment; then the phone starts the system, or powers off where the
    // request says to.
    let leave = if request.shutdown_after { Target::PowerOff } else { Target::System };
    let mut leave_at: Option<u64> = None;
    // The terminal page's shell, from when the page first asks for one.
    let mut shell: Option<pty::Shell> = None;
    // A folder being read for the file manager or a place, on a thread of
    // its own.
    let mut listing: Option<Listing> = None;
    // Copying, moving or deleting under way.
    let mut worker: Option<filework::Worker> = None;
    // Giving the data partition's files their SELinux labels under way.
    let mut relabeler: Option<filework::Worker> = None;
    // Backing up or restoring under way.
    let mut backuper: Option<backup::Worker> = None;
    // Whether a restore of the system's partitions is under way: it takes
    // them off to write over them, and they are brought back once it ends.
    let mut reattach = false;
    // Taking an image out of a system package under way.
    let mut extractor: Option<extract::Worker> = None;
    // An update's merge followed: one the recovery found under way, which
    // the kernel took up again as the snapshots were mapped, or one started
    // on the install page.
    let mut merger: Option<merge::Merger> = None;
    if let Some(slot) = info.slot {
        let standing = skyward_vab::standing(slot)?;
        if matches!(standing, Standing::Merging(_)) && skyward_vab::snapshots_mapped(slot)? {
            skyward_sys::kmsg("the update's merge goes on")?;
            merger = Some(merge::Merger::start(slot, false)?);
        }
        app.set_update(standing);
    }
    // The DNS servers the network joined gave, for looking names up.
    let servers: Servers = Arc::new(Mutex::new(Vec::new()));
    // The WLAN, on a phone whose driver the recovery brings up; it starts
    // off. Without the bundle of the vendor interface's libraries the
    // supplicant does not start, and the WLAN page says why.
    let wlan = match &cfg.wlan {
        Some(w) => {
            app.state.wlan = Some(Wlan::default());
            let modules = cfg.modules.sources.iter().map(|s| skyward_modules::source_dir(s)).collect::<Result<Vec<_>>>()?;
            let libraries = secure::libraries().unwrap_or_else(|e| {
                let _ = skyward_sys::kmsg(&format!("the vendor interface's libraries are not open: {e:#}"));
                None
            });
            Some(wlan::Worker::start(w.clone(), modules, libraries, servers.clone())?)
        }
        None => None,
    };
    // A WLAN started before the data partition is open keeps its networks in
    // memory and saves the ones joined there in it: the supplicant that holds
    // them is started once the partition they are kept on is open, by the
    // opening below.
    if wlan_wanted {
        match &wlan {
            Some(w) => w.send(wlan::Command::On),
            None => {
                let _ = skyward_sys::kmsg("the WLAN was on, and this phone's recovery brings none up");
            }
        }
    }
    let mut scanned_at = Instant::now();
    // When adbd was asked for the network's port, until the kernel is read as showing it holds it.
    let mut adb_until: Option<Instant> = None;
    app.state.rom = cfg.rom.as_ref().map(|rom| RomState::new(rom.name.clone()));
    // Asking the update server, and downloading the full package.
    let mut checker: Option<updates::Worker> = None;
    let mut downloader: Option<updates::Worker> = None;
    let mut writer: Option<updates::Worker> = None;
    let mut rom_checker: Option<updates::Worker> = None;
    let mut rom_downloader: Option<updates::Worker> = None;
    // The phone sleeps while its screen is off and nothing needs it awake.
    let sleeper = Sleeper::start()?;
    // A uevent may be a charger or a computer attached or taken away: the
    // battery is read at once, and the phone stays awake until it is.
    let uevents = uevent::Listener::open()?;
    let mut asked: Option<Instant> = None;
    // The USB drive plugged in and the card in the tray, looked for each
    // second.
    let mut plugged: Option<Plugged> = None;
    let mut seen: Option<String> = None;
    let mut card_plugged: Option<Plugged> = None;
    let mut card_seen: Option<String> = None;
    let mut drive_at = Instant::now();
    loop {
        let now = monotonic_ms();
        if leave_at.is_some_and(|at| now >= at) {
            leave_at = None;
            power(leave)?;
        }
        if !touch_known && started.elapsed() >= TOUCH_WAIT {
            touch_known = true;
            if !inputs.has_panel() {
                skyward_sys::kmsg("no touch panel came up; the pages are worked with the keys")?;
                app.touchscreen_missing();
            }
        }
        for fx in app.tick(now) {
            match fx {
                Effect::Haptic => {
                    if let Some(h) = &mut haptics {
                        h.click(app.state.vibration)?;
                    }
                }
                // The power key held past its wait for volume down turns
                // the screen off.
                Effect::Ticks => {
                    if let Some(h) = &mut haptics {
                        h.ticks()?;
                    }
                }
                other => unreachable!("time passing asks for nothing but the motor, not {other:?}"),
            }
        }
        let time = clock(app.state.time_zone)?;
        if time != shown_clock {
            shown_clock = time;
            app.redraw();
        }
        for reading in readings.taken() {
            match reading {
                Reading::Status(battery, usb, at) => {
                    let battery = battery?;
                    if let (Some(switch), Some(battery)) = (&mut charging, &battery) {
                        let limit = if app.state.respect_charge_limit { app.state.charge_limit } else { None };
                        switch.keep(battery.level, limit)?;
                    }
                    if asked.is_some_and(|asked| at >= asked) {
                        asked = None;
                    }
                    if battery != info.battery || usb != info.usb {
                        plug(&mut app, &info.battery, &battery, now)?;
                        info.battery = battery;
                        info.usb = usb;
                        app.refresh();
                    }
                }
                Reading::All(all, at) => {
                    let all = (*all)?;
                    if let (Some(switch), Some(battery)) = (&mut charging, &all.battery) {
                        let limit = if app.state.respect_charge_limit { app.state.charge_limit } else { None };
                        switch.keep(battery.level, limit)?;
                    }
                    if asked.is_some_and(|asked| at >= asked) {
                        asked = None;
                    }
                    if at >= read_at && all != info {
                        plug(&mut app, &info.battery, &all.battery, now)?;
                        info = all;
                        app.refresh();
                    }
                }
            }
        }
        // In fastboot the page follows fastbootd and the cable each second.
        if fastboot && port_at.elapsed().as_secs() >= 1 {
            let (running, link) = fastboot_status()?;
            app.set_fastboot(running, link);
            port_at = Instant::now();
        }
        // The page comes up out of the dark over a moment, rather than the
        // bootloader's frame snapping into this one, and is drawn again as it
        // does.
        let appear = (started.elapsed().as_millis() as f32 / APPEAR.as_millis() as f32).min(1.0);
        if appear < 1.0 {
            app.redraw();
        }
        let panel = app.panel(now);
        let mut shown = false;
        if panel.is_some() {
            // A display turned off comes on again with a frame drawn for it.
            if app.take_dirty() || !surface.is_on() {
                let t0 = Instant::now();
                app.compose(&mut screen, &fonts, &info, &shown_clock, now);
                screen.fade(appear);
                surface.present(&screen)?;
                eprintln!("frame {:?} in {} ms", app.screen(), t0.elapsed().as_millis());
                if let Some(path) = dump {
                    fs::write(path, &screen.data).with_context(|| format!("write {}", path.display()))?;
                }
                shown = true;
            }
            blanked = false;
        } else if !blanked {
            // Black out the panel's frame memory before the backlight goes
            // to 0 and the display off, so an OLED that floors the backlight
            // above zero rather than cutting it shows nothing as it goes off.
            screen.data.fill(0);
            surface.present(&screen)?;
            surface.settle()?;
            blanked = true;
        }
        // After the frame is on the panel, so a panel coming back on lights
        // with what it should show now, never with what it showed when it
        // went off.
        if backlight.level == 0 && panel.is_some() {
            surface.settle()?;
        }
        backlight.show(panel)?;
        if panel.is_some() {
            // Lit, a panel that came on again switches to its fastest
            // refresh.
            surface.speed_up()?;
            rested_at = None;
        } else if surface.is_on() {
            // With the backlight off, the panel rests, and the touch
            // controller following it stops drawing power.
            surface.rest()?;
            rested_at = Some(Instant::now());
        } else if rested_at.is_some_and(|at| at.elapsed() >= REST) {
            // Rested long enough, the display goes off: the panel and the
            // display processor feeding it stop drawing power.
            surface.turn_off()?;
            rested_at = None;
        }
        // A page at rest draws the rows it has yet to while nothing else
        // is to be done, a band at a time, looking for input between bands.
        let ahead = panel.is_some() && !shown && app.draw_ahead(&fonts, &info);
        // A frame shown has waited for the one before it to reach the
        // panel, which keeps the loop to the panel's pace; otherwise the CPU
        // gets at least a millisecond.
        let floor = if shown { 0 } else { 1 };
        let until_due = app.deadline(now).map(|d| d.saturating_sub(now) as i32);
        // Coming up, the page is stepped rather than waited on.
        #[cfg(feature = "roots")]
        let rooting = app.rooting();
        #[cfg(not(feature = "roots"))]
        let rooting = false;
        // A run says how far it has come without a pipe of its own, so the
        // page is drawn again often enough for its card to move.
        let wait_cap = if appear < 1.0 {
            16
        } else if rooting {
            100
        } else if listing.is_some() {
            50
        } else {
            1000
        };
        // A resting panel turns off once its rest is over.
        let rest_left = rested_at.map(|at| REST.saturating_sub(at.elapsed()).as_millis() as i32);
        let timeout = if ahead { 0 } else { until_due.into_iter().chain(rest_left).min().map_or(wait_cap, |t| t.clamp(floor, wait_cap)) };
        // With the display off, the phone sleeps unless something needs it
        // awake: work under way, fastboot or sideload, a restart on its way,
        // a command running in the terminal, a network being joined, the
        // battery read after a uevent, or a computer on the USB port, whose
        // adb and file transfer stay up.
        let systems_busy = systems_worker.as_ref().is_some_and(systems::Worker::busy);
        let busy = installer.is_some()
            || eraser.is_some()
            || worker.is_some()
            || relabeler.is_some()
            || backuper.is_some()
            || merger.is_some()
            || decryptor.is_some()
            || part_worker.is_some()
            || checker.is_some()
            || downloader.is_some()
            || writer.is_some()
            || rom_checker.is_some()
            || rom_downloader.is_some()
            || systems_busy;
        let joining = app.state.wlan.as_ref().and_then(|w| w.joined.as_ref()).is_some_and(|j| matches!(j.stage, Joining::Joining | Joining::Addressing));
        let running = match &shell {
            Some(sh) => !sh.waiting()?,
            None => false,
        };
        let may_sleep = surface.is_off()
            && !busy
            && !fastboot
            && !sideloading
            && leave_at.is_none()
            && !joining
            && !running
            && asked.is_none()
            && usb_link()? != UsbLink::Connected;
        sleeper.allow(may_sleep);
        let mut fds = vec![inputs.fd(), uevents.fd(), sleeper.fd()];
        if let Some(i) = &installer {
            fds.push(i.worker.fd());
        }
        if let Some(c) = &control {
            fds.push(c.fd());
        }
        if let Some(e) = &eraser {
            fds.push(e.worker.fd());
        }
        if let Some(sh) = &shell {
            fds.extend(sh.fds());
        }
        if let Some(w) = &worker {
            fds.push(w.worker.fd());
        }
        if let Some(r) = &relabeler {
            fds.push(r.worker.fd());
        }
        if let Some(b) = &backuper {
            fds.push(b.worker.fd());
        }
        if let Some(e) = &extractor {
            fds.push(e.worker.fd());
        }
        if let Some(m) = &merger {
            fds.push(m.worker.fd());
        }
        if let Some(d) = &decryptor {
            fds.push(d.worker.fd());
        }
        if let Some(p) = &part_worker {
            fds.push(p.worker.fd());
        }
        if let Some(w) = &wlan {
            fds.push(w.fd());
        }
        if let Some(c) = &checker {
            fds.push(c.worker.fd());
        }
        #[cfg(feature = "roots")]
        if let Some(c) = &kernel_checker {
            fds.push(c.fd());
        }
        if let Some(d) = &downloader {
            fds.push(d.worker.fd());
        }
        if let Some(w) = &writer {
            fds.push(w.worker.fd());
        }
        if let Some(s) = &systems_worker {
            fds.push(s.fd());
        }
        fds.push(readings.fd());
        wait(&fds, timeout)?;
        let now = monotonic_ms();
        for slept in sleeper.slept() {
            match slept {
                Slept::Woke => skyward_sys::kmsg("the phone slept and woke")?,
                Slept::Refused(why) => skyward_sys::kmsg(&format!("the phone did not sleep: {why}"))?,
            }
        }
        if uevents.heard()? {
            readings.ask();
            asked = Some(Instant::now());
        }
        let mut drive_told = None;
        let mut card_told = None;
        if drive_at.elapsed().as_secs() >= 1 {
            drive_told = look_for_medium(Place::UsbDrive, None, &mut plugged, &mut seen)?;
            card_told = look_for_medium(Place::SdCard, tray.as_deref(), &mut card_plugged, &mut card_seen)?;
            drive_at = Instant::now();
        }
        // What the installer and the eraser told comes first, so a touch
        // acts on the page as it now stands.
        {
            let told = installer.as_ref().map(|i| i.worker.events()).unwrap_or_default();
            for ev in told {
                use skyward_install::{Event as Told, Outcome};
                let finished = matches!(ev, Told::Finished(_));
                let installed = matches!(ev, Told::Finished(Outcome::Installed { .. } | Outcome::Ran { .. }));
                let wipe_after = matches!(ev, Told::Finished(Outcome::Installed { wipe: true, .. }));
                let script_cache = matches!(ev, Told::Finished(Outcome::Ran { wipe_cache: true, .. }));
                // A second system installed is read for the pages to show.
                if matches!(ev, Told::Done(Outcome::SystemInstalled { .. }) | Told::Finished(Outcome::SystemInstalled { .. })) {
                    systems_worker.as_mut().context("a second system went in on a phone that cannot hold one")?.send(systems::Task::Read);
                }
                eprintln!("install {ev:?}");
                match &ev {
                    // Rescue's own erasing tells rescue how it went once it
                    // ends; file transfer that will not let internal
                    // storage go keeps it from starting.
                    Told::Wipe => {
                        ensure!(eraser.is_none(), "erasing is under way already");
                        if withhold(&mut transfer, &mut app, now)? {
                            eraser = Some(rescue_wipe(&mut app)?);
                            rescue_erasing = true;
                        } else {
                            installer.as_ref().context("rescue asked for erasing")?.wiped(false);
                        }
                    }
                    Told::Reboot(target) => rescue_leaves = Some(rescue_target(target)),
                    _ => {}
                }
                app.install(ev, now);
                if finished {
                    installer = None;
                    sideloading = false;
                    // The port is the pages' again: file transfer comes back
                    // where its switch is on.
                    if transfer.is_none() {
                        let zone = app.state.time_zone;
                        transfer = start_transfer(cfg, &info, &mut app, zone);
                    }
                    // A sideload or an update the system asked for is done
                    // with however it went, and so is rescue, once its page
                    // is left or the computer asks the phone to leave. A
                    // sideload asked to leaves on its own after it, and an
                    // update that went in does, once what the package or the
                    // request asked to be erased is.
                    if matches!(request.work, Work::Sideload { .. } | Work::Update(_) | Work::Rescue) && kept {
                        bcb::clear(&misc)?;
                        kept = false;
                    }
                    let (leaves, cache_after) = match &request.work {
                        Work::Sideload { auto_reboot } => (*auto_reboot, script_cache),
                        Work::Update(update) => (installed, script_cache || (installed && update.wipe_cache)),
                        _ => (false, script_cache),
                    };
                    if cache_after && !app.state.cache_partition {
                        skyward_sys::kmsg("the install asked for the cache partition to be erased, and the phone has none")?;
                    }
                    if wipe_after {
                        ensure!(eraser.is_none(), "erasing is under way already");
                        eraser = Some(factory_reset(&mut app, false, false, None)?);
                        erase_leaves = leaves;
                    } else if cache_after && app.state.cache_partition {
                        ensure!(eraser.is_none(), "erasing is under way already");
                        eraser = Some(parts_wipe(&mut app, vec![Step::Cache], &cfg.wipe.logs, false, None)?);
                        erase_leaves = leaves;
                    } else if leaves {
                        leave_at = Some(now + LEAVE_AFTER_MS);
                    }
                    // Rescue asked to come back is told so in misc, as
                    // Android's own recovery tells it.
                    if let Some((target, again)) = rescue_leaves.take() {
                        if again {
                            bcb::write(&misc, &bcb::Bcb { command: "boot-rescue".into(), args: Vec::new() })?;
                        }
                        power(target)?;
                    }
                }
            }
        }
        let mut erased = false;
        if let Some(e) = &eraser {
            for ev in e.worker.events() {
                match ev {
                    // Rescue's page says only that erasing is under way.
                    wipe::Event::At(_) | wipe::Event::Progress(..) if rescue_erasing => {}
                    wipe::Event::At(step) => app.erase_at(part(step)),
                    wipe::Event::Progress(done, total) => app.erase_progress(done, total),
                    // Rescue stays on: its request stays in misc, and the
                    // computer hears how erasing went.
                    wipe::Event::Ended(outcome) if rescue_erasing => {
                        let said = outcome.as_ref().map_or_else(wipe::said, |_| "done".to_string());
                        skyward_sys::kmsg(&format!("erasing for rescue ended: {said}"))?;
                        installer.as_ref().context("rescue waits for erasing to end")?.wiped(outcome.is_ok());
                        app.rescue_wiped(outcome.map_err(erase_stop), now);
                        if app.state.data == Data::Erased {
                            closed.clear();
                        }
                        rescue_erasing = false;
                        erased = true;
                    }
                    wipe::Event::Opened(opening) => {
                        if let Some(mounted) = &opening.mounted {
                            log_mounted(mounted)?;
                        }
                        closed = opening.closed;
                        show_closed(&mut app, &closed)?;
                    }
                    wipe::Event::Ended(outcome) => {
                        let said = outcome.as_ref().map_or_else(wipe::said, |_| "done".to_string());
                        skyward_sys::kmsg(&format!("erasing ended: {said}"))?;
                        // As with the phone's own recovery, the request is
                        // done with either way, so a restart starts the
                        // system rather than erasing again. A factory
                        // reset done is what an update that asked for one
                        // asked, too.
                        if kept {
                            bcb::clear(&misc)?;
                            kept = false;
                        }
                        if outcome.is_ok() && app.state.erase.reset {
                            bcb::clear(&misc)?;
                        }
                        if outcome.is_ok() && erase_leaves {
                            leave_at = Some(now + LEAVE_AFTER_MS);
                        }
                        app.erase_ended(outcome.map_err(erase_stop), now);
                        // Users of a data partition erased whole have no
                        // files left to open.
                        if app.state.data == Data::Erased {
                            closed.clear();
                        }
                        erased = true;
                    }
                }
            }
        }
        // Erasing all data makes /metadata afresh, and the settings with it
        // are written there again.
        if erased {
            eraser = None;
            keeper.remade();
        }
        let job_end = |end: filework::End| match end {
            filework::End::Done => JobEnd::Done,
            filework::End::Stopped => JobEnd::Stopped,
            filework::End::Failed(why) => JobEnd::Failed(why),
        };
        let mut worked = false;
        let listed = listing.as_ref().and_then(|l| l.rx.try_recv().ok());
        if let Some((task, files, listed)) = listed {
            match task {
                ListingTask::Files(folder) => app.set_files(folder, files, listed),
                ListingTask::Place(place, folder) => app.set_place(place, folder, files, listed),
            }
            listing = None;
        }
        if let Some(w) = &worker {
            for ev in w.worker.events() {
                match ev {
                    filework::Event::Progress(p) => app.file_progress((p.bytes, p.total_bytes), (p.entries, p.total_entries)),
                    filework::Event::Relabeling(_) => unreachable!("work on files gives no labels"),
                    filework::Event::Ended(end) => {
                        app.file_job_ended(job_end(end), now);
                        worked = true;
                    }
                }
            }
        }
        let mut relabeled = false;
        if let Some(r) = &relabeler {
            for ev in r.worker.events() {
                match ev {
                    filework::Event::Relabeling(p) => app.relabel_progress(p.checked, p.total, p.changed),
                    filework::Event::Progress(_) => unreachable!("relabeling moves no bytes"),
                    filework::Event::Ended(end) => {
                        app.relabel_ended(job_end(end), now);
                        relabeled = true;
                    }
                }
            }
        }
        if relabeled {
            relabeler = None;
        }
        #[cfg(feature = "roots")]
        {
            let mut ended = false;
            if let Some(worker) = &apatch_worker {
                for told in worker.events() {
                    match told {
                        crate::apatch::Told::Stage(stage) => app.apatch_stage(stage),
                        crate::apatch::Told::Ended(outcome) => {
                            let _ = skyward_sys::kmsg(match &outcome {
                                skyward_root::folkpatch::Outcome::Installed(_) => "apatch: the boot partitions are patched",
                                skyward_root::folkpatch::Outcome::Refused(_) => "apatch: nothing was written",
                                skyward_root::folkpatch::Outcome::Failed(_) => "apatch: the run did not finish",
                            });
                            app.apatch_finished(outcome, now);
                            ended = true;
                        }
                    }
                }
            }
            if ended {
                apatch_worker = None;
            }
        }
        #[cfg(feature = "roots")]
        {
            let mut ended = false;
            if let Some(worker) = &ksu_worker {
                for told in worker.events() {
                    match told {
                        crate::ksu::Told::Stage(stage) => app.ksu_stage(stage),
                        crate::ksu::Told::Ended(outcome) => {
                            app.ksu_finished(outcome, now);
                            ended = true;
                        }
                    }
                }
            }
            if ended {
                ksu_worker = None;
            }
        }
        #[cfg(feature = "roots")]
        {
            let mut ended = false;
            if let Some(worker) = &kernel_worker {
                for told in worker.events() {
                    match told {
                        crate::kernel::Told::Stage(stage) => app.kernel_stage(stage),
                        crate::kernel::Told::Ended(outcome) => {
                            app.kernel_finished(outcome, now);
                            ended = true;
                        }
                    }
                }
            }
            if ended {
                kernel_worker = None;
            }
        }
        #[cfg(feature = "roots")]
        {
            let mut ended = false;
            if let Some(checker) = &kernel_checker {
                for checked in checker.events() {
                    match checked {
                        crate::kernel::Checked::Done(details) => {
                            app.set_kernel_details(details);
                            ended = true;
                        }
                    }
                }
            }
            if ended {
                kernel_checker = None;
                kernel_checker_expired = false;
            } else if kernel_checker.is_some() && !kernel_checker_expired && now.saturating_sub(kernel_checker_at) >= 10_000 {
                app.kernel_details_timed_out();
                kernel_checker_expired = true;
            }
        }
        let mut parted = false;
        if let Some(p) = &part_worker {
            for ev in p.worker.events() {
                match ev {
                    partwork::Event::Listed(names) => app.part_listed(names),
                    partwork::Event::At(index) => app.part_at(index),
                    partwork::Event::Ended(index, item, said) => {
                        skyward_sys::kmsg(&format!("partition work: item {index} ended as {item:?}"))?;
                        app.part_ended(index, item_end(item), &said);
                    }
                    partwork::Event::Done(done) => {
                        skyward_sys::kmsg(&format!("partition work ended: {done:?}"))?;
                        let work = app.state.part_work.as_ref().context("partition work shows on its page")?;
                        let whole = done.is_ok() && !work.items.iter().any(|(_, s)| matches!(s, ItemState::Ended(ItemEnd::Failed(_))));
                        let data = work.items.iter().any(|(name, _)| name == DATA_PARTITION);
                        // What the system asked for is done with either way,
                        // and the phone leaves on its own once it went
                        // through every partition.
                        if work.asked {
                            if kept {
                                bcb::clear(&misc)?;
                                kept = false;
                            }
                            if whole {
                                leave_at = Some(now + LEAVE_AFTER_MS);
                            }
                        }
                        // Work on the data partition leaves it closed.
                        if data {
                            closed.clear();
                        }
                        app.part_work_ended(done.map_err(part_stop), now);
                        parted = true;
                    }
                }
            }
        }
        // Work on whole partitions may have erased or made afresh the one
        // the settings are kept on.
        if parted {
            part_worker = None;
            keeper.remade();
        }
        // The folder shown is read again for what the work changed.
        if worked {
            worker = None;
            let folder = app.state.files.path.clone();
            start_listing(&mut listing, ListingTask::Files(folder))?;
        }
        let mut backed = false;
        if let Some(b) = &backuper {
            for ev in b.worker.events() {
                match ev {
                    backup::Event::Plan(plan) => app.backup_plan(plan),
                    backup::Event::At { item, done, of, checking } => app.backup_at(BackupAt { item, done, of, checking }),
                    backup::Event::Ended(end) => {
                        app.backup_ended(backup_end(end), now);
                        backed = true;
                        if reattach {
                            reattach = false;
                            crate::prepare(cfg, skyward_vab::current_slot()?)?;
                            read_at = Instant::now();
                            info = skyward_info::collect(cfg)?;
                            app.set_mounts(mounts());
                            app.refresh();
                        }
                    }
                }
            }
        }
        if backed {
            backuper = None;
        }
        let mut extracted = false;
        if let Some(e) = &extractor {
            for ev in e.worker.events() {
                match ev {
                    extract::Event::Progress { done, total } => app.package_progress(done, total),
                    extract::Event::Ended(end) => {
                        match end {
                            extract::End::Done { path, size } => {
                                let path = path.to_string_lossy().into_owned();
                                if let Some(path) = app.package_extracted(path, size, now) {
                                    if withhold(&mut transfer, &mut app, now)? {
                                        installer = Some(Installer::open(PathBuf::from(path), detach(cfg.services.clone()), install_options(&app))?);
                                    }
                                }
                            }
                            extract::End::Stopped => app.package_stopped(now),
                            extract::End::Failed(why) => app.package_failed(why, now),
                        }
                        extracted = true;
                    }
                }
            }
        }
        if extracted {
            extractor = None;
        }
        if let Some(m) = &merger {
            let slot = info.slot.context("only an A/B phone merges an update")?;
            for ev in m.worker.events() {
                match ev {
                    merge::Event::Looked(look) => {
                        eprintln!("merge {look:?}");
                        app.set_update(match look {
                            Merge::Going(done) => Standing::Merging(done),
                            Merge::Done | Merge::NeedsReboot => Standing::Merged,
                            Merge::Failed(code) => Standing::Failed(code),
                            Merge::Cancelled | Merge::Idle(_) => skyward_vab::standing(slot)?,
                        });
                    }
                    merge::Event::Failed(why) => {
                        skyward_sys::kmsg(&format!("the update did not merge: {why}"))?;
                        app.report(&skyward_ui::fmt::merge_failed(app.state.lang, &why), now);
                        app.set_update(skyward_vab::standing(slot)?);
                    }
                }
            }
            // Merged or not, the phone's readings say where the update
            // stands now.
            if m.worker.finished() {
                merger = None;
                read_at = Instant::now();
                info = skyward_info::collect(cfg)?;
                app.refresh();
            }
        }
        if let Some(w) = &mut systems_worker {
            for ev in w.events() {
                let lang = app.state.lang;
                match ev {
                    systems::Event::Resumed(to) => {
                        skyward_sys::kmsg(&format!("a switch cut off before is finished, and the {to:?} system starts next"))?;
                        app.report(skyward_ui::fmt::switch_resumed(lang), now);
                    }
                    systems::Event::NotResumed(why) => {
                        skyward_sys::kmsg(&format!("a switch cut off before is not finished: {why}"))?;
                        app.report(&skyward_ui::fmt::switch_unfinished(lang, &why), now);
                    }
                    systems::Event::Switched(to) => {
                        skyward_sys::kmsg(&format!("the {to:?} system starts next"))?;
                        // A switch the system asked for is done with.
                        if switch_asked && kept {
                            bcb::clear(&misc)?;
                            kept = false;
                        }
                        switch_asked = false;
                        switched = true;
                        power(Target::System)?;
                    }
                    systems::Event::Failed(why) => {
                        skyward_sys::kmsg(&format!("work on the systems failed: {why}"))?;
                        if switch_asked && kept {
                            bcb::clear(&misc)?;
                            kept = false;
                        }
                        switch_asked = false;
                        app.systems_work_failed(&why, now);
                    }
                    // The page stays as it is while the phone restarts.
                    systems::Event::Read(read) => {
                        if !switched {
                            app.set_systems(read);
                        }
                    }
                }
            }
        }
        if let Some((told, traffic)) = wlan.as_ref().map(wlan::Worker::told) {
            if let Some(w) = told {
                app.set_wlan(w);
            }
            if let Some(t) = traffic {
                app.set_traffic(t);
            }
        }
        if let Some(until) = adb_until {
            if adbwifi::listening() {
                adb_until = None;
                app.set_adb_wifi(AdbWifi::On);
            } else if Instant::now() >= until {
                adb_until = None;
                let why = format!("adbd did not take port {}", skyward_ui::app::ADB_WIFI_PORT);
                skyward_sys::kmsg(&why)?;
                app.set_adb_wifi(AdbWifi::Failed(why));
            }
        }
        // The WLAN page scans again now and then while it shows, as the
        // stock settings do.
        let wlan_on = app.state.wlan.as_ref().is_some_and(|w| w.power == WlanPower::On);
        if app.screen() == Screen::Wlan && wlan_on && scanned_at.elapsed() >= SCAN_EVERY {
            scanned_at = Instant::now();
            wlan_worker(&wlan)?.send(wlan::Command::Scan);
        }
        // Settings changed go into their file a moment after the last
        // change, while nothing else has /metadata: not in fastboot, whose
        // fastbootd may, nor while an install, a merge, erasing, a backup or
        // work on whole partitions is under way.
        let metadata_busy = fastboot
            || installer.is_some()
            || merger.is_some()
            || eraser.is_some()
            || backuper.is_some()
            || part_worker.is_some()
            || systems_worker.as_ref().is_some_and(systems::Worker::busy);
        if !metadata_busy {
            if let Some(due) = keeper.due(app.settings(), now) {
                match settings::write(&due) {
                    Ok(()) => {
                        skyward_sys::kmsg("the settings are kept")?;
                        keeper.written(due);
                    }
                    Err(e) => {
                        skyward_sys::kmsg(&format!("the settings were not kept: {e:#}"))?;
                        keeper.failed();
                    }
                }
            }
        }
        let mut checked = false;
        if let Some(c) = &checker {
            for ev in c.worker.events() {
                let updates::Event::Checked(answer) = ev else { unreachable!("asking the server downloads nothing") };
                let answer = answer.map(|found| match found {
                    Some(release) => Newest {
                        version: release.tag.clone(),
                        newer: true,
                        build: Some(Build {
                            name: release.image.name.clone(),
                            url: release.image.url.clone(),
                            link: release.image.url.clone(),
                            size: release.image.size,
                            sha256: release.sha256(None).unwrap_or_default(),
                            sums: release.sums.as_ref().map(|s| s.url.clone()),
                        }),
                    },
                    None => Newest { version: skyward_info::BUILD.to_string(), newer: false, build: None },
                });
                app.skyward_checked(answer);
                checked = true;
            }
        }
        if checked {
            checker = None;
        }
        let mut fetched = false;
        if let Some(d) = &downloader {
            for ev in d.worker.events() {
                match ev {
                    updates::Event::Progress(progress, rate) => app.skyward_progress(SkywardStage::Download(progress), rate),
                    updates::Event::Ended(end) => {
                        match end {
                            // The build is written over the recovery as soon
                            // as it is whole.
                            updates::End::Downloaded => {
                                let build = app.state.skyward_newest().and_then(|n| n.build.clone());
                                match build {
                                    Some(build) => {
                                        app.skyward_progress(SkywardStage::Write { done: 0, total: build.size }, 0);
                                        let task = updates::Task::Install { path: downloaded_build(&build), active: info.slot };
                                        writer = Some(updates::Worker::start(task, servers.clone())?);
                                    }
                                    None => app.skyward_ended(SkywardEnd::NotWritten("the build could not be written".into()), now),
                                }
                            }
                            updates::End::Stopped => app.skyward_ended(SkywardEnd::Stopped, now),
                            updates::End::Failed(why) => app.skyward_ended(SkywardEnd::Download(why), now),
                        }
                        fetched = true;
                    }
                    updates::Event::Checked(_)
                    | updates::Event::Writing(..)
                    | updates::Event::Installed { .. }
                    | updates::Event::RomChecked(_)
                    | updates::Event::RomEnded(_) => {
                        unreachable!("a download asks nothing and writes nothing")
                    }
                }
            }
        }
        if fetched {
            downloader = None;
        }
        let mut rom_checked = false;
        if let Some(c) = &rom_checker {
            for ev in c.worker.events() {
                let updates::Event::RomChecked(answer) = ev else { unreachable!("asking the ROM downloads nothing") };
                let have = match &answer {
                    Ok(rom::Answer::Newer(release)) => rom_have(&app, release),
                    _ => Have::Nothing,
                };
                app.rom_checked(answer, have);
                rom_checked = true;
            }
        }
        if rom_checked {
            rom_checker = None;
        }
        let mut rom_fetched = false;
        if let Some(d) = &rom_downloader {
            for ev in d.worker.events() {
                match ev {
                    updates::Event::Progress(progress, rate) => app.rom_download_progress(progress, rate),
                    updates::Event::RomEnded(end) => {
                        let release = app.state.rom_newest().context("a download is of the newest system")?.clone();
                        let end = match end {
                            updates::End::Downloaded => DownloadEnd::Downloaded,
                            updates::End::Stopped => DownloadEnd::Stopped,
                            updates::End::Failed(why) => DownloadEnd::Failed(why),
                        };
                        skyward_sys::kmsg(&format!("downloading {} ended: {end:?}", release.image.name))?;
                        app.rom_download_ended(end, rom_have(&app, &release), now);
                        rom_fetched = true;
                    }
                    updates::Event::Checked(_)
                    | updates::Event::Ended(_)
                    | updates::Event::Writing(..)
                    | updates::Event::Installed { .. }
                    | updates::Event::RomChecked(_) => unreachable!("a ROM download asks nothing and writes nothing"),
                }
            }
        }
        if rom_fetched {
            rom_downloader = None;
        }
        let mut wrote = false;
        if let Some(w) = &writer {
            for ev in w.worker.events() {
                match ev {
                    updates::Event::Writing(done, total) => app.skyward_progress(SkywardStage::Write { done, total }, 0),
                    updates::Event::Installed { slots, result } => {
                        let end = match result {
                            Ok(()) => SkywardEnd::Updated(slots),
                            Err(why) => SkywardEnd::WriteFailed(slots, why),
                        };
                        app.skyward_ended(end, now);
                        wrote = true;
                    }
                    updates::Event::Checked(_)
                    | updates::Event::Progress(..)
                    | updates::Event::Ended(_)
                    | updates::Event::RomChecked(_)
                    | updates::Event::RomEnded(_) => {
                        unreachable!("writing asks nothing and downloads nothing")
                    }
                }
            }
        }
        if wrote {
            writer = None;
        }
        if let Some(ended) = decryptor.as_ref().and_then(|d| d.worker.recv()) {
            decryptor = None;
            match ended {
                decrypt::Ended::Opened(Ok(opening)) => {
                    if let Some(mounted) = &opening.mounted {
                        log_mounted(mounted)?;
                    }
                    closed = opening.closed;
                    show_closed(&mut app, &closed)?;
                    // The system keeps its charge limit on the data partition;
                    // keep what it says for a start that does not open it.
                    if let Some(db) = &charge_db {
                        let limit = charging::system_limit(Path::new(db));
                        if limit != app.state.charge_limit {
                            app.set_charge_limit(limit);
                        }
                        if let Some(cache) = &charge_cache {
                            let kept = settings::writable(|| charging::cache(Path::new(cache), limit));
                            if let Err(e) = kept {
                                let _ = skyward_sys::kmsg(&format!("the charge limit is not kept: {e:#}"));
                            }
                        }
                    }
                    // The supplicant keeps its networks in memory until the
                    // data partition is open: it reads the ones saved there
                    // now, and joins one.
                    if wlan_wanted && !app.state.online() {
                        if let Some(w) = &wlan {
                            w.send(wlan::Command::On);
                        }
                    }
                    if let Some(w) = &wlan {
                        w.send(wlan::Command::Adopt);
                    }
                }
                decrypt::Ended::Opened(Err(why)) => {
                    skyward_sys::kmsg(&format!("the data partition stays closed: {why}"))?;
                    // The update the system asked for cannot be read, and
                    // is done with.
                    if pending.take().is_some() {
                        let failed = skyward_install::Outcome::Failed(skyward_install::Failure::Unreadable(why.clone()));
                        app.install(skyward_install::Event::Finished(failed), now);
                        if kept {
                            bcb::clear(&misc)?;
                            kept = false;
                        }
                    }
                    app.data_failed(why);
                }
                decrypt::Ended::Unlocked(Ok(still)) => {
                    closed = still;
                    show_closed(&mut app, &closed)?;
                }
                decrypt::Ended::Unlocked(Err(why)) => {
                    skyward_sys::kmsg(&format!("what was entered did not open the user's files: {why}"))?;
                    app.unlock_failed(why, now);
                }
            }
            // Screenshots taken while internal storage was closed go into
            // the gallery's once it opens.
            if app.state.place_open(Place::Storage) {
                if let Err(e) = keep_held_screenshots() {
                    let why = format!("{e:#}");
                    skyward_sys::kmsg(&format!("the screenshots held did not go into the gallery's: {why}"))?;
                    app.screenshots_not_kept(&why, now);
                }
            }
            // The update the system asked for starts once what holds it is
            // open; until the user whose own files hold it unlocks them,
            // its page says to.
            if let Some(wanted) = pending.take() {
                match wanted.needs {
                    Needs::User(user) if closed.iter().any(|c| c.user == user) => {
                        app.asked_locked(user, now);
                        pending = Some(wanted);
                    }
                    _ => installer = Some(install_asked(cfg, &app, wanted)?),
                }
            }
        }
        if let Some(sh) = &mut shell {
            let wrote = sh.read()?;
            if !wrote.is_empty() {
                app.term_output(&wrote);
            }
            if let Some(status) = sh.ended()? {
                // What it wrote last shows before its end does.
                let last = sh.read()?;
                if !last.is_empty() {
                    app.term_output(&last);
                }
                app.term_ended(pty::how(status));
                shell = None;
            }
        }
        // Then what programs asked on the recovery socket, each answered as
        // the recovery stands when its turn comes.
        let requests = match &control {
            Some(c) => c.requests()?,
            None => Vec::new(),
        };
        let incoming = arriving
            .take()
            .into_iter()
            .chain(drive_told.map(|found| Incoming::Medium(Place::UsbDrive, found)))
            .chain(card_told.map(|found| Incoming::Medium(Place::SdCard, found)))
            .chain(requests.into_iter().map(Incoming::Asked))
            .chain(inputs.read()?.into_iter().map(Incoming::Input));
        for came in incoming {
            let effects = match came {
                Incoming::Arrived => app.arrive(),
                Incoming::Medium(place, found) => app.set_medium(place, found),
                Incoming::Asked(asked) => {
                    skyward_sys::kmsg(&format!("asked on the recovery socket: {asked:?}"))?;
                    answer(asked, fastboot)
                }
                Incoming::Input(Event::Touch(t, at)) => app.touch(t, &fonts, at),
                Incoming::Input(Event::Key(k, true)) => app.key(k, now),
                Incoming::Input(Event::Key(k, false)) => app.key_up(k, now),
            };
            // What the pages ask for in answer to the work done goes on
            // after it.
            let mut queue = VecDeque::from(effects);
            while let Some(fx) = queue.pop_front() {
                eprintln!("effect {fx:?}");
                match fx {
                    // Leaving is the user's answer to whatever the request
                    // still kept in misc asked.
                    Effect::Reboot(target) => {
                        if kept {
                            bcb::clear(&misc)?;
                            kept = false;
                        }
                        power(target)?
                    }
                    Effect::Dismiss => {
                        if kept {
                            bcb::clear(&misc)?;
                            kept = false;
                        }
                    }
                    Effect::FactoryReset => {
                        ensure!(eraser.is_none(), "erasing is under way already");
                        if !withhold(&mut transfer, &mut app, now)? {
                            continue;
                        }
                        eraser = Some(factory_reset(&mut app, request.keep_memtag_mode, true, None)?);
                        erase_leaves = true;
                    }
                    // Opening the data partition and erasing it are not to
                    // cross; the page says to wait for the one under way.
                    Effect::Wipe(chosen) => {
                        ensure!(eraser.is_none(), "erasing is under way already");
                        if decryptor.is_some() {
                            app.report(DATA_OPENING, now);
                            continue;
                        }
                        if !withhold(&mut transfer, &mut app, now)? {
                            continue;
                        }
                        erase_leaves = false;
                        if chosen.contains(&Wipe::All) {
                            eraser = Some(factory_reset(&mut app, false, false, None)?);
                        } else if chosen.iter().any(|&w| w != Wipe::Cache) && app.state.data == Data::Erased {
                            app.erase_started(false, false, Vec::new());
                            app.erase_ended(Err(EraseStop::Erased), now);
                        } else {
                            let steps = chosen.into_iter().map(step).collect();
                            eraser = Some(parts_wipe(&mut app, steps, &cfg.wipe.logs, false, None)?);
                        }
                    }
                    Effect::Install => installer.as_ref().context("no installer holds an update")?.install(),
                    // A shell that will not start, or will not take keys,
                    // is the page's to report; the recovery carries on.
                    Effect::StartShell(folder) => {
                        ensure!(shell.is_none(), "a shell runs already");
                        let (rows, cols) = app.term_size(&fonts);
                        match pty::Shell::start(rows, cols, &cfg.device.codename, Path::new(&folder)) {
                            Ok(sh) => {
                                shell = Some(sh);
                                app.term_started(rows, cols);
                            }
                            Err(e) => {
                                skyward_sys::kmsg(&format!("no shell for the terminal: {e:#}"))?;
                                app.report(&format!("{e:#}"), now);
                            }
                        }
                    }
                    Effect::TermInput(bytes) => {
                        let sh = shell.as_mut().context("keys went to a shell that has ended")?;
                        if let Err(e) = sh.write(&bytes) {
                            app.report(&format!("{e:#}"), now);
                        }
                    }
                    Effect::ShellTo(folder) => {
                        let sh = shell.as_mut().context("a folder went to a shell that has ended")?;
                        let went = match sh.waiting() {
                            Ok(true) => sh.change_to(&folder),
                            Ok(false) => {
                                app.report(skyward_ui::fmt::TERMINAL_BUSY, now);
                                Ok(())
                            }
                            Err(e) => Err(e),
                        };
                        if let Err(e) = went {
                            app.report(&format!("{e:#}"), now);
                        }
                    }
                    Effect::Haptic => {
                        if let Some(h) = &mut haptics {
                            h.click(app.state.vibration)?;
                        }
                    }
                    Effect::Ticks => {
                        if let Some(h) = &mut haptics {
                            h.ticks()?;
                        }
                    }
                    // The panel shows the last frame drawn, which is what is
                    // saved; one that cannot be is the page's to report.
                    Effect::Screenshot { storage } => {
                        let t0 = Instant::now();
                        let saved = save_screenshot(&screen, storage, app.state.time_zone).map_err(|e| format!("{e:#}"));
                        eprintln!("screenshot {saved:?} in {} ms", t0.elapsed().as_millis());
                        app.screenshot_saved(saved, now);
                    }
                    Effect::Sideload => {
                        ensure!(installer.is_none(), "an installer is running already");
                        if !withhold(&mut transfer, &mut app, now)? {
                            continue;
                        }
                        stop_transfer(&mut transfer)?;
                        installer = Some(Installer::start(detach(cfg.services.clone()), install_options(&app))?);
                        sideloading = true;
                    }
                    Effect::InstallFile(place, path) => {
                        ensure!(installer.is_none(), "an installer is running already");
                        if !withhold(&mut transfer, &mut app, now)? {
                            continue;
                        }
                        installer = Some(Installer::open(in_place(place, &path), detach(cfg.services.clone()), install_options(&app))?);
                    }
                    Effect::InstallPath(path) => {
                        ensure!(installer.is_none(), "an installer is running already");
                        if !withhold(&mut transfer, &mut app, now)? {
                            continue;
                        }
                        installer = Some(Installer::open(PathBuf::from(path), detach(cfg.services.clone()), install_options(&app))?);
                    }
                    Effect::Extract { package, partition, out } => {
                        ensure!(extractor.is_none(), "an extraction is running already");
                        extractor = Some(extract::Worker::start(extract::Task {
                            package: PathBuf::from(package),
                            partition,
                            out: PathBuf::from(out),
                        })?);
                    }
                    Effect::StopExtract => {
                        // It may have ended between the page's tap and this:
                        // there is then nothing left to stop.
                        if let Some(e) = &extractor {
                            e.stop();
                        }
                    }
                    Effect::Flash { partition, pick } => {
                        let slot = skyward_vab::current_slot()?;
                        installer.as_ref().context("no installer holds an image")?.flash(partition, pick.slots(slot));
                    }
                    // The run carries its own key, and writes nothing but
                    // boot; the phone's partitions it reads and writes are
                    // not the ones this recovery runs from. Whatever stops
                    // it from starting is said on the page, which would
                    // otherwise wait on a run that never began.
                    #[cfg(feature = "roots")]
                    Effect::Apatch { pick, key, repatch } => {
                        ensure!(installer.is_none() && apatch_worker.is_none(), "a run is under way already");
                        let refused = |app: &mut App, why: String| {
                            app.apatch_finished(
                                skyward_root::folkpatch::Outcome::Failed(skyward_root::folkpatch::Failure {
                                    stage: skyward_root::folkpatch::Stage::Checking,
                                    slot: None,
                                    why,
                                    backups: Vec::new(),
                                    written: Vec::new(),
                                    key_path: None,
                                }),
                                now,
                            );
                        };
                        if !withhold(&mut transfer, &mut app, now)? {
                            refused(&mut app, "file transfer kept internal storage".into());
                            continue;
                        }
                        let started = (|| -> Result<crate::apatch::Worker> {
                            let current = skyward_vab::current_slot()?.context("not an A/B phone")?;
                            let battery = info.battery.clone().context("the battery was not read")?;
                            let options = skyward_root::folkpatch::Options {
                                device: cfg.device.codename.clone(),
                                current,
                                slots: pick.slots(current),
                                battery,
                                key,
                                repatch,
                                partition_dir: PathBuf::from("/dev/block/by-name"),
                                storage: PathBuf::from(skyward_crypto::data::SDCARD),
                                private: PathBuf::from("/data/local/tmp/skyward-root"),
                                stamp: skyward_ui::fmt::stamp(unix_now()?, app.state.time_zone),
                            };
                            skyward_sys::kmsg(&format!("apatch: patching {} slot(s)", options.slots.len()))?;
                            crate::apatch::Worker::start(options)
                        })();
                        match started {
                            Ok(worker) => apatch_worker = Some(worker),
                            Err(e) => refused(&mut app, format!("{e:#}")),
                        }
                    }
                    // The key of the newest run still on this phone, so that
                    // patching after an update can keep it.
                    #[cfg(feature = "roots")]
                    Effect::ReadApatchKey => {
                        let key = skyward_root::folkpatch::last_key(
                            Path::new("/data/local/tmp/skyward-root"),
                            Path::new(skyward_crypto::data::SDCARD),
                        );
                        app.set_apatch_last_key(key);
                    }
                    // Whether the slots the page would write carry the patch
                    // already, said on the page before anything is written.
                    #[cfg(feature = "roots")]
                    Effect::ReadApatchPatched { pick } => {
                        let mut slots = [None, None];
                        let mut any = false;
                        if let Some(current) = info.slot {
                            for slot in pick.slots(current) {
                                let patched = skyward_root::folkpatch::patched_slot(
                                    Path::new("/dev/block/by-name"),
                                    slot,
                                    Path::new("/data/local/tmp/skyward-root"),
                                )
                                .unwrap_or(false);
                                slots[slot.number() as usize] = Some(patched);
                                any |= patched;
                            }
                        }
                        app.set_apatch_patched(any, slots);
                    }
                    // A copy beside the manager, which the user asked for and
                    // was told the cost of.
                    #[cfg(feature = "roots")]
                    Effect::ShareApatchKey { run, key } => {
                        let kept = skyward_root::folkpatch::share_key(&run, &key)
                            .inspect(|path| {
                                let _ = skyward_sys::kmsg(&format!("apatch: the key is copied to {}", path.display()));
                            })
                            .map_err(|e| format!("{e:#}"));
                        app.root_key_shared(kept, now);
                    }
                    // A KernelSU-family manager's run: its release is read and
                    // its APK downloaded off this thread, then its ksud writes
                    // the boot partitions.
                    #[cfg(feature = "roots")]
                    Effect::Ksu { flavor, tier, source, installed, pick } => {
                        ensure!(
                            installer.is_none() && apatch_worker.is_none() && ksu_worker.is_none() && kernel_worker.is_none(),
                            "a run is under way already"
                        );
                        let refused = |app: &mut App, why: String| {
                            app.ksu_finished(
                                skyward_root::ksu::Outcome::Failed(skyward_root::ksu::Failure {
                                    stage: skyward_root::ksu::Stage::Checking,
                                    slot: None,
                                    why,
                                    backups: Vec::new(),
                                    written: Vec::new(),
                                    run_dir: None,
                                }),
                                now,
                            );
                        };
                        if !withhold(&mut transfer, &mut app, now)? {
                            refused(&mut app, "file transfer kept internal storage".into());
                            continue;
                        }
                        let started = (|| -> Result<crate::ksu::Worker> {
                            let current = skyward_vab::current_slot()?.context("not an A/B phone")?;
                            let battery = info.battery.clone().context("the battery was not read")?;
                            let task = crate::ksu::Task {
                                device: cfg.device.codename.clone(),
                                current,
                                slots: pick.slots(current),
                                battery,
                                flavor,
                                tier,
                                source,
                                installed,
                                kmi: cfg.ksu.kmi.clone(),
                                stamp: skyward_ui::fmt::stamp(unix_now()?, app.state.time_zone),
                            };
                            skyward_sys::kmsg(&format!("ksu: patching {} slot(s) with {}", task.slots.len(), task.flavor.id()))?;
                            crate::ksu::Worker::start(task, servers.clone())
                        })();
                        match started {
                            Ok(worker) => ksu_worker = Some(worker),
                            Err(e) => refused(&mut app, format!("{e:#}")),
                        }
                    }
                    // A device's own kernel: its release is read and its
                    // image downloaded off this thread, then written whole to
                    // the boot partitions.
                    #[cfg(feature = "roots")]
                    Effect::Kernel { plugin, pick } => {
                        ensure!(
                            installer.is_none() && apatch_worker.is_none() && ksu_worker.is_none() && kernel_worker.is_none(),
                            "a run is under way already"
                        );
                        let refused = |app: &mut App, why: String| {
                            app.kernel_finished(
                                skyward_root::kernel::Outcome::Failed(skyward_root::kernel::Failure {
                                    stage: skyward_root::kernel::Stage::Checking,
                                    slot: None,
                                    why,
                                    backups: Vec::new(),
                                    written: Vec::new(),
                                    run_dir: None,
                                }),
                                now,
                            );
                        };
                        if !withhold(&mut transfer, &mut app, now)? {
                            refused(&mut app, "file transfer kept internal storage".into());
                            continue;
                        }
                        let started = (|| -> Result<crate::kernel::Worker> {
                            let current = skyward_vab::current_slot()?.context("not an A/B phone")?;
                            let battery = info.battery.clone().context("the battery was not read")?;
                            let task = crate::kernel::Task {
                                device: cfg.device.codename.clone(),
                                current,
                                slots: pick.slots(current),
                                battery,
                                plugin,
                                stamp: skyward_ui::fmt::stamp(unix_now()?, app.state.time_zone),
                            };
                            skyward_sys::kmsg(&format!("kernel: installing {} slot(s)", task.slots.len()))?;
                            crate::kernel::Worker::start(task, servers.clone())
                        })();
                        match started {
                            Ok(worker) => kernel_worker = Some(worker),
                            Err(e) => refused(&mut app, format!("{e:#}")),
                        }
                    }
                    // The newest release's version, read for the kernel page.
                    #[cfg(feature = "roots")]
                    Effect::ReadKernelDetails { plugin } => {
                        if kernel_checker.is_none() {
                            match crate::kernel::Checker::start(plugin, servers.clone()) {
                                Ok(checker) => {
                                    kernel_checker = Some(checker);
                                    kernel_checker_at = now;
                                    kernel_checker_expired = false;
                                }
                                Err(e) => app.report(&format!("{e:#}"), now),
                            }
                        }
                    }
                    // The manager of the family installed in Android, so the
                    // page can offer its APK instead of a download.
                    #[cfg(feature = "roots")]
                    Effect::ReadKsuInstalled => {
                        let installed = skyward_root::ksu::installed::find_all(Path::new("/data/app"));
                        app.set_ksu_installed(installed);
                    }
                    Effect::Cancel => match &installer {
                        Some(i) => i.cancel()?,
                        // The update the system asked for, waiting on the
                        // lock of the user whose files hold it, is given up
                        // as its page is left, and is done with.
                        None => {
                            ensure!(pending.take().is_some(), "no installer to cancel");
                            skyward_sys::kmsg("the update asked for is given up, its page left")?;
                            if kept {
                                bcb::clear(&misc)?;
                                kept = false;
                            }
                            let cancelled = skyward_install::Event::Finished(skyward_install::Outcome::Cancelled);
                            app.install(cancelled, now);
                        }
                    },
                    Effect::ReadMounts => app.set_mounts(mounts()),
                    Effect::ReadChargeLimit => {
                        let limit = charge_db.as_deref().and_then(|db| charging::system_limit(Path::new(db)));
                        app.set_charge_limit(limit);
                    }
                    Effect::ReadSlots => app.set_boot_slots(read_slots()),
                    Effect::SetSlot(slot) => app.slot_switched(set_slot(slot), now),
                    Effect::ReadSystems => systems_worker.as_mut().context("this phone holds no second system")?.send(systems::Task::Read),
                    // An install still under way may yet write the boot
                    // partitions; a switch waits for it.
                    Effect::SwitchSystem(to) => {
                        if installer.is_some() {
                            app.systems_work_failed(INSTALL_UNFINISHED, now);
                            continue;
                        }
                        skyward_sys::kmsg(&format!("switching to the {to:?} system, as the systems page asked"))?;
                        systems_worker.as_mut().context("this phone holds no second system")?.send(systems::Task::Switch(to));
                    }
                    Effect::RemoveSystem => {
                        if installer.is_some() {
                            app.systems_work_failed(INSTALL_UNFINISHED, now);
                            continue;
                        }
                        skyward_sys::kmsg("removing the second system, as the systems page asked")?;
                        systems_worker.as_mut().context("this phone holds no second system")?.send(systems::Task::Remove);
                    }
                    Effect::InstallSystem { size } => installer.as_ref().context("no installer holds a system")?.install_system(size),
                    // The decrypt page shows meanwhile that the work is
                    // under way, and takes nothing more until it is done.
                    Effect::OpenData => {
                        ensure!(decryptor.is_none(), "the data partition is being opened already");
                        decryptor = Some(Decryptor::start(decrypt::Job::Open)?);
                    }
                    Effect::Unlock { user, code } => {
                        ensure!(decryptor.is_none(), "the data partition is being opened already");
                        let job = decrypt::Job::Unlock { closed: closed.clone(), user, credential: code.into_bytes() };
                        decryptor = Some(Decryptor::start(job)?);
                    }
                    Effect::ReadDir(folder) => start_listing(&mut listing, ListingTask::Files(folder))?,
                    Effect::ReadPlace(place, folder) => start_listing(&mut listing, ListingTask::Place(place, folder))?,
                    // A name taken where an entry goes is the user's to
                    // decide on before anything is written.
                    Effect::Paste { op, path, into, clash } => {
                        ensure!(worker.is_none(), "work on files is under way already");
                        let clash = match clash {
                            Some(choice) => clash_rule(choice),
                            None => match skyward_files::clashes(&[PathBuf::from(&path)], Path::new(&into)) {
                                Ok(taken) if taken.is_empty() => skyward_files::Clash::Refuse,
                                Ok(taken) => {
                                    let names = taken.iter().map(|t| t.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())).collect();
                                    app.file_clash(op, path, into, names);
                                    continue;
                                }
                                Err(e) => {
                                    app.report(&format!("{e:#}"), now);
                                    continue;
                                }
                            },
                        };
                        let ownership = match ownership(Path::new(&into)) {
                            Ok(ownership) => ownership,
                            Err(e) => {
                                app.report(&format!("{e:#}"), now);
                                continue;
                            }
                        };
                        let (from, to) = (PathBuf::from(&path), PathBuf::from(&into));
                        let (task, shown) = match op {
                            FileOp::Copy => (filework::Task::Copy { from, into: to, clash, ownership }, FileTask::Copy { path, into }),
                            FileOp::Move => (filework::Task::Move { from, into: to, clash, ownership }, FileTask::Move { path, into }),
                        };
                        worker = Some(filework::Worker::start(task)?);
                        app.file_job_started(shown);
                    }
                    Effect::Delete(path) => {
                        ensure!(worker.is_none(), "work on files is under way already");
                        worker = Some(filework::Worker::start(filework::Task::Delete { path: PathBuf::from(&path) })?);
                        app.file_job_started(FileTask::Delete { path });
                    }
                    Effect::StopJob => worker.as_ref().context("there is no work on files to stop")?.stop(),
                    // Rules kept on a partition that is not mounted are the
                    // page's to report, with what to do first.
                    Effect::Relabel => {
                        ensure!(relabeler.is_none(), "relabeling is under way already");
                        let slot = skyward_vab::current_slot()?;
                        match relabel::rule_files(slot) {
                            Ok(rules) => {
                                relabeler = Some(filework::Worker::start(filework::Task::Relabel { rules })?);
                                app.relabel_started();
                            }
                            Err(e) => match e.downcast_ref::<relabel::Unmounted>() {
                                Some(relabel::Unmounted(base)) => app.report(&skyward_ui::fmt::mount_first(app.state.lang, base), now),
                                None => app.report(&skyward_ui::fmt::rules_unread(app.state.lang, &format!("{e:#}")), now),
                            },
                        }
                    }
                    Effect::StopRelabel => relabeler.as_ref().context("there is no relabeling to stop")?.stop(),
                    // Partitions that cannot be read are the page's to say.
                    Effect::ReadFilesystems => app.set_filesystems(read_filesystems().map_err(|e| format!("{e:#}"))),
                    // The data partition made afresh as another filesystem
                    // is a factory reset, as the system asks for one; the
                    // rest is work on one partition. Opening the data
                    // partition and working on it are not to cross, and file
                    // transfer lets internal storage go first.
                    Effect::Filesystem { name, action } => {
                        ensure!(eraser.is_none() && part_worker.is_none(), "work on partitions is under way already");
                        if name == DATA_PARTITION && decryptor.is_some() {
                            app.report(DATA_OPENING, now);
                            continue;
                        }
                        if name == DATA_PARTITION && !withhold(&mut transfer, &mut app, now)? {
                            continue;
                        }
                        let part = fs_part(&name)?;
                        let (kind, task) = match action {
                            FsAction::Remake(filesystem) if name == DATA_PARTITION => {
                                erase_leaves = false;
                                eraser = Some(factory_reset(&mut app, false, false, Some(filesystem))?);
                                continue;
                            }
                            FsAction::Repair => (PartKind::Repair, partwork::Task::Repair(part)),
                            FsAction::Remake(filesystem) => (PartKind::Remake(filesystem.clone()), partwork::Task::Remake(part, filesystem)),
                        };
                        skyward_sys::kmsg(&format!("{kind:?} the filesystem of {name}, as the filesystem page asked"))?;
                        app.part_work_started(kind, false);
                        part_worker = Some(partwork::Worker::start(task)?);
                    }
                    // An install still stopping may yet write; a backup or
                    // restore waits for it.
                    Effect::Backup { parts, place, compress, verify, sparse } => {
                        ensure!(backuper.is_none(), "a backup or restore is under way already");
                        if installer.is_some() {
                            app.report(INSTALL_UNFINISHED, now);
                            continue;
                        }
                        let into = backups_in(place);
                        let there = into.ancestors().find(|p| p.exists()).context("the root is always there")?;
                        let ownership = match ownership(there) {
                            Ok(ownership) => ownership,
                            Err(e) => {
                                app.report(&format!("{e:#}"), now);
                                continue;
                            }
                        };
                        let made = unix_now()?;
                        let task = backup::BackupTask {
                            parts,
                            stamp: skyward_ui::fmt::stamp(made, app.state.time_zone),
                            into,
                            ownership,
                            compress,
                            verify,
                            sparse,
                            remount: (place == Place::UsbDrive).then(|| PathBuf::from(USB_DRIVE)),
                            made,
                            device: cfg.device.codename.clone(),
                            system: info.os_version.clone(),
                            efs: cfg.backup.efs.clone(),
                            skipped: backup::data_skipped(&cfg.backup.data_skipped),
                        };
                        backuper = Some(backup::Worker::start(backup::Task::Backup(task))?);
                        app.backup_started(false, place, verify);
                    }
                    Effect::Restore { place, name, parts } => {
                        ensure!(backuper.is_none(), "a backup or restore is under way already");
                        // A restore of the system's partitions takes them off
                        // to write over them, and what runs from them with
                        // it: they are brought back once it ends, so the
                        // recovery goes on working with what it restored.
                        reattach = parts[1];
                        if installer.is_some() {
                            app.report(INSTALL_UNFINISHED, now);
                            continue;
                        }
                        let task = backup::RestoreTask {
                            folder: backups_in(place).join(&name),
                            parts,
                            device: cfg.device.codename.clone(),
                            efs: cfg.backup.efs.clone(),
                            skipped: backup::data_skipped(&cfg.backup.data_skipped),
                            detach: detach(cfg.services.clone()),
                        };
                        backuper = Some(backup::Worker::start(backup::Task::Restore(task))?);
                        app.backup_started(true, place, true);
                    }
                    Effect::FindBackups { storage, usb, sdcard } => find_backups(&mut app, storage, usb, sdcard, now),
                    Effect::DeleteBackup { place, name } => {
                        let deleted = delete_backup(place, &backups_in(place).join(&name)).map_err(|e| format!("{e:#}"));
                        queue.extend(app.backup_deleted(deleted, now));
                    }
                    Effect::StopBackup => backuper.as_ref().context("there is no backup or restore to stop")?.stop(),
                    // A phone that is not A/B has no update of the other
                    // slot to look at.
                    Effect::ReadUpdate => {
                        if let Some(slot) = info.slot {
                            // A merge followed tells where it stands itself.
                            if merger.is_none() {
                                app.set_update(skyward_vab::standing(slot)?);
                            }
                            // An update in the other slot starts with that
                            // slot, which a system's updater may not have set
                            // yet.
                            app.set_boot_slots(read_slots());
                        }
                    }
                    Effect::Merge => {
                        ensure!(merger.is_none(), "the update is being merged already");
                        let slot = info.slot.context("only an A/B phone merges an update")?;
                        skyward_sys::kmsg("merging the update, as the install page asked")?;
                        merger = Some(merge::Merger::start(slot, true)?);
                    }
                    Effect::Wlan(on) => {
                        wlan_wanted = on;
                        wlan_worker(&wlan)?.send(if on { wlan::Command::On } else { wlan::Command::Off })
                    }
                    // adbd takes the port a moment after init is asked to restart it; the row tells what came
                    // of it.
                    Effect::AdbWifi(on) => match if on { adbwifi::listen() } else { adbwifi::stop() } {
                        Ok(()) if on => adb_until = Some(Instant::now() + ADB_WAIT),
                        Ok(()) => app.set_adb_wifi(AdbWifi::Off),
                        Err(e) => {
                            skyward_sys::kmsg(&format!("adbd was not asked for the network's port: {e:#}"))?;
                            app.set_adb_wifi(AdbWifi::Failed(format!("{e:#}")))
                        }
                    },
                    Effect::ScanWlan => {
                        scanned_at = Instant::now();
                        wlan_worker(&wlan)?.send(wlan::Command::Scan);
                    }
                    Effect::JoinWlan { ssid, name, security, password } => {
                        wlan_worker(&wlan)?.send(wlan::Command::Join { ssid, name, security, password: password.0.clone() })
                    }
                    Effect::LeaveWlan => wlan_worker(&wlan)?.send(wlan::Command::Leave),
                    // What the installed system is called is the page's to
                    // report when it cannot be read.
                    Effect::CheckSkyward => {
                        ensure!(checker.is_none(), "the releases are being asked already");
                        let update = cfg.update.as_ref().context("this phone's releases are not known")?;
                        let probe = cfg.wlan.as_ref().context("the phone's WLAN is not brought up")?.probe.clone();
                        let task = updates::Task::Check {
                            repo: update.repo.clone(),
                            asset: update.asset.clone(),
                            build: skyward_info::BUILD.to_string(),
                            probe,
                        };
                        checker = Some(updates::Worker::start(task, servers.clone())?);
                    }
                    // The image goes into the recovery's own folder, which is
                    // in memory, and is written over the recovery once whole.
                    Effect::UpdateSkyward(build) => {
                        ensure!(downloader.is_none(), "the image is being downloaded already");
                        ensure!(installer.is_none(), "an installer is running already");
                        skyward_sys::kmsg(&format!("updating Skyward to {}", build.name))?;
                        let path = downloaded_build(&build);
                        let task = updates::Task::Download { build, path };
                        downloader = Some(updates::Worker::start(task, servers.clone())?);
                    }
                    Effect::StopSkywardUpdate => downloader.as_ref().context("there is no download to stop")?.stop(),
                    // The ROM's own feed, asked for the newest system it
                    // publishes, and its package downloaded beside the
                    // recovery's.
                    Effect::CheckRom => {
                        ensure!(rom_checker.is_none(), "the ROM's feed is being asked already");
                        let rom = cfg.rom.as_ref().context("this phone's ROM feed is not known")?;
                        let probe = cfg.wlan.as_ref().context("the phone's WLAN is not brought up")?.probe.clone();
                        let task = updates::Task::RomCheck {
                            feed: rom.feed.clone(),
                            installed: info.os_version.clone().unwrap_or_default(),
                            flavor: rom.flavor.clone(),
                            probe,
                        };
                        rom_checker = Some(updates::Worker::start(task, servers.clone())?);
                    }
                    Effect::DownloadRom(release) => {
                        ensure!(rom_downloader.is_none(), "the ROM's package is being downloaded already");
                        let path = PathBuf::from(rom_downloaded(&release));
                        let task = updates::Task::RomDownload { release, path };
                        rom_downloader = Some(updates::Worker::start(task, servers.clone())?);
                    }
                    Effect::StopRomDownload => rom_downloader.as_ref().context("there is no ROM download to stop")?.stop(),
                    // Logs that cannot be saved are the page's to report;
                    // the recovery carries on.
                    Effect::ExportLogs { recovery, kernel, storage } => {
                        let into = match storage {
                            true => Path::new(skyward_crypto::data::SDCARD).join(STORAGE_LOGS),
                            false => PathBuf::from(TMP_LOGS),
                        };
                        match export_logs(recovery, kernel, &into, app.state.time_zone) {
                            Ok(folder) => app.report(&skyward_ui::fmt::exported(app.state.lang, &folder.display().to_string()), now),
                            Err(e) => app.report(&skyward_ui::fmt::export_failed(app.state.lang, &format!("{e:#}")), now),
                        }
                    }
                    Effect::SetMode { path, mode } => {
                        let outcome = skyward_files::set_mode(Path::new(&path), mode).map_err(|e| format!("{e:#}"));
                        app.mode_set(mode, outcome, now);
                    }
                    Effect::OpenText(path) => {
                        let read = skyward_files::read_up_to(Path::new(&path), skyward_ui::editor::MAX_BYTES).map_err(|e| format!("{e:#}"));
                        app.text_opened(&fonts, path, read, now);
                    }
                    // The folder is listed again, for the file's new size
                    // and time.
                    Effect::SaveText { path, text } => match skyward_files::replace(Path::new(&path), text.as_bytes()) {
                        Ok(()) => {
                            app.text_saved(Ok(()), now);
                            let folder = app.state.files.path.clone();
                            start_listing(&mut listing, ListingTask::Files(folder))?;
                        }
                        Err(e) => app.text_saved(Err(format!("{e:#}")), now),
                    },
                    Effect::Rename { path, name } => match skyward_files::rename(Path::new(&path), &name) {
                        Ok(_) => {
                            app.renamed(Ok(()), now);
                            let folder = app.state.files.path.clone();
                            start_listing(&mut listing, ListingTask::Files(folder))?;
                        }
                        Err(e) if e.is::<skyward_files::Taken>() => app.renamed(Err(RenameRefused::Taken), now),
                        Err(e) => app.renamed(Err(RenameRefused::Failed(format!("{e:#}"))), now),
                    },
                    // A partition that will not mount, or is in use, is the
                    // page's to report; the recovery carries on.
                    Effect::Mount(base) => {
                        let slot = skyward_vab::current_slot()?;
                        if let Err(e) = skyward_vab::mount_one(slot, &base) {
                            app.mount_failed(MountFailure { mounting: true, base, detail: format!("{e:#}") });
                        }
                        app.set_mounts(mounts());
                    }
                    Effect::Unmount(base) => {
                        let slot = skyward_vab::current_slot()?;
                        if let Err(e) = skyward_vab::unmount_one(slot, &base) {
                            app.mount_failed(MountFailure { mounting: false, base, detail: format!("{e:#}") });
                        }
                        app.set_mounts(mounts());
                    }
                    // init stops fastbootd as the port goes through "none";
                    // then the slot's partitions come up as at boot.
                    Effect::EnterRecovery => {
                        skyward_sys::usb::switch(skyward_sys::usb::resting(), USB_TIMEOUT)?;
                        if let Some(found) = fastboot_found.take() {
                            let asked = bcb::read(&misc)?;
                            if asked != found {
                                if let Some(target) = left_for(&asked) {
                                    skyward_sys::kmsg(&format!("fastbootd asked the phone to leave for {target:?}"))?;
                                    power(target)?;
                                    return Ok(());
                                }
                            }
                        }
                        let slot = skyward_vab::current_slot()?;
                        crate::prepare(cfg, slot)?;
                        read_at = Instant::now();
                        info = skyward_info::collect(cfg)?;
                        fastboot = false;
                        if wlan_wanted {
                            match &wlan {
                                Some(w) => w.send(wlan::Command::On),
                                None => {
                                    let _ = skyward_sys::kmsg("the WLAN was on, and this phone's recovery brings none up");
                                }
                            }
                        }
                        app.enter_recovery();
                        if let Some(w) = systems_worker.as_mut() {
                            w.send(systems::Task::Resume);
                        }
                    }
                    // A port that something else has takes file transfer
                    // on, or leaves it, when it comes back to rest.
                    Effect::FileTransfer(on) => {
                        let rests = !fastboot && !sideloading;
                        match (on, transfer.take()) {
                            (true, None) => match Transfer::start(cfg, &info, app.state.time_zone) {
                                Ok(t) => {
                                    transfer = Some(t);
                                    if rests {
                                        if let Err(e) = skyward_sys::usb::switch(skyward_sys::usb::resting(), USB_TIMEOUT) {
                                            skyward_sys::kmsg(&format!("the port did not take file transfer: {e:#}"))?;
                                            app.report(&format!("{e:#}"), now);
                                        }
                                    }
                                }
                                Err(e) => {
                                    skyward_sys::kmsg(&format!("file transfer did not start: {e:#}"))?;
                                    app.file_transfer_failed(false, &skyward_ui::fmt::transfer_failed(app.state.lang, &format!("{e:#}")), now);
                                }
                            },
                            (false, Some(t)) => {
                                if let Err(e) = t.stop(rests) {
                                    skyward_sys::kmsg(&format!("file transfer did not stop cleanly: {e:#}"))?;
                                    app.report(&skyward_ui::fmt::transfer_left_on(app.state.lang, &format!("{e:#}")), now);
                                }
                            }
                            // The switch already stands where it was asked
                            // to: a second tap before the first took effect,
                            // or settings read again, is no reason to stop.
                            (true, Some(t)) => transfer = Some(t),
                            (false, None) => {}
                        }
                    }
                }
            }
        }
        // Internal storage goes to the computer while it is open, and no
        // erasing or installing could take it away. File transfer that does
        // not follow, or whose service ended, stops, and the page says why.
        let kept = match &mut transfer {
            Some(t) if !t.service.running() => Err(anyhow!("the file transfer service ended")),
            Some(t) => {
                let offer = app.state.data.storage_open() && eraser.is_none() && installer.is_none();
                t.offer(offer, app.state.lang).and_then(|()| {
                    if t.zone != app.state.time_zone {
                        t.service.set_zone(skyward_ui::fmt::rules(app.state.time_zone).clone())?;
                        t.zone = app.state.time_zone;
                    }
                    Ok(())
                })
            }
            None => Ok(()),
        };
        if let Err(e) = kept {
            // An ended service's own error says more than its ending does.
            let why = match transfer.take().expect("only file transfer that runs fails").stop(!fastboot && !sideloading) {
                Err(cause) => cause,
                Ok(()) => e,
            };
            skyward_sys::kmsg(&format!("file transfer stops: {why:#}"))?;
            app.file_transfer_failed(false, &skyward_ui::fmt::transfer_stopped(app.state.lang, &format!("{why:#}")), now);
        }
    }
}
