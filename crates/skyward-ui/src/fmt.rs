use crate::app::{
    BackupRefusal, Data, EraseStop, FileKind, FileRow, ItemEnd, MountFailure, Part, PartKind, PartStop, Place, Source, TermEnd, BACKUP_PARTS,
    TIME_ZONES,
};
use crate::lang::{Lang, Text};
use crate::strings::*;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use jiff::Timestamp;
use skyward_info::{Battery, Charge, Cpu, PatchLevel, Storage};
use skyward_install::ota::Trouble;
use skyward_install::{Failure, ImageKind, Ramdisk, Refusal};
use skyward_net::download::Failure as DownloadFailure;
use skyward_net::reach::Reach;
use skyward_net::release::Unanswered;
use skyward_net::supplicant::Security;
use skyward_net::wlan::{Bands, Joined, Power, Stage as WlanStage, Wlan};
use skyward_vab::{Hold, Slot, SnapshotState, Standing};
use std::sync::OnceLock;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Marketed RAM size: the smallest standard module size that holds `MemTotal`.
pub fn ram(kib: u64) -> String {
    const SIZES: [f64; 16] = [1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 10.0, 12.0, 16.0, 18.0, 20.0, 24.0, 32.0, 48.0, 64.0];
    let gib = kib as f64 / (1024.0 * 1024.0);
    let size = SIZES.iter().copied().find(|&s| s >= gib).unwrap_or(gib.ceil());
    format!("{size:.1} GB")
}

/// Marketed storage size in GB: the smallest power of two (in GiB) that holds
/// the userdata partition.
pub fn capacity_gb(total: u64) -> u64 {
    let gib = total as f64 / GIB;
    let mut gb = 8;
    while (gb as f64) < gib {
        gb *= 2;
    }
    gb
}

pub fn capacity(total: u64) -> String {
    match capacity_gb(total) {
        gb if gb >= 1024 => format!("{}TB", gb / 1024),
        gb => format!("{gb}GB"),
    }
}

pub fn gb(bytes: u64) -> String {
    format!("{:.1}GB", bytes as f64 / 1e9)
}

/// `used/total` as the stock settings count it: everything that is not free
/// is used, against the marketed size.
pub fn storage_used(s: &Storage) -> Option<String> {
    let free = s.free?;
    let used = capacity_gb(s.total) as f64 - free as f64 / 1e9;
    Some(format!("{used:.1}GB/{}", capacity(s.total)))
}

/// `Octa-core, up to 3.19GHz`, `Octa-core, up to 3.19GHz`
pub fn cpu(_l: Lang, c: &Cpu) -> String {
    let cores = match c.cores {
        1 => "Single-core".into(),
        2 => "Dual-core".into(),
        4 => "Quad-core".into(),
        6 => "Hexa-core".into(),
        8 => "Octa-core".into(),
        n => format!("{n}-core"),
    };
    match c.max_khz {
        Some(khz) => {
            let ghz = format!("{:.2}", khz as f64 / 1e6);
            crate::t!(S_UP_TO_GHZ, cores = cores, ghz = ghz)
        }
        None => cores,
    }
}

/// A marketing name laid out the way the stock settings stack it in large
/// type: an edition credit ("by Leica") starts its own line.
pub fn model_lines(name: &str) -> String {
    name.replacen(" by ", "\nby ", 1)
}

pub fn panel((w, h): (u32, u32)) -> String {
    format!("{} x {}", w.max(h), w.min(h))
}

pub fn inches(d: f32) -> String {
    format!("{d}″")
}

pub fn mah(uah: u64) -> String {
    format!("{}mAh", uah / 1000)
}

pub fn charge(_l: Lang, c: Charge) -> &'static str {
    match c {
        Charge::Charging => S_CHARGING,
        Charge::Discharging | Charge::NotCharging => S_NOT_CHARGING,
        Charge::Full => S_FULL,
        Charge::Unknown => S_UNKNOWN,
    }
}

/// `74% · Charging`
pub fn level(l: Lang, b: &Battery) -> String {
    format!("{}% · {}", b.level, charge(l, b.charge))
}

pub fn celsius(tenths: i32) -> String {
    format!("{}°C", degrees(tenths))
}

/// A temperature given in tenths of a degree, to the tenth: `35.2`.
pub fn degrees(tenths: i32) -> String {
    format!("{:.1}", tenths as f64 / 10.0)
}

pub fn volts(uv: u64) -> String {
    format!("{:.2}V", uv as f64 / 1e6)
}

/// The kernel's battery health in the language; a state it does not list
/// shows as reported.
pub fn health(_l: Lang, h: &str) -> String {
    let text = match h {
        "Good" => S_GOOD,
        "Overheat" | "Hot" => S_OVERHEATED,
        "Warm" => S_WARM,
        "Cool" => S_COOL,
        "Cold" => S_COLD,
        "Dead" => S_DEAD,
        "Over voltage" => S_OVER_VOLTAGE,
        "Over current" => S_OVER_CURRENT,
        "Unspecified failure" => S_FAILURE,
        "Calibration required" => S_NEEDS_CALIBRATION,
        "No battery" => S_NO_BATTERY,
        "Unknown" => S_UNKNOWN,
        other => return other.to_string(),
    };
    text.to_string()
}

/// Health and remaining capacity against design, e.g. `Good · 93% of design capacity`.
pub fn wear(l: Lang, b: &Battery) -> Option<String> {
    let state = b.health.as_deref().map(|h| health(l, h));
    let left = match (b.full, b.design) {
        (Some(full), Some(design)) if design > 0 => {
            let share = (full * 100 + design / 2) / design;
            Some(crate::t!(S_MAXIMUM_CAPACITY, share = share))
        }
        _ => None,
    };
    match (state, left) {
        (Some(s), Some(l)) => Some(format!("{s} · {l}")),
        (s, l) => s.or(l),
    }
}

pub fn android(l: Lang, release: &str, patch_level: Option<&PatchLevel>) -> String {
    match patch_level {
        Some(p) => {
            let date = patch(l, p);
            crate::t!(S_ANDROID_SECURITY_UPDATE, release = release, date = date)
        }
        None => release.to_string(),
    }
}

pub fn verified(_l: Lang, state: &str) -> String {
    let text = match state {
        "green" => S_GREEN_VERIFIED_WITH_THE_OFFICIAL_KEY,
        "yellow" => S_YELLOW_VERIFIED_WITH_A_CUSTOM_KEY,
        "orange" => S_ORANGE_UNLOCKED_SO_THE_SYSTEM_ISNT_VERIFIED,
        "red" => S_RED_VERIFICATION_FAILED,
        other => return other.to_string(),
    };
    text.to_string()
}

pub fn snapshot(_l: Lang, s: &SnapshotState) -> &'static str {
    match s {
        SnapshotState::None => S_NO_UPDATE_WAITING_TO_MERGE,
        SnapshotState::Snapshotted => S_UPDATE_WRITTEN_WAITING_TO_MERGE,
        SnapshotState::Merging => S_MERGING_THE_UPDATE,
        SnapshotState::MergeFailed => S_THE_UPDATE_FAILED_TO_MERGE,
        SnapshotState::Unknown => S_COULDNT_READ_THE_UPDATES_STATUS,
    }
}

/// Why super is off limits while an update is in flight; `None` when it is not.
pub fn snapshot_note(_l: Lang, s: &SnapshotState) -> Option<&'static str> {
    let text = match s {
        SnapshotState::None => return None,
        SnapshotState::Snapshotted | SnapshotState::Merging => {
            S_PARTITIONS_IN_SUPER_ARENT_FLASHED_UNTIL_THE_UPDATE_MERGES
        }
        SnapshotState::MergeFailed => S_PARTITIONS_IN_SUPER_ARENT_FLASHED_UNTIL_THE_SYSTEM_DEALS_WITH_THE_UPDATE,
        SnapshotState::Unknown => S_PARTITIONS_IN_SUPER_ARENT_FLASHED_UNTIL_THE_UPDATES_STATUS_CAN_BE_READ,
    };
    Some(text)
}

/// The home page's warning of a battery too low to install on: how low it
/// is, and what installing waits for.
pub fn battery_low(_l: Lang, level: u8, charging: bool) -> (String, String) {
    let least = skyward_install::least_charge(charging);
    (
        crate::t!(S_BATTERY_AT, level = level),
        crate::t!(S_CHARGE_IT_PAST_TO_INSTALL, least = least),
    )
}

/// The home page's word that the phone restarts from the slot it does not
/// run from now.
pub fn next_boot(_l: Lang, next: Slot, now: Slot) -> (String, String) {
    let (next, now) = (next.letter(), now.letter());
    (
        crate::t!(S_RESTARTS_FROM_SLOT, next = next),
        crate::t!(S_RUNNING_FROM_SLOT_NOW, now = now),
    )
}

/// Why the slot to start next stays as an update in flight set it; `None`
/// when none is.
pub fn snapshot_slot_note(_l: Lang, s: &SnapshotState) -> Option<&'static str> {
    let text = match s {
        SnapshotState::None => return None,
        SnapshotState::Snapshotted | SnapshotState::Merging => {
            S_THE_BOOT_SLOT_ISNT_SWITCHED_UNTIL_THE_UPDATE_MERGES
        }
        SnapshotState::MergeFailed => {
            S_THE_BOOT_SLOT_ISNT_SWITCHED_UNTIL_THE_SYSTEM_DEALS_WITH_THE_UPDATE
        }
        SnapshotState::Unknown => {
            S_THE_BOOT_SLOT_ISNT_SWITCHED_UNTIL_THE_UPDATES_STATUS_CAN_BE_READ
        }
    };
    Some(text)
}

/// What an install script that writes to super does to an update in
/// flight; `None` when none is known to be.
pub fn snapshot_script_note(_l: Lang, s: &SnapshotState) -> Option<&'static str> {
    let text = match s {
        SnapshotState::None | SnapshotState::Unknown => return None,
        SnapshotState::Snapshotted | SnapshotState::Merging => S_A_SYSTEM_UPDATE_HASNT_FINISHED_MERGING_A_FLASHABLE_ZIP_THAT_REWRITES_PARTITIONS_IN_SUPER_KEEPS_IT_FROM_EVER_FINISHING,
        SnapshotState::MergeFailed => S_A_SYSTEM_UPDATE_FAILED_TO_MERGE_AND_THE_SYSTEM_HAS_TO_DEAL_WITH_IT_A_FLASHABLE_ZIP_THAT_REWRITES_PARTITIONS_IN_SUPER_KEEPS_IT_FROM_EVER_FINISHING,
    };
    Some(text)
}

/// Why the slot's partitions in super are not backed up while an update is
/// in flight; `None` when none is.
pub fn snapshot_backup_note(_l: Lang, s: &SnapshotState) -> Option<&'static str> {
    let text = match s {
        SnapshotState::None => return None,
        SnapshotState::Snapshotted | SnapshotState::Merging => {
            S_SYSTEM_PARTITIONS_ARENT_BACKED_UP_UNTIL_THE_UPDATE_MERGES
        }
        SnapshotState::MergeFailed => {
            S_SYSTEM_PARTITIONS_ARENT_BACKED_UP_UNTIL_THE_SYSTEM_DEALS_WITH_THE_UPDATE
        }
        SnapshotState::Unknown => {
            S_SYSTEM_PARTITIONS_ARENT_BACKED_UP_UNTIL_THE_UPDATES_STATUS_CAN_BE_READ
        }
    };
    Some(text)
}

/// Why backing up, or restoring, did not start.
pub fn backup_refusal(_l: Lang, r: &BackupRefusal, restore: bool) -> String {
    match r {
        BackupRefusal::Room { needed, free } => {
            let (needed, free) = (gb(*needed), gb(*free));
            crate::t!(S_NOT_ENOUGH_STORAGE_THE_BACKUP_NEEDS_AND_IS_FREE, needed = needed, free = free)
        }
        BackupRefusal::Update(state) => {
            let what = match restore {
                true => S_RESTORED,
                false => S_BACKED_UP,
            };
            let parts = if restore { "The boot and system partitions" } else { "System partitions" };
            match state {
                SnapshotState::Snapshotted | SnapshotState::Merging => crate::t!(S_ARENT_UNTIL_THE_UPDATE_MERGES_YOU_CAN_SEE_IT_IN_INSTALL, parts = parts, what = what),
                SnapshotState::MergeFailed => crate::t!(S_THE_UPDATE_FAILED_TO_MERGE_ARENT_UNTIL_THE_SYSTEM_DEALS_WITH_IT, parts = parts, what = what),
                SnapshotState::Unknown => crate::t!(S_ARENT_WHILE_THE_UPDATES_STATUS_CANT_BE_READ, parts = parts, what = what),
                SnapshotState::None => unreachable!("no update in flight holds anything back"),
            }
        }
        BackupRefusal::OtherPhone(device) => crate::t!(S_THIS_BACKUP_IS_FROM_ANOTHER_DEVICE_AND_CANT_BE_RESTORED_TO_THIS_PHONE, device = device),
        BackupRefusal::Resized(name) => crate::t!(S_ISNT_THE_SIZE_IT_WAS_AT_THE_BACKUP_SO_THE_SYSTEM_MAY_HAVE_BEEN_UPDATED_THE_BACKUPS_SYSTEM_PARTITIONS_CANT_BE_RESTORED_TO_THE_CURRENT_SYSTEM, name = name),
        BackupRefusal::DataReset => S_THE_DATA_PARTITION_WAS_MADE_ANEW_AFTER_THE_BACKUP_AS_A_FACTORY_RESET_DOES_SO_THE_BACKUPS_DATA_PARTITION_CANT_BE_RESTORED
            .into(),
    }
}

/// What the install page says of the system update standing so, and the
/// line under it on what comes next; `next` is the slot the bootloader
/// starts next, where the boot control could tell.
pub fn update(_l: Lang, s: &Standing, next: Option<Slot>) -> (&'static str, Option<String>) {
    let line = |text: Text| Some(text.to_string());
    match s {
        Standing::None => (S_NO_UPDATE_WAITING_TO_MERGE, None),
        Standing::Unknown => (
            S_COULDNT_READ_THE_UPDATES_STATUS,
            line(S_COULDNT_READ_THE_UPDATES_RECORDS_IN_METADATA),
        ),
        Standing::Writing => (
            S_THE_UPDATE_ISNT_FULLY_WRITTEN,
            line(S_INSTALLING_GOES_ON_ONCE_THE_SYSTEM_STARTS),
        ),
        Standing::Installed(installed) => {
            let slot = installed.letter();
            let detail = match next {
                Some(next) if next == *installed => crate::t!(S_AFTER_A_REBOOT_THE_NEW_SYSTEM_STARTS_FROM_SLOT_AND_MERGES_ONCE_IT_HAS_STARTED, slot = slot),
                Some(_) => crate::t!(S_REBOOT_FROM_THE_SYSTEMS_SYSTEM_UPDATE_TO_START_SLOT_S_NEW_SYSTEM, slot = slot),
                None => crate::t!(S_MERGES_ONCE_SLOT_S_NEW_SYSTEM_STARTS, slot = slot),
            };
            (S_UPDATE_INSTALLED, Some(detail))
        }
        Standing::Waiting => (
            S_UPDATE_WAITING_TO_MERGE,
            line(S_MERGES_ONCE_THE_NEW_SYSTEM_STARTS_OR_CAN_BE_MERGED_HERE),
        ),
        Standing::Compressed => (
            S_UPDATE_WAITING_TO_MERGE,
            line(S_AN_UPDATE_OF_THIS_KIND_MERGES_IN_THE_SYSTEM_ONCE_IT_STARTS),
        ),
        Standing::Merging(_) => (S_MERGING_THE_UPDATE, None),
        Standing::Failed(code) => (
            S_THE_UPDATE_FAILED_TO_MERGE,
            Some(crate::t!(S_MERGING_GOES_ON_AFTER_A_REBOOT, code = format!("{code:?}"))),
        ),
        Standing::Merged => (
            S_UPDATE_MERGED,
            line(S_THE_FILES_THE_UPDATE_LEFT_ARE_CLEARED_WHEN_THE_SYSTEM_NEXT_STARTS),
        ),
    }
}

/// Where an update standing so is, beside the entry of its page.
/// A build's stamp (`2026-10-30T19:37:12Z`) as the page shows it: the day,
/// then the time to the minute; a build that was never stamped says so.
pub fn build_date(_l: Lang, stamp: &str) -> String {
    match stamp.get(..16) {
        Some(head) if stamp.len() == 20 && stamp.ends_with('Z') && stamp.as_bytes().get(10) == Some(&b'T') => {
            format!("{} {}", &head[..10], &head[11..16])
        }
        _ => S_UNKNOWN.to_string(),
    }
}

/// Why writing the downloaded image over this recovery failed.
pub fn install_failed(_l: Lang, why: &str) -> String {
    crate::t!(S_COULDNT_WRITE_IT, why = why)
}

pub fn data(_l: Lang, d: &Data) -> &'static str {
    match d {
        Data::Closed | Data::Opening | Data::Failed(_) => S_NOT_MOUNTED,
        Data::Locked { .. } => S_LOCKED,
        Data::Open => S_DECRYPTED,
        Data::Erased => S_WIPED,
    }
}

/// A part of the phone erasing goes through, as its page names it.
pub fn part(_l: Lang, p: Part) -> &'static str {
    match p {
        Part::Data => S_DATA_PARTITION,
        Part::Cache => S_CACHE_PARTITION,
        Part::Metadata => S_METADATA_PARTITION,
        Part::AppData => S_APP_DATA,
        Part::Media => S_INTERNAL_STORAGE,
        Part::AppCaches => S_APP_CACHES,
        Part::SystemCaches => S_SYSTEM_CACHES,
        Part::Art => S_ART_CACHE,
        Part::Logs => S_LOGS,
        Part::Updates => S_DOWNLOADED_UPDATES,
    }
}

/// A part of the phone erasing goes through, as an English sentence names
/// it.
fn part_in_sentence(p: Part) -> &'static str {
    match p {
        Part::Data => "the data partition",
        Part::Cache => "the cache partition",
        Part::Metadata => "the metadata partition",
        Part::AppData => "app data",
        Part::Media => "internal storage",
        Part::AppCaches => "app caches",
        Part::SystemCaches => "system caches",
        Part::Art => "the ART cache",
        Part::Logs => "the logs",
        Part::Updates => "the downloaded updates",
    }
}

/// A name that opens an English title, as it reads within a sentence: its
/// first letter small, unless the word is written all in capitals.
fn in_sentence(name: &str) -> String {
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(second)) if first.is_uppercase() && !second.is_uppercase() => {
            first.to_lowercase().chain(name[first.len_utf8()..].chars()).collect()
        }
        _ => name.to_string(),
    }
}

/// Things named in a row: in Chinese with 、 between them, in English with
/// commas, each after the first as a sentence names it.
pub fn list<'a>(l: Lang, names: impl IntoIterator<Item = &'a str>) -> String {
    let names: Vec<String> = names.into_iter().enumerate().map(|(k, n)| if l == Lang::En && k > 0 { in_sentence(n) } else { n.to_string() }).collect();
    names.join(S_TEXT_4)
}

/// Users by how the pages call them: the owner, then the others by number,
/// as「the owner, user 999」, or as an English sentence names them, as "the owner
/// and user 999".
pub fn users(_l: Lang, users: &[u32]) -> String {
    let names: Vec<String> = users
        .iter()
        .map(|&u| match u {
            0 => S_THE_OWNER.to_string(),
            u => crate::t!(S_USER, u = u),
        })
        .collect();
    match names.as_slice() {
        [.., last] if names.len() > 1 => format!("{} and {last}", names[..names.len() - 1].join(", ")),
        _ => names.join(""),
    }
}

/// Why erasing did not start or stopped short, and what the system said of
/// it when there is that.
pub fn erase_stop(l: Lang, s: &EraseStop) -> (String, Option<String>) {
    let said = |text: Text| text.to_string();
    match s {
        EraseStop::Update(Hold::Unconfirmed) => (
            said(S_THE_CURRENT_SLOTS_SYSTEM_UPDATE_HASNT_FINISHED_SO_NOTHING_CAN_BE_WIPED_YET_MERGE_IT_IN_INSTALL_THEN_TRY_AGAIN),
            None,
        ),
        EraseStop::Update(Hold::Merging) => (
            said(S_A_SYSTEM_UPDATE_IS_MERGING_SO_NOTHING_CAN_BE_WIPED_YET_YOU_CAN_SEE_IT_IN_INSTALL_TRY_AGAIN_ONCE_IT_HAS_MERGED),
            None,
        ),
        EraseStop::Update(Hold::MergeFailed) => (
            said(S_A_SYSTEM_UPDATE_FAILED_TO_MERGE_SO_NOTHING_CAN_BE_WIPED_YET_START_THE_SYSTEM_TO_DEAL_WITH_IT_THEN_TRY_AGAIN),
            None,
        ),
        EraseStop::NotRebuilt => (
            said(S_THIS_PHONES_SYSTEM_DOESNT_MAKE_AN_EMPTIED_DATA_PARTITION_ANEW_WHEN_IT_STARTS_SO_IT_CANT_BE_WIPED),
            None,
        ),
        EraseStop::NoCache => (said(S_THIS_PHONE_HAS_NO_CACHE_PARTITION), None),
        EraseStop::Erased => (
            said(S_THE_DATA_PARTITION_IS_EMPTY_AND_WILL_BE_MADE_ANEW_WHEN_THE_PHONE_NEXT_STARTS_THERES_NOTHING_TO_WIPE),
            None,
        ),
        EraseStop::Closed(why) => (
            said(S_COULDNT_OPEN_THE_DATA_PARTITION_SO_NOTHING_WAS_WIPED),
            Some(why.clone()),
        ),
        EraseStop::Locked(locked) => {
            let who = users(l, locked);
            let what = format!("The data of {who} isn't decrypted, so nothing was wiped. Enter the screen lock in Decrypt, then try again.");
            (what, None)
        }
        EraseStop::Failed(p, why) => {
            let what = format!("Couldn't wipe {}.", part_in_sentence(*p));
            (what, Some(why.clone()))
        }
    }
}

/// How work on whole partitions of a kind is told of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verb {
    /// Under way: Repairing.
    going: &'static str,
    /// Done to a partition: repaired.
    done: &'static str,
    /// The work as a thing: Repair, as in Repair failed.
    noun: &'static str,
    /// The verb itself: repair, as in Couldn't repair vendor.
    base: &'static str,
}

impl Verb {
    pub fn of(kind: &PartKind) -> Verb {
        let (going, done, noun, base) = match kind {
            PartKind::Disable => ("Wiping", "wiped", "Wipe", "wipe"),
            PartKind::Unshare => ("Unsharing", "unshared", "Unsharing", "unshare"),
            PartKind::Repair => ("Repairing", "repaired", "Repair", "repair"),
            PartKind::Remake(_) => ("Formatting", "formatted", "Format", "format"),
        };
        Verb { going, done, noun, base }
    }

    /// Under way: Repairing.
    pub fn going(self, _l: Lang) -> String {
        crate::t!(S_TEXT, going = self.going)
    }

    /// Ended, with every partition done: Repair complete.
    pub fn complete(self, _l: Lang) -> String {
        crate::t!(S_COMPLETE, noun = self.noun)
    }

    /// Ended partway through the partitions: Wipe incomplete.
    pub fn incomplete(self, _l: Lang) -> String {
        crate::t!(S_INCOMPLETE, noun = self.noun)
    }

    /// Gone wrong: Repair failed.
    pub fn failed(self, _l: Lang) -> String {
        crate::t!(S_FAILED, noun = self.noun)
    }

    /// A partition the work ended before: Not repaired.
    pub fn not_done(self, _l: Lang) -> String {
        crate::t!(S_NOT, done = self.done)
    }

    /// What the page asks while the work goes on.
    pub fn hold_on(self, _l: Lang) -> String {
        S_DONT_POWER_OFF_UNTIL_ITS_DONE.to_string()
    }

    /// What could not be done to the partition `name`.
    pub fn failed_on(self, _l: Lang, name: &str) -> String {
        format!("Couldn't {} {name}", self.base)
    }
}

/// How a partition's work ended, as its row says it, `verb` telling of the
/// work: Repaired, Repair failed.
pub fn item_end(l: Lang, end: &ItemEnd, verb: Verb) -> String {
    let text = match end {
        ItemEnd::Erased => S_WIPED,
        ItemEnd::Unshared => S_UNSHARED,
        ItemEnd::NotShared(_) => S_NOTHING_TO_DO,
        ItemEnd::Clean => S_NO_ERRORS_FOUND,
        ItemEnd::Fixed => S_REPAIRED,
        ItemEnd::Made => S_FORMATTED,
        ItemEnd::Failed(_) => return verb.failed(l),
    };
    text.to_string()
}

/// Why work on whole partitions stopped before going through them, on a
/// phone whose codename is `codename`, and the details behind it when
/// there are any.
pub fn part_stop(l: Lang, stop: &PartStop, codename: &str) -> (String, Option<String>) {
    match stop {
        PartStop::Refused(r) => (erase_request_refusal(l, r, codename), None),
        PartStop::Unmounted(base) => (
            crate::t!(S_MOUNT_THE_PARTITION_IN_MOUNT_THEN_TRY_AGAIN, base = base),
            None,
        ),
        PartStop::Failed(why) => (UNEXPECTED.into(), Some(why.clone())),
    }
}

/// What the pages say of an error they have no words of their own for,
/// above what went wrong.
const UNEXPECTED: Text = S_SOMETHING_WENT_WRONG;

/// A file size, `7.1GB` or `96.0MB`.
pub fn bytes(n: u64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.1}GB", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{:.1}MB", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.0}KB", n as f64 / 1e3),
        n => format!("{n}B"),
    }
}

/// The months, as English names them.
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// A day of the calendar as the language writes it: `May 8, 2026`,
/// `May 8, 2026`.
fn day(_l: Lang, year: i32, month: i32, d: i32) -> String {
    let name = MONTHS[(month - 1) as usize];
    crate::t!(S_TEXT_2, name = name, d = d, year = year)
}

/// `April 1, 2026`
pub fn patch(l: Lang, p: &PatchLevel) -> String {
    day(l, p.year.into(), p.month.into(), p.day.into())
}

/// The rules of each of [`TIME_ZONES`], read once from the ones the
/// recovery carries.
pub fn rules(zone: usize) -> &'static TimeZone {
    static RULES: OnceLock<Vec<TimeZone>> = OnceLock::new();
    let all = RULES.get_or_init(|| {
        TIME_ZONES.iter().map(|&(_, _, name)| TimeZone::get(name).expect("the recovery carries the rules of every zone listed")).collect()
    });
    &all[zone]
}

/// A Unix time on the calendar and clock of the zone at `zone` in
/// [`TIME_ZONES`]. A time past either end of the calendar, as a damaged
/// file's can be, is taken as that end.
fn local(unix: i64, zone: usize) -> DateTime {
    let unix = unix.clamp(Timestamp::MIN.as_second(), Timestamp::MAX.as_second());
    let at = Timestamp::from_second(unix).expect("a time within the calendar");
    at.to_zoned(rules(zone).clone()).datetime()
}

/// The day a Unix time falls on in the zone at `zone` in [`TIME_ZONES`],
/// `May 8, 2026`.
pub fn date(l: Lang, unix: i64, zone: usize) -> String {
    let d = local(unix, zone);
    day(l, d.year().into(), d.month().into(), d.day().into())
}

/// When a list entry last changed, as the stock file manager tells it
/// against `now`, when the folder was read: the time for today, month and
/// day for a file from this year, and the whole date for a folder from this
/// year or anything older; all in the zone at `zone` in [`TIME_ZONES`].
pub fn file_time(unix: i64, folder: bool, now: i64, zone: usize) -> String {
    let (day, today) = (local(unix, zone).date(), local(now, zone).date());
    if day == today {
        clock(unix, zone)
    } else if day.year() == today.year() && !folder {
        format!("{:02}/{:02}", day.month(), day.day())
    } else {
        format!("{}/{:02}/{:02}", day.year(), day.month(), day.day())
    }
}

/// A file's size as the stock file manager writes it: in steps of 1000,
/// going up a unit past 900, with at most two decimals and no trailing
/// zeros.
pub fn file_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let (mut v, mut unit) = (bytes as f64, 0);
    while v > 900.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    let hundredths = (v * 100.0).round() as u64;
    let n = match hundredths {
        h if h % 100 == 0 => format!("{v:.0}"),
        h if h % 10 == 0 => format!("{v:.1}"),
        _ => format!("{v:.2}"),
    };
    format!("{n} {}", UNITS[unit])
}

/// A Unix time in the zone at `zone` in [`TIME_ZONES`], to the minute:
/// `October 19, 2026, 02:31`, `October 19, 2026, 02:31`.
pub fn datetime(l: Lang, unix: i64, zone: usize) -> String {
    let (date, clock) = (date(l, unix, zone), clock(unix, zone));
    crate::t!(S_TEXT_3, date = date, clock = clock)
}

/// The time of day in the zone at `zone` in [`TIME_ZONES`], `HH:MM`, as
/// the status bar shows it.
pub fn clock(unix: i64, zone: usize) -> String {
    let t = local(unix, zone);
    format!("{:02}:{:02}", t.hour(), t.minute())
}

/// A Unix time in the zone at `zone` in [`TIME_ZONES`], to the second, as
/// the name of what is saved then, which sorts by it: `20261021-133000`.
pub fn stamp(unix: i64, zone: usize) -> String {
    let t = local(unix, zone);
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", t.year(), t.month(), t.day(), t.hour(), t.minute(), t.second())
}

/// A file's type and permissions as `ls -l` prints them: `drwxr-xr-x`.
pub fn mode(mode: u32) -> String {
    let kind = match mode & 0o170_000 {
        0o040_000 => 'd',
        0o120_000 => 'l',
        0o020_000 => 'c',
        0o060_000 => 'b',
        0o010_000 => 'p',
        0o140_000 => 's',
        _ => '-',
    };
    let mut out = String::from(kind);
    // The set-user, set-group and sticky bits show in each class's x.
    for (shift, special, set, unset) in [(6, 0o4000, 's', 'S'), (3, 0o2000, 's', 'S'), (0, 0o1000, 't', 'T')] {
        let bits = (mode >> shift) & 7;
        out.push(if bits & 4 != 0 { 'r' } else { '-' });
        out.push(if bits & 2 != 0 { 'w' } else { '-' });
        out.push(match (bits & 1 != 0, mode & special != 0) {
            (true, true) => set,
            (false, true) => unset,
            (true, false) => 'x',
            (false, false) => '-',
        });
    }
    out
}

/// The line under an entry's name in the file list, laid out as the stock
/// file manager lays it out, with dates told against `now`, when the folder
/// was read, in the zone at `zone` in [`TIME_ZONES`].
pub fn file_line(l: Lang, f: &FileRow, now: i64, zone: usize) -> String {
    const DIVIDER: &str = "  |  ";
    match &f.kind {
        FileKind::Folder { items: Some(n) } => {
            let items = match (l, n) {
                (Lang::En, 1) => "1 item".to_string(),
                _ => crate::t!(S_ITEMS, n = n),
            };
            format!("{}{DIVIDER}{items}", file_time(f.modified, true, now, zone))
        }
        FileKind::Folder { items: None } => S_UNREADABLE.to_string(),
        FileKind::File => format!("{}{DIVIDER}{}", file_time(f.modified, false, now, zone), file_size(f.size)),
        FileKind::Link { target, .. } => crate::t!(S_LINK_TO, target = target),
        FileKind::Special => {
            let what = match f.mode & 0o170_000 {
                0o020_000 => S_CHARACTER_DEVICE,
                0o060_000 => S_BLOCK_DEVICE,
                0o010_000 => S_PIPE,
                _ => S_SOCKET,
            };
            format!("{}{DIVIDER}{what}", file_time(f.modified, false, now, zone))
        }
    }
}

/// The parts a backup holds: `boot, system`.
pub fn parts(l: Lang, held: &[bool; 4]) -> String {
    list(l, BACKUP_PARTS.iter().zip(held).filter(|(_, &on)| on).map(|(name, _)| *name))
}

/// What an image is, and what it holds.
pub fn image_kind(_l: Lang, kind: ImageKind) -> (&'static str, &'static str) {
    const RECOVERY: Text = S_RECOVERY_IMAGE;
    const BOOT: Text = S_BOOT_IMAGE;
    let (name, holds) = match kind {
        ImageKind::Boot { kernel: true, ramdisk: Ramdisk::Recovery } => {
            (RECOVERY, S_HOLDS_A_KERNEL_AND_A_RECOVERY_RAMDISK)
        }
        ImageKind::Boot { kernel: false, ramdisk: Ramdisk::Recovery } => {
            (RECOVERY, S_HOLDS_A_RECOVERY_RAMDISK_ALONE)
        }
        ImageKind::Boot { kernel: true, ramdisk: Ramdisk::Generic } => {
            (BOOT, S_HOLDS_A_KERNEL_AND_A_GENERIC_RAMDISK)
        }
        ImageKind::Boot { kernel: true, ramdisk: Ramdisk::None } => (BOOT, S_HOLDS_A_KERNEL_ALONE),
        ImageKind::Boot { kernel: false, .. } => (BOOT, S_HOLDS_A_GENERIC_RAMDISK_ALONE),
        ImageKind::VendorBoot => {
            (S_VENDOR_BOOT_IMAGE, S_HOLDS_THE_VENDOR_RAMDISK_AND_DEVICE_TREE)
        }
        ImageKind::Dtbo => (S_DTBO_IMAGE, S_HOLDS_DEVICE_TREE_OVERLAYS),
        ImageKind::Vbmeta => (S_VBMETA_IMAGE, S_HOLDS_VERIFIED_BOOT_DATA),
    };
    (name, holds)
}

/// What a partition an image can go to is for.
pub fn partition_role(_l: Lang, name: &str) -> &'static str {
    match name {
        "boot" => S_KERNEL,
        "init_boot" => S_GENERIC_RAMDISK,
        "recovery" => S_RECOVERY,
        "vendor_boot" => S_VENDOR_RAMDISK_AND_DEVICE_TREE,
        "dtbo" => S_DEVICE_TREE_OVERLAYS,
        "vbmeta" => S_VERIFIED_BOOT_DATA,
        "vbmeta_system" => S_VERIFICATION_DATA_OF_THE_SYSTEM_PARTITIONS,
        "vbmeta_vendor" => S_VERIFICATION_DATA_OF_THE_VENDOR_PARTITIONS,
        _ => S_PARTITION,
    }
}

/// `slot B`, `slots A and B`, `slot B`, `slots A and B`
pub fn slots(_l: Lang, slots: &[Slot]) -> String {
    let letters: Vec<&str> = slots.iter().map(|s| s.letter()).collect();
    let joined = letters.join(S_AND);
    if slots.len() == 1 {
        format!("slot {joined}")
    } else {
        format!("slots {joined}")
    }
}

/// How long the screen stays on with nothing touched: `30 seconds`, `2 minutes`,
/// or `Never`.
pub fn screen_off(l: Lang, after_ms: Option<u64>) -> String {
    match after_ms {
        None => S_NEVER.into(),
        Some(ms) if ms < 60_000 => match (l, ms / 1000) {
            (Lang::En, 1) => "1 second".into(),
            (_, n) => crate::t!(S_SECONDS, n = n),
        },
        Some(ms) => match (l, ms / 60_000) {
            (Lang::En, 1) => "1 minute".into(),
            (_, n) => crate::t!(S_MINUTES, n = n),
        },
    }
}

/// A mount or unmount that failed, with why: `Couldn't mount vendor: …`
pub fn mount_failure(_l: Lang, m: &MountFailure) -> String {
    let (base, detail) = (&m.base, &m.detail);
    match m.mounting {
        true => crate::t!(S_COULDNT_MOUNT, base = base, detail = detail),
        false => crate::t!(S_COULDNT_UNMOUNT, base = base, detail = detail),
    }
}

/// How another package is given to the installer from where this one came
/// from: the computer sends it, or another file is chosen, as one is in
/// place of a package the system named. English starts its sentence with it.
fn give(_l: Lang, source: Source) -> &'static str {
    match source {
        Source::Computer | Source::Rescue => S_SEND,
        Source::File(_) | Source::Opened { .. } | Source::Asked { .. } => S_CHOOSE,
    }
}

/// Why a package from `source` was not installed, for a phone with this
/// codename, its dates in the zone at `zone` in [`TIME_ZONES`].
pub fn refusal(l: Lang, r: &Refusal, codename: &str, source: Source, zone: usize) -> String {
    let give = give(l, source);
    match r {
        Refusal::Unknown => crate::t!(S_THIS_FILE_ISNT_RECOGNIZED_AN_A_B_FULL_OTA_PACKAGE_A_FLASHABLE_ZIP_OR_A_PARTITION_IMAGE, give = give),
        Refusal::Image => crate::t!(S_A_PARTITION_IMAGE_CANT_BE_INSTALLED_THIS_WAY_AN_A_B_FULL_OTA_PACKAGE_OR_A_FLASHABLE_ZIP, give = give),
        Refusal::NotAb(kind) if kind.is_empty() => crate::t!(S_THIS_PACKAGE_DOESNT_SAY_ITS_TYPE_AN_A_B_FULL_OTA_PACKAGE, give = give),
        Refusal::NotAb(kind) => crate::t!(S_THIS_PACKAGES_TYPE_IS_AN_A_B_FULL_OTA_PACKAGE, kind = kind, give = give),
        Refusal::OtherDevice(devices) => crate::t!(S_THIS_PACKAGE_IS_FOR_AND_THIS_PHONE_IS, devices = devices, codename = codename),
        Refusal::OtherSerial => S_THIS_PACKAGE_IS_ONLY_FOR_DEVICES_WITH_CERTAIN_SERIAL_NUMBERS_AND_THIS_PHONE_ISNT_ONE_OF_THEM
            .into(),
        Refusal::Incremental => crate::t!(S_AN_INCREMENTAL_OTA_PACKAGE_CANT_BE_INSTALLED_A_FULL_OTA_PACKAGE, give = give),
        Refusal::Older { package, installed } => {
            let (package, installed) = (date(l, *package, zone), date(l, *installed, zone));
            crate::t!(S_THIS_PACKAGE_WAS_BUILT_ON_BEFORE_THE_CURRENT_SYSTEM_BUILT_ON, package = package, installed = installed)
        }
        Refusal::OlderPatch { package, installed } => crate::t!(S_THIS_PACKAGES_SECURITY_PATCH_IS_OLDER_THAN_THE_CURRENT_SYSTEMS, package = package, installed = installed),
        Refusal::NoPartition => S_THIS_PHONE_HAS_NO_PARTITION_THIS_IMAGE_CAN_BE_FLASHED_TO.into(),
        Refusal::Battery { level, charging } => {
            let least = skyward_install::least_charge(*charging);
            crate::t!(S_THE_BATTERY_IS_AT_CHARGE_IT_PAST_THEN_TRY_AGAIN, level = level, least = least)
        }
        Refusal::Pending(state) => {
            let standing = snapshot(l, state);
            crate::t!(S_REBOOT_TO_THE_SYSTEM_TO_FINISH_THE_LAST_UPDATE_THEN_TRY_AGAIN, standing = standing)
        }
        Refusal::OldHost => S_THE_COMPUTERS_ADB_IS_TOO_OLD_UPDATE_ANDROID_SDK_PLATFORM_TOOLS_THEN_TRY_AGAIN
            .into(),
        Refusal::Unsigned => S_THIS_ZIP_ISNT_SIGNED_TURN_OFF_VERIFY_ZIP_SIGNATURES_THEN_TRY_AGAIN
            .into(),
        Refusal::Unverified => S_THIS_ZIPS_SIGNATURE_DIDNT_VERIFY_TURN_OFF_VERIFY_ZIP_SIGNATURES_THEN_TRY_AGAIN
            .into(),
        Refusal::Panicked => S_THIS_UPDATE_CANT_BE_INSTALLED_NOW_REBOOT_TO_THE_SYSTEM_THEN_TRY_AGAIN
            .into(),
        Refusal::NotBrick(_) | Refusal::NoSerial => unreachable!("only a request to erase partitions is refused as {r:?}"),
    }
}

/// Why the system's request to erase the partitions it lists was not
/// carried out, on a phone whose codename is `codename`.
pub fn erase_request_refusal(_l: Lang, r: &Refusal, codename: &str) -> String {
    match r {
        Refusal::Unsigned => S_THE_REQUEST_ISNT_SIGNED.into(),
        Refusal::Unverified => S_THE_REQUESTS_SIGNATURE_DIDNT_VERIFY.into(),
        Refusal::NotBrick(kind) if kind.is_empty() => S_THE_REQUEST_DOESNT_SAY_ITS_TYPE.into(),
        Refusal::NotBrick(kind) => crate::t!(S_THE_REQUESTS_TYPE_IS, kind = kind),
        Refusal::OtherDevice(devices) => {
            crate::t!(S_THE_REQUEST_IS_FOR_AND_THIS_PHONE_IS, devices = devices, codename = codename)
        }
        Refusal::OtherSerial => S_THE_REQUEST_IS_ONLY_FOR_DEVICES_WITH_CERTAIN_SERIAL_NUMBERS_AND_THIS_PHONE_ISNT_ONE_OF_THEM
            .into(),
        Refusal::NoSerial => S_THE_REQUEST_NAMES_NO_SERIAL_NUMBER.into(),
        other => unreachable!("a request to erase partitions is not refused as {other:?}"),
    }
}

/// What went wrong installing a package from `source`, and the details
/// behind it when there are any.
pub fn failure(l: Lang, f: &Failure, source: Source) -> (String, Option<String>) {
    let said = |text: Text| text.to_string();
    match f {
        Failure::Engine(code) => {
            let what = match (Trouble::of(*code), source) {
                (Trouble::Signature, _) => {
                    let give = give(l, source);
                    crate::t!(S_THE_PACKAGES_SIGNATURE_DIDNT_VERIFY_AN_OFFICIAL_PACKAGE, give = give)
                }
                (Trouble::Damaged, _) => said(S_THE_PACKAGE_IS_DAMAGED_DOWNLOAD_IT_AGAIN),
                (Trouble::Older, _) => said(S_THE_PACKAGE_IS_TOO_OLD),
                (Trouble::NoSpace, _) => said(S_NOT_ENOUGH_SPACE_IN_THE_SUPER_PARTITION),
                (Trouble::Transfer, Source::Computer | Source::Rescue) => said(S_COULDNT_READ_THE_PACKAGE_CHECK_THE_CABLE_THEN_SEND_IT_AGAIN),
                (Trouble::Transfer, Source::File(Place::Storage) | Source::Opened { place: Place::Storage } | Source::Asked { .. }) => {
                    said(S_COULDNT_READ_THE_PACKAGE)
                }
                (Trouble::Transfer, Source::File(Place::UsbDrive) | Source::Opened { place: Place::UsbDrive }) => {
                    said(S_COULDNT_READ_THE_PACKAGE_CHECK_THE_USB_DRIVE_THEN_TRY_AGAIN)
                }
                (Trouble::Transfer, Source::File(Place::Cloud) | Source::Opened { place: Place::Cloud }) => {
                    said(S_COULDNT_READ_THE_PACKAGE_CHECK_THE_CLOUD_CONNECTION_THEN_TRY_AGAIN)
                }
                (Trouble::Write, _) => said(S_COULDNT_WRITE_A_PARTITION),
                (Trouble::Verify, _) => said(S_A_PARTITION_DIDNT_VERIFY),
                (Trouble::PostInstall, _) => said(S_COULDNT_FINISH_INSTALLING),
                (Trouble::Unsupported, _) => said(S_THIS_PACKAGES_FORMAT_ISNT_SUPPORTED),
                (Trouble::Other, _) => said(S_THE_UPDATER_RAN_INTO_AN_ERROR),
            };
            (what, Some(crate::t!(S_ERROR_CODE, code = code)))
        }
        Failure::Script(code) => {
            (crate::t!(S_THE_INSTALL_SCRIPT_FAILED_WITH_EXIT_CODE, code = code), None)
        }
        Failure::Disconnected => (
            said(S_THE_COMPUTER_STOPPED_SENDING_CHECK_THE_CABLE_THEN_SEND_IT_AGAIN),
            None,
        ),
        Failure::Unreadable(why) => match source {
            Source::File(Place::UsbDrive) | Source::Opened { place: Place::UsbDrive } => (
                said(S_COULDNT_READ_THE_FILE_CHECK_THE_USB_DRIVE_THEN_TRY_AGAIN),
                Some(why.clone()),
            ),
            Source::File(Place::Cloud) | Source::Opened { place: Place::Cloud } => (
                said(S_COULDNT_READ_THE_FILE_CHECK_THE_CLOUD_CONNECTION_THEN_TRY_AGAIN),
                Some(why.clone()),
            ),
            _ => (said(S_COULDNT_READ_THE_FILE), Some(why.clone())),
        },
        Failure::Changed => match source {
            Source::Computer | Source::Rescue => {
                (said(S_THE_FILE_CHANGED_WHILE_IT_WAS_SENT_SEND_IT_AGAIN), None)
            }
            Source::File(_) | Source::Opened { .. } | Source::Asked { .. } => {
                (said(S_THE_FILE_CHANGED_WHILE_IT_WAS_READ_CHOOSE_IT_AGAIN), None)
            }
        },
        Failure::Error(detail) => (said(UNEXPECTED), Some(detail.clone())),
    }
}

/// How many of a WLAN fan's three arcs a signal of `dbm` lights.
pub fn signal_level(dbm: i32) -> u8 {
    match dbm {
        d if d >= -60 => 3,
        d if d >= -70 => 2,
        d if d >= -80 => 1,
        _ => 0,
    }
}

/// How fast data goes, `bytes` a second, as the stock status bar writes it
/// over its unit: in KB/s up to 999 and in MB/s past that, to three
/// figures, two of them after the point below 10 and one below 100.
pub fn speed(bytes: u64) -> (String, &'static str) {
    let kb = bytes as f64 / 1024.0;
    let (v, unit) = if kb < 999.5 { (kb, "KB/s") } else { (kb / 1024.0, "MB/s") };
    let figures = if v < 9.995 {
        format!("{v:.2}")
    } else if v < 99.95 {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    };
    (figures, unit)
}

/// A signal's strength in words, as the stock network details say it.
pub fn signal(_l: Lang, dbm: i32) -> &'static str {
    match signal_level(dbm) {
        3 => S_EXCELLENT,
        2 => S_GOOD,
        1 => S_FAIR,
        _ => S_WEAK,
    }
}

/// The bands a network is on, as the tag after its name: none for 2.4 GHz
/// alone, as the stock WLAN page shows it.
pub fn band_tag(b: Bands) -> Option<String> {
    let names: Vec<&str> = [(b.ghz2, "2.4G"), (b.ghz5, "5G"), (b.ghz6, "6G")].into_iter().filter_map(|(on, n)| on.then_some(n)).collect();
    (names != ["2.4G"] && !names.is_empty()).then(|| names.join("/"))
}

/// The bands a network is on, in full.
pub fn bands(l: Lang, b: Bands) -> String {
    list(l, [(b.ghz2, "2.4 GHz"), (b.ghz5, "5 GHz"), (b.ghz6, "6 GHz")].into_iter().filter_map(|(on, n)| on.then_some(n)))
}

/// A generation of Wi-Fi, as the stock network details say it.
pub fn generation(_l: Lang, n: u8) -> String {
    crate::t!(S_WI_FI, n = n)
}

/// How a network is protected, as the system's network details name it.
pub fn security(_l: Lang, s: Security) -> &'static str {
    match s {
        Security::Open => S_NONE,
        Security::Owe => S_ENHANCED_OPEN,
        Security::Wep => "WEP",
        Security::Psk => "WPA3-Personal",
        Security::Sae => "WPA3-Personal",
        Security::PskSae => "WPA2/WPA3-Personal",
        Security::Enterprise => "WPA/WPA2-Enterprise",
    }
}

/// What the home page says beside its WLAN entry: the network joined, or
/// how the WLAN stands.
pub fn wlan_value(_l: Lang, w: &Wlan) -> &str {
    match (&w.power, &w.joined) {
        (Power::Off | Power::Failed(_), _) => S_OFF,
        (Power::Starting, _) => S_TURNING_ON,
        (Power::On, Some(Joined { stage: WlanStage::Connected(_), name, .. })) => name,
        (Power::On, Some(_)) => S_CONNECTING,
        (Power::On, None) => S_NOT_CONNECTED,
    }
}

/// How joining the network chosen goes, under its name; joined, how far it
/// reaches, as the system's WLAN page says it.
pub fn joining(_l: Lang, stage: &WlanStage) -> &'static str {
    match stage {
        WlanStage::Joining => S_CONNECTING,
        WlanStage::Addressing => S_GETTING_AN_IP_ADDRESS,
        WlanStage::Connected(link) => match link.reach {
            Some(Reach::Login(_)) => S_SIGN_IN_REQUIRED,
            Some(Reach::Offline(_)) => S_CONNECTED_NO_INTERNET,
            Some(Reach::Online) | None => S_CONNECTED,
        },
        WlanStage::WrongPassword => S_WRONG_PASSWORD,
        WlanStage::NotFound => S_NETWORK_NOT_FOUND,
        WlanStage::Failed => S_COULDNT_CONNECT,
        WlanStage::NoAddress(_) => S_COULDNT_GET_AN_IP_ADDRESS,
    }
}

/// What a network that wants a login in a browser first leaves to do,
/// with a recovery that has no browser to log in with.
const LOGIN: Text = S_NEEDS_A_SIGN_IN_IN_A_BROWSER_BEFORE_IT_REACHES_THE_INTERNET_AND_SKYWARD_HAS_NO_BROWSER_TO_SIGN_IN_WITH_USE_A_NETWORK_THAT_NEEDS_NO_SIGN_IN_INSTEAD_SUCH_AS_A_PHONES_HOTSPOT;

/// What the WLAN page says under the network joined of how far it reaches,
/// where it reaches short of the internet.
pub fn reach(_l: Lang, reach: &Reach) -> Option<String> {
    let login = LOGIN;
    match reach {
        Reach::Online => None,
        Reach::Login(_) => Some(crate::t!(S_THIS_NETWORK, login = login)),
        Reach::Offline(why) => Some(crate::t!(S_THIS_NETWORK_DOESNT_REACH_THE_INTERNET, why = why)),
    }
}

/// Why the update server gave no answer, as the update page says it.
pub fn unanswered(_l: Lang, why: &Unanswered) -> String {
    let login = LOGIN;
    match why {
        Unanswered::Login => crate::t!(S_COULDNT_CHECK_FOR_UPDATES_THE_WI_FI_NETWORK_JOINED, login = login),
        Unanswered::Offline(why) => crate::t!(S_COULDNT_CHECK_FOR_UPDATES_THE_WI_FI_NETWORK_JOINED_DOESNT_REACH_THE_INTERNET, why = why),
        Unanswered::Failed(why) => crate::t!(S_COULDNT_CHECK_FOR_UPDATES, why = why),
    }
}

/// A size the update server gives, such as 7.1G, with its unit spelled
/// out; one in another form shows as it came.
pub fn package_size(size: &str) -> String {
    match size.char_indices().last() {
        Some((at, unit @ ('K' | 'M' | 'G' | 'T'))) if size[..at].parse::<f64>().is_ok() => format!("{} {unit}B", &size[..at]),
        _ => size.to_string(),
    }
}

/// Why downloading the full package did not end with it whole.
pub fn download_failure(_l: Lang, f: &DownloadFailure) -> String {
    match f {
        DownloadFailure::NoRoom { needed, free } => {
            let (needed, free) = (file_size(*needed), file_size(*free));
            crate::t!(S_NOT_ENOUGH_INTERNAL_STORAGE_HAS_TO_BE_FREE_AND_IS, needed = needed, free = free)
        }
        DownloadFailure::Damaged => S_THE_IMAGE_DIDNT_VERIFY_AND_WAS_DELETED_DOWNLOAD_IT_AGAIN
            .into(),
        DownloadFailure::Changed => {
            S_THAT_RELEASE_HAS_BEEN_REPLACED_CHECK_FOR_UPDATES_AGAIN.into()
        }
        DownloadFailure::Other(why) => crate::t!(S_DOWNLOAD_FAILED, why = why),
    }
}

/// How the terminal page's shell ended.
pub fn term_end(_l: Lang, end: &TermEnd) -> String {
    match end {
        TermEnd::Exited(code) => crate::t!(S_THE_SHELL_EXITED_WITH_CODE, code = code),
        TermEnd::Signalled(signal) => crate::t!(S_SIGNAL_STOPPED_THE_SHELL, signal = signal),
        TermEnd::Other(status) => crate::t!(S_THE_SHELL_ENDED, status = status),
    }
}

/// What a computer is told internal storage is called, as the stock system
/// names it.
pub fn storage_name(_l: Lang) -> &'static str {
    S_INTERNAL_SHARED_STORAGE
}

/// File transfer kept internal storage from work that needed it back, as
/// the system says why.
pub fn transfer_kept(_l: Lang, why: &str) -> String {
    crate::t!(S_COULDNT_STOP_FILE_TRANSFER, why = why)
}

/// File transfer did not turn on, as the system says why.
pub fn transfer_failed(_l: Lang, why: &str) -> String {
    crate::t!(S_COULDNT_TURN_ON_FILE_TRANSFER, why = why)
}

/// File transfer did not turn off, as the system says why.
pub fn transfer_left_on(_l: Lang, why: &str) -> String {
    crate::t!(S_COULDNT_TURN_OFF_FILE_TRANSFER, why = why)
}

/// File transfer stopped on its own, as the system says why.
pub fn transfer_stopped(_l: Lang, why: &str) -> String {
    crate::t!(S_FILE_TRANSFER_STOPPED, why = why)
}

/// Merging the update failed, as the system says why.
pub fn merge_failed(_l: Lang, why: &str) -> String {
    crate::t!(S_COULDNT_MERGE_THE_UPDATE, why = why)
}

/// What work that takes a program of the installed system's `base`
/// partition asks while that partition is not mounted.
pub fn mount_first(_l: Lang, base: &str) -> String {
    crate::t!(S_MOUNT_THE_PARTITION_IN_MOUNT_FIRST, base = base)
}

/// The installed system's SELinux rules could not be read, as the system
/// says why.
pub fn rules_unread(_l: Lang, why: &str) -> String {
    crate::t!(S_COULDNT_READ_THE_SYSTEMS_SELINUX_RULES, why = why)
}

/// The logs were saved in `folder`.
pub fn exported(_l: Lang, folder: &str) -> String {
    crate::t!(S_EXPORTED_TO, folder = folder)
}

/// The logs were not saved, as the system says why.
pub fn export_failed(_l: Lang, why: &str) -> String {
    crate::t!(S_COULDNT_EXPORT, why = why)
}

/// A screenshot went into the gallery's screenshots.
pub const SCREENSHOT_SAVED: Text = S_SCREENSHOT_SAVED_TO_THE_GALLERY;

/// A screenshot is held until the owner's storage opens, and then goes into
/// the gallery's.
pub const SCREENSHOT_HELD: Text = S_SCREENSHOT_KEPT_IT_GOES_TO_THE_GALLERY_ONCE_DECRYPTED;

/// A screenshot was not saved, as the system says why.
pub fn screenshot_failed(_l: Lang, why: &str) -> String {
    crate::t!(S_COULDNT_SAVE_THE_SCREENSHOT, why = why)
}

/// The screenshots held while the owner's storage was closed did not go into
/// the gallery's, as the system says why.
pub fn screenshots_not_kept(_l: Lang, why: &str) -> String {
    crate::t!(S_KEPT_SCREENSHOTS_COULDNT_GO_TO_THE_GALLERY, why = why)
}

/// The name a screenshot taken `unix_ms` milliseconds into Unix time is saved
/// under, in the zone at `zone` in [`TIME_ZONES`]: as the system names its
/// own, the time to the millisecond and then where it was taken,
/// `Screenshot_2026-10-26-01-45-00-123_Skyward.png`.
pub fn screenshot_name(unix_ms: i64, zone: usize) -> String {
    let t = local(unix_ms.div_euclid(1000), zone);
    format!(
        "Screenshot_{:04}-{:02}-{:02}-{:02}-{:02}-{:02}-{:03}_Skyward.png",
        t.year(),
        t.month(),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        unix_ms.rem_euclid(1000)
    )
}

/// What the terminal is told while a command it runs holds it.
pub const TERMINAL_BUSY: Text = S_THE_TERMINAL_IS_RUNNING_A_COMMAND_END_IT_THEN_TRY_AGAIN;

#[cfg(test)]
mod tests {
    use super::*;

    const HANS: Lang = Lang::En;

    // Xiaomi 13 Ultra (ishtar, 16 GB + 512 GB): readings from `skyward info`
    // and the strings its stock About pages print for them.
    // Xiaomi 17 Ultra (nezha, 16 GB + 1 TB): MemTotal from its boot log and
    // the userdata size from `fastboot getvar all`.
    #[test]
    fn ram_rounds_to_module_size() {
        assert_eq!(ram(15_556_972), "16.0 GB");
        assert_eq!(ram(15_472_464), "16.0 GB");
    }

    #[test]
    fn storage_matches_the_stock_settings() {
        let ishtar = Storage { total: 496_550_785_024, free: Some(61_300_000_000) };
        assert_eq!(capacity(ishtar.total), "512GB");
        assert_eq!(storage_used(&ishtar).unwrap(), "450.7GB/512GB");
        assert_eq!(capacity(0xF2_091F_B000), "1TB");
        assert_eq!(storage_used(&Storage { total: 1, free: None }), None);
    }

    #[test]
    fn cpu_summary() {
        let nezha = Cpu { cores: 8, max_khz: Some(4_608_000) };
        assert_eq!(cpu(HANS, &Cpu { cores: 8, max_khz: Some(3_187_200) }), "Octa-core, up to 3.19GHz");
        assert_eq!(cpu(HANS, &nezha), "Octa-core, up to 4.61GHz");
        assert_eq!(cpu(HANS, &Cpu { cores: 2, max_khz: None }), "Dual-core");
        assert_eq!(cpu(Lang::En, &Cpu { cores: 12, max_khz: None }), "12-core");
    }

    // Xiaomi 13 Ultra battery as `skyward info` read it.

    // Xiaomi 17 Ultra build and panel, as its stock settings print them.




    #[test]
    fn a_screenshot_is_named_as_the_system_names_its_own() {
        assert_eq!(screenshot_name(1_792_348_265_007, 0), "Screenshot_2026-10-19-02-31-05-007_Skyward.png");
        assert_eq!(screenshot_name(1_792_348_265_999, zone("UTC")), "Screenshot_2026-10-18-18-31-05-999_Skyward.png", "in the zone chosen");
    }

    /// Where a zone named in the tz database is in [`TIME_ZONES`].
    fn zone(name: &str) -> usize {
        TIME_ZONES.iter().position(|&(_, _, n)| n == name).expect("a zone listed")
    }



    #[test]
    fn modes_read_as_ls_prints_them() {
        assert_eq!(mode(0o040_755), "drwxr-xr-x");
        assert_eq!(mode(0o100_644), "-rw-r--r--");
        assert_eq!(mode(0o120_777), "lrwxrwxrwx");
        assert_eq!(mode(0o041_777), "drwxrwxrwt", "the sticky /tmp");
        assert_eq!(mode(0o104_755), "-rwsr-xr-x", "set-user on an executable");
        assert_eq!(mode(0o020_600), "crw-------");
    }


    #[test]
    fn list_dates_are_told_against_the_day_the_folder_was_read() {
        // 2026-10-19 02:31 in Beijing.
        let at = 1_792_348_260;
        assert_eq!(file_time(at, false, at + 3600, 0), "02:31", "today: the time");
        assert_eq!(file_time(at, true, at + 3600, 0), "02:31", "a folder today too");
        assert_eq!(file_time(at, false, at + 86_400, 0), "10/19", "this year: a file's month and day");
        assert_eq!(file_time(at, true, at + 86_400, 0), "2026/10/19", "this year: a folder's whole date");
        assert_eq!(file_time(at, false, at + 90 * 86_400, 0), "2026/10/19", "another year: the whole date");
        // 2026-10-18 23:59 UTC is already the 19th in Beijing.
        assert_eq!(file_time(1_792_367_940, false, at, 0), "07:59");
        // In London the file changed on the evening of the 18th, the day
        // before.
        assert_eq!(file_time(1_792_348_260, false, 1_792_367_940, zone("Europe/London")), "10/18");
    }

    #[test]
    fn sizes_go_up_a_unit_past_900_with_two_decimals_at_most() {
        assert_eq!(file_size(0), "0 B");
        assert_eq!(file_size(900), "900 B");
        assert_eq!(file_size(901), "0.9 KB");
        assert_eq!(file_size(42_633), "42.63 KB");
        assert_eq!(file_size(26_950), "26.95 KB");
        assert_eq!(file_size(1_090_000), "1.09 MB");
        assert_eq!(file_size(634_960), "634.96 KB");
        assert_eq!(file_size(2_000_000_000), "2 GB");
        assert_eq!(file_size(1_500_000), "1.5 MB");
    }









    #[test]
    fn a_speed_is_written_to_three_figures_over_its_unit() {
        let s = |bytes| {
            let (figures, unit) = speed(bytes);
            format!("{figures} {unit}")
        };
        // As the 17 Ultra's status bar showed slow ones.
        assert_eq!(s(0), "0.00 KB/s");
        assert_eq!(s(61), "0.06 KB/s");
        assert_eq!(s(7188), "7.02 KB/s");
        assert_eq!(s(12_800), "12.5 KB/s");
        assert_eq!(s(512_000), "500 KB/s");
        assert_eq!(s(1_023_000), "999 KB/s");
        assert_eq!(s(1_024_000), "0.98 MB/s", "past 999 KB/s the unit grows");
        assert_eq!(s(42_772_090), "40.8 MB/s");
    }


}
