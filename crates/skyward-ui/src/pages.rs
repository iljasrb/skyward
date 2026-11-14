use crate::app::{
    Action, AdbWifi, BackupAt, BackupEnd, BackupJob, ClashChoice, Confirm, Damage, Data, Dock,
    DownloadEnd, DownloadJob, EraseStop, Field, FileKind, FileOp, FileTask, Fold, Folder, Frame, FsAction, FsChoice, FsRow, Have, HeroArt, Hit,
    Install, JobEnd, Keys, Pick, Place, Rows, RomCheck, Screen, SkywardCheck, SkywardEnd, SkywardJob, SkywardStage, Slider, Source, State,
    Switch, SystemsWork, Target, Medium, UsbLink, Wipe, ADDABLE, ADB_WIFI_PORT, BACKUP_PARTS, DATA_ERASED, DATA_PARTITION,
    FASTBOOT_CHOICES, SCREEN_OFF, SD_CARD_UNREADABLE, SD_CARD_WANTED, SD_CARD_WANTED_DETAIL, TARGETS, TIME_ZONES,
    USB_DRIVE_UNREADABLE, USB_DRIVE_WANTED, WIPES, installs, starting,
};
use crate::canvas::Canvas;
use crate::color::Rgba;
use crate::editor;
use crate::files;
use crate::fmt::{self, Verb};
use crate::font::Fonts;
use crate::geom::Rect;
use crate::lang::{Lang, Text};
use crate::search;
use crate::strings::*;
use crate::term::{self, Term};
use crate::theme::Theme;
use crate::icons::{self, Icon};
use crate::widgets::{self, Caption, Detail, Fact, Outline, Row, Spec, SystemCard, SystemMark, Tone, Trail};
use skyward_info::{DeviceInfo, BUILD, VERSION};
use skyward_install::{Outcome, Reflash, Refusal, Stage};
use skyward_net::download::Progress;
use skyward_net::supplicant::Security;
use skyward_net::wlan::{Bands, Network, Power, Stage as Joining};
use skyward_systems::Who;
use skyward_vab::{Slot, Standing};
use std::path::Path;

// The colours the stock Settings fills its icons' tiles with, the same in
// light and dark mode.
const BLUE: Rgba = Rgba::rgb(0x34, 0x82, 0xff);
const GREEN: Rgba = Rgba::rgb(0x1d, 0xcd, 0x3a);
const PURPLE: Rgba = Rgba::rgb(0x77, 0x67, 0xf9);
const SLATE: Rgba = Rgba::rgb(0x8c, 0x9d, 0xb0);
const ORANGE: Rgba = Rgba::rgb(0xff, 0x69, 0x00);
const RED: Rgba = Rgba::rgb(0xfa, 0x38, 0x2e);
// The stock Settings' own tile for removable storage.
const TEAL: Rgba = Rgba::rgb(0x00, 0xa3, 0x9c);

/// What every page draws from: style, typefaces, the device readings, and
/// what the pages hold, from the choices made on them to where the data
/// partition stands.
pub struct Ctx<'a> {
    pub t: &'a Theme,
    pub f: &'a Fonts,
    pub info: &'a DeviceInfo,
    pub state: &'a State,
    /// The shell the terminal page shows, once one has started.
    pub term: Option<&'a Term>,
    /// The part of the page to draw; all of it if none is given. The page
    /// is laid out whole either way.
    pub band: Option<Band>,
}

/// Rows `rows` of a page, and of them the columns `cols`, each from the
/// first to one past the last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Band {
    pub rows: (u32, u32),
    pub cols: (u32, u32),
}

pub fn render(screen: Screen, cx: &Ctx) -> Frame {
    match screen {
        Screen::Decrypt => decrypt(cx),
        Screen::Home => home(cx),
        Screen::Settings => settings(cx),
        Screen::Reboot => reboot(cx),
        Screen::Wipe => wipe(cx),
        Screen::Mount => mount(cx),
        Screen::Install => install(cx),
        Screen::Choose(place) => choose(cx, place),
        Screen::PackageImages => package_images(cx),
        Screen::Installing => installing(cx),
        Screen::About => about(cx),
        Screen::Specs => specs(cx),
        Screen::ScreenOff => screen_off(cx),
        Screen::Search => search(cx),
        Screen::Backup => backup(cx),
        Screen::Restore => restore(cx),
        Screen::RestoreBackup => restore_backup(cx),
        Screen::BackupJob => backup_job(cx),
        Screen::Advanced => advanced(cx),
        Screen::Files => files(cx),
        Screen::FileInfo => file_info(cx),
        Screen::Permissions => permissions(cx),
        Screen::Editor => editor(cx),
        Screen::Terminal => terminal(cx),
        Screen::Relabel => relabel(cx),
        Screen::Filesystems => filesystems(cx),
        Screen::Filesystem => filesystem(cx),
        Screen::PartWork => part_work(cx),
        Screen::TimeZone => time_zone(cx),
        Screen::Logs => logs(cx),
        Screen::Update => update(cx),
        Screen::SkywardUpdate => skyward_update(cx),
        Screen::License => license(cx),
        Screen::FontLicense => font_license(cx),
        Screen::Slot => slot(cx),
        Screen::Systems => systems(cx),
        Screen::Fastboot => fastboot(cx),
        Screen::Erase => erase(cx),
        Screen::CantStart => cant_start(cx),
        Screen::Unhandled => unhandled(cx),
        Screen::Rename => rename(cx),
        Screen::Clash => clash(cx),
        Screen::FileJob => file_job(cx),
        Screen::Wlan => wlan(cx),
        Screen::WlanPassword => wlan_password(cx),
        Screen::WlanNetwork => wlan_network(cx),
        Screen::WlanAdd => wlan_add(cx),
        #[cfg(feature = "roots")]
        Screen::Root => root(cx),
        #[cfg(feature = "roots")]
        Screen::Apatch => apatch(cx),
        #[cfg(feature = "roots")]
        Screen::ApatchJob => apatch_job(cx),
        #[cfg(feature = "roots")]
        Screen::Ksu => ksu(cx),
        #[cfg(feature = "roots")]
        Screen::KsuJob => ksu_job(cx),
        #[cfg(feature = "roots")]
        Screen::KsuManager => ksu_manager(cx),
        #[cfg(feature = "roots")]
        Screen::Kernel => kernel(cx),
        #[cfg(feature = "roots")]
        Screen::KernelJob => kernel_job(cx),
        #[cfg(feature = "roots")]
        Screen::ApatchKey => apatch_key(cx),
    }
}

/// Label and colour of each swipe-to-confirm control: the filesystem
/// page's as `s` says what it is to do.
pub fn confirm_label(which: Confirm, s: &State) -> (&'static str, bool) {
    const INSTALL: Text = S_SLIDE_TO_INSTALL;
    let (label, danger) = match which {
        Confirm::Filesystem => match s.fs_action {
            FsAction::Repair => (S_SLIDE_TO_REPAIR, true),
            FsAction::Remake(_) => (S_SLIDE_TO_CHANGE, true),
        },
        Confirm::Reboot => (S_SLIDE_TO_REBOOT, false),
        Confirm::Wipe => (S_SLIDE_TO_WIPE, true),
        Confirm::Flash => (S_SLIDE_TO_FLASH, true),
        Confirm::Backup => (S_SLIDE_TO_BACK_UP, false),
        Confirm::Restore => (S_SLIDE_TO_RESTORE, true),
        Confirm::Slot => (S_SLIDE_TO_SWITCH, true),
        Confirm::FactoryReset => (S_SLIDE_TO_FACTORY_RESET, true),
        Confirm::DeleteEntry | Confirm::DeleteBackup => (S_SLIDE_TO_DELETE, true),
        Confirm::Install => (INSTALL, false),
        Confirm::InstallWipe => (S_SLIDE_TO_INSTALL_AND_WIPE, true),
        Confirm::InstallOlder => (S_SLIDE_TO_INSTALL_IT_ANYWAY, true),
        Confirm::RunScript => (INSTALL, true),
        Confirm::Merge => (S_SLIDE_TO_MERGE, true),
        Confirm::Relabel => (S_SLIDE_TO_FIX, true),
        Confirm::SwitchSystem => (S_SLIDE_TO_SWITCH_AND_REBOOT, false),
        Confirm::RemoveSystem => (S_SLIDE_TO_REMOVE, true),
        Confirm::InstallSystem => (INSTALL, false),
        #[cfg(feature = "roots")]
        Confirm::ApatchPatch => match s.apatch.as_ref().is_some_and(|root| root.patched) {
            true => (S_SLIDE_TO_PATCH_IT_AGAIN, true),
            false => (S_SLIDE_TO_INSTALL_KERNELPATCH, true),
        },
        #[cfg(feature = "roots")]
        Confirm::ApatchRepatch => (S_SLIDE_TO_PATCH_IT_AGAIN, true),
        #[cfg(feature = "roots")]
        Confirm::KsuPatch => (S_SLIDE_TO_PATCH_THE_BOOT_IMAGE, true),
        #[cfg(feature = "roots")]
        Confirm::KernelInstall => (S_SLIDE_TO_INSTALL_THE_KERNEL, true),
        #[cfg(feature = "roots")]
        Confirm::ApatchDone | Confirm::KsuDone | Confirm::KernelDone => {
            (S_SLIDE_WHEN_DONE, false)
        }
    };
    (label, danger)
}

/// The home page's large title, and what its search bar and the search
/// page's field say while nothing is typed.
pub const HOME_TITLE: &str = "Skyward";
pub const SEARCH_HINT: Text = S_SEARCH_FEATURES_AND_PARTITIONS;

const REBOOT_TARGETS: [(Text, Option<Text>); 6] = [
    (S_SYSTEM, None),
    (S_RECOVERY, None),
    ("Fastboot", Some(S_USERSPACE_FASTBOOTD)),
    ("Bootloader", None),
    ("EDL", Some(S_QUALCOMM_EMERGENCY_DOWNLOAD)),
    (S_POWER_OFF, None),
];

/// What pages are called both on the rows that open them and in their own
/// titles.
const WLAN: Text = "Wi-Fi";
const DECRYPT: Text = S_DECRYPT;
const ADVANCED: Text = S_ADVANCED;
const SETTINGS: Text = S_SETTINGS;
const REBOOT: Text = S_REBOOT;
const ABOUT: Text = S_ABOUT_PHONE;
const SPECS: Text = S_ALL_SPECS;
const LOGS: Text = S_EXPORT_LOGS;
const SCREEN_TIMEOUT: Text = S_SCREEN_TIMEOUT;
const SYSTEM_UPDATE: Text = S_SYSTEM_UPDATE_2;
const UPDATE: Text = S_UPDATE;
const SKYWARD_UPDATE: Text = S_SKYWARD_UPDATE;
const FILES: Text = S_FILE_MANAGER;
const TERMINAL: Text = S_TERMINAL;
const TIME_ZONE: Text = S_TIME_ZONE;
const LICENSE: Text = S_OPEN_SOURCE_LICENSES;
const FONTS: Text = S_UI_FONTS;
const DUAL_BOOT: Text = S_DUAL_BOOT;
const CHECKING: Text = S_CHECKING_FOR_UPDATES;
const CHECK_FOR_UPDATES: Text = S_CHECK_FOR_UPDATES;
const DOWNLOADING: Text = S_DOWNLOADING;
const CHECKING_PART: Text = S_VERIFYING_WHAT_WAS_DOWNLOADED;

/// A canvas for a page `h` tall in the colour `bg`, holding the rows `cx`
/// asks to be drawn.
fn sheet(cx: &Ctx, h: u32, bg: Rgba) -> Canvas {
    let w = cx.t.m.w;
    let Band { rows, cols } = cx.band.unwrap_or(Band { rows: (0, h), cols: (0, w) });
    Canvas::band(w, h, rows.0..rows.1, cols.0..cols.1, bg)
}

fn page(cx: &Ctx) -> Canvas {
    sheet(cx, cx.t.m.h, cx.t.pal.bg)
}

/// A canvas tall enough for a scrolling page; [`fit`] trims it to the content.
fn long_page(cx: &Ctx) -> Canvas {
    sheet(cx, cx.t.m.h * 3, cx.t.pal.bg)
}

/// How tall a long page whose content ends at `bottom` is: to the bottom
/// margin after it, and never shorter than one screen.
fn fitted(t: &Theme, bottom: f32) -> u32 {
    ((bottom + t.m.margin * 2.0).ceil() as u32).max(t.m.h)
}

/// Trim a long page to its content plus the bottom margin, and never shorter
/// than one screen.
fn fit(c: Canvas, t: &Theme, bottom: f32) -> Canvas {
    c.crop_height(fitted(t, bottom))
}

fn hits(rects: &[Rect], actions: &[Action]) -> Vec<Hit> {
    rects.iter().zip(actions).map(|(&rect, &action)| Hit { rect, action }).collect()
}

/// Recovery home, laid out like the Settings root: title, search, a device
/// card, then grouped entries. The status bar carries the readings: the
/// slot, the data partition's lock and the battery's temperature.
pub fn home(cx: &Ctx) -> Frame {
    let (t, f, i, l) = (cx.t, cx.f, cx.info, cx.state.lang);
    let mut c = long_page(cx);
    let mut out = Vec::new();
    // The search bar is left room on the page and drawn over it, folding
    // away as the page scrolls up.
    let bar = Rect::new(t.m.margin, t.m.content_top, t.m.w as f32 - t.m.margin * 2.0, t.m.search_h);
    let mut y = bar.y + bar.h + t.m.after_search;
    let cards_top = y;
    let fold = Fold { bar, hint: SEARCH_HINT, span: cards_top - bar.y };
    out.push(Hit { rect: bar, action: Action::Open(Screen::Search) });

    let sub = match &i.os_version {
        Some(v) => format!("{} · {v}", i.codename),
        None => i.codename.clone(),
    };
    // Under the phone's name the card holds only what asks for something to
    // be done: a battery too low to install on, an update waiting to merge,
    // and a restart into the slot the system does not run from now.
    let low = match i.battery.as_ref().and_then(skyward_install::battery_refusal) {
        Some(Refusal::Battery { level, charging }) => Some(fmt::battery_low(l, level, charging)),
        _ => None,
    };
    let next = match (i.slot, &cx.state.boot_slots) {
        (Some(now), Some(Ok(slots))) if slots.active != now => Some(fmt::next_boot(l, slots.active, now)),
        _ => None,
    };
    let mut rows = Vec::new();
    let mut opens = Vec::new();
    if let Some((title, note)) = &low {
        rows.push(Row::new(title).icon(RED, Icon::Battery).subtitle(note).trail(Trail::None));
        opens.push(None);
    }
    if let Some(note) = fmt::snapshot_note(l, &i.snapshot) {
        rows.push(Row::new(fmt::snapshot(l, &i.snapshot)).icon(ORANGE, Icon::Layers).subtitle(note).trail(Trail::Arrow));
        opens.push(Some(Action::Open(Screen::Install)));
    }
    if let Some((title, note)) = &next {
        rows.push(Row::new(title).icon(ORANGE, Icon::Power).subtitle(note).trail(Trail::Arrow));
        opens.push(Some(Action::Open(Screen::Slot)));
    }
    if cx.state.system_notice.is_some() {
        let why = S_TAP_TO_SEE_WHY;
        rows.push(Row::new(SECOND_FAILED).icon(RED, Icon::Systems).subtitle(why).trail(Trail::Arrow));
        opens.push(Some(Action::Open(Screen::Systems)));
    }
    let card = widgets::device_card(&mut c, t, f, y, &i.model, &sub, &rows);
    out.push(Hit { rect: card.rows[0], action: Action::Open(Screen::About) });
    out.extend(card.rows[1..].iter().zip(&opens).filter_map(|(&rect, open)| open.map(|action| Hit { rect, action })));
    y = card.bottom + t.m.card_gap;
    // On a phone whose WLAN the recovery brings up, the WLAN comes first
    // under the device, as on the stock settings home; with it, where the
    // phone can hold a second system, the two systems.
    let mut rows = Vec::new();
    let mut opens = Vec::new();
    if let Some(w) = &cx.state.wlan {
        rows.push(Row::new(WLAN).icon(BLUE, Icon::Wlan).trail(Trail::Value(fmt::wlan_value(l, w))));
        opens.push(Action::Open(Screen::Wlan));
    }
    let starts = systems_value(cx);
    if cx.state.offers(Screen::Systems) {
        let row = Row::new(DUAL_BOOT).icon(PURPLE, Icon::Systems);
        rows.push(match &starts {
            Some(value) => row.trail(Trail::Value(value)),
            None => row,
        });
        opens.push(Action::Open(Screen::Systems));
    }
    if !rows.is_empty() {
        let first = widgets::list(&mut c, t, f, y, &rows);
        out.extend(hits(&first.rows, &opens));
        y = first.bottom + t.m.card_gap;
    }
    let tools = widgets::list(&mut c, t, f, y, &[
        Row::new(S_INSTALL_2).icon(GREEN, Icon::Install),
        Row::new(S_BACKUP).icon(BLUE, Icon::Backup),
        Row::new(S_RESTORE).icon(PURPLE, Icon::Restore),
        Row::new(S_MOUNT).icon(SLATE, Icon::Mount),
        Row::new(DECRYPT).icon(BLUE, Icon::Lock).trail(Trail::Value(fmt::data(l, &cx.state.data))),
    ]);
    out.extend(hits(&tools.rows, &[
        Action::Open(Screen::Install),
        Action::Open(Screen::Backup),
        Action::Open(Screen::Restore),
        Action::Open(Screen::Mount),
        Action::Open(Screen::Decrypt),
    ]));
    y = tools.bottom + t.m.card_gap;
    #[cfg(feature = "roots")]
    {
        let root = widgets::list(&mut c, t, f, y, &[Row::new(ROOT).icon(GREEN, Icon::Shield)]);
        out.extend(hits(&root.rows, &[Action::Open(Screen::Root)]));
        y = root.bottom + t.m.card_gap;
    }
    let target = REBOOT_TARGETS[cx.state.reboot].0;
    let system = widgets::list(&mut c, t, f, y, &[
        Row::new(S_WIPE_DATA).icon(RED, Icon::Wipe),
        Row::new(ADVANCED).icon(SLATE, Icon::More),
        Row::new(SETTINGS).icon(SLATE, Icon::Gear),
        Row::new(REBOOT).icon(ORANGE, Icon::Power).trail(Trail::Value(target)),
    ]);
    out.extend(hits(&system.rows, &[
        Action::Open(Screen::Wipe),
        Action::Open(Screen::Advanced),
        Action::Open(Screen::Settings),
        Action::Open(Screen::Reboot),
    ]));
    // However little it holds, the page scrolls far enough to fold its bar
    // away.
    let height = fitted(t, system.bottom).max(t.m.h + fold.span.ceil() as u32);
    Frame { back: false, fold: Some(fold), ..Frame::new(t, c.crop_height(height), out, HOME_TITLE, cards_top) }
}

/// What the home page says beside its entry to the systems: the one that
/// starts, where a second system is installed, once they are read.
fn systems_value(cx: &Ctx) -> Option<String> {
    match &cx.state.systems {
        Some(Ok(systems)) => Some(match (&systems.other, starting(systems)) {
            (None, _) => S_NO_SECOND_SYSTEM.into(),
            (Some(_), Who::Installed) => systems.installed.name.clone(),
            (Some(other), Who::Other) => other.name.clone(),
        }),
        _ => None,
    }
}

/// What the advanced page says beside its recovery update entry: a download
/// under way, a newer recovery found, or the build this one is.
fn update_value(cx: &Ctx) -> String {
    let (s, l) = (cx.state, cx.state.lang);
    if s.rom_downloading() {
        return S_DOWNLOADING.to_string();
    }
    if s.rom_newest().is_some() {
        return S_NEW_VERSION.to_string();
    }
    fmt::build_date(l, BUILD)
}

/// Preferences, opening with the stock「Display & brightness」theme previews.
pub fn settings(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let themes = widgets::theme_picker(&mut c, t, f, l, top + t.m.dp(8.9));
    out.extend(hits(&themes.rows, &[Action::Theme { dark: false }, Action::Theme { dark: true }]));
    let mut y = widgets::section(&mut c, t, f, themes.bottom, S_BRIGHTNESS);
    let off = fmt::screen_off(l, SCREEN_OFF[s.screen_off]);
    let below = Row::new(SCREEN_TIMEOUT).trail(Trail::Value(&off));
    let slider = widgets::slider_card(&mut c, t, f, y, Caption::Sun, S_SCREEN_BRIGHTNESS, s.brightness, Some(below));
    out.push(Hit { rect: slider.track, action: Action::Slider(Slider::Brightness) });
    if let Some(r) = slider.below {
        out.push(Hit { rect: r, action: Action::Open(Screen::ScreenOff) });
    }
    y = widgets::section(&mut c, t, f, slider.bottom, S_VIBRATION);
    let below = Row::new(S_TOUCH_VIBRATION).trail(Trail::Switch(s.vibrate));
    let vibration = widgets::slider_card(&mut c, t, f, y, Caption::Vibrate, S_VIBRATION_INTENSITY, s.vibration, Some(below));
    out.push(Hit { rect: vibration.track, action: Action::Slider(Slider::Vibration) });
    if let Some(r) = vibration.below {
        out.push(Hit { rect: r, action: Action::Toggle(Switch::Vibrate) });
    }
    y = widgets::section(&mut c, t, f, vibration.bottom, S_GENERAL);
    let general = widgets::list(&mut c, t, f, y, &[
        Row::new(TIME_ZONE).icon(ORANGE, Icon::Clock).trail(Trail::Value(TIME_ZONES[s.time_zone].0)),
    ]);
    out.extend(hits(&general.rows, &[Action::Open(Screen::TimeZone)]));
    y = widgets::section(&mut c, t, f, general.bottom, "USB");
    let usb = widgets::list(&mut c, t, f, y, &[Row::new(S_FILE_TRANSFER).icon(BLUE, Icon::Usb).trail(Trail::Switch(s.file_transfer))]);
    out.extend(hits(&usb.rows, &[Action::Toggle(Switch::FileTransfer)]));
    let note = S_WHEN_ON_A_COMPUTER_CAN_MANAGE_THE_FILES_IN_INTERNAL_STORAGE_ONCE_IT_IS_DECRYPTED;
    y = widgets::footnote(&mut c, t, f, usb.bottom, note);

    if s.charging_available {
        y = widgets::section(&mut c, t, f, y, S_BATTERY);
        let at = match (s.respect_charge_limit, s.charge_limit) {
            (false, _) => S_OFF.to_string(),
            (true, Some(limit)) => crate::t!(S_AT_PERCENT, limit = limit),
            (true, None) => S_THE_SYSTEM_KEEPS_NONE.to_string(),
        };
        let row = Row::new(S_RESPECT_ANDROIDS_CHARGING_LIMIT)
            .icon(GREEN, Icon::Battery)
            .subtitle(&at)
            .trail(Trail::Switch(s.respect_charge_limit));
        let charging = widgets::list(&mut c, t, f, y, &[row]);
        out.extend(hits(&charging.rows, &[Action::Toggle(Switch::RespectChargeLimit)]));
        y = widgets::footnote(&mut c, t, f, charging.bottom, S_THE_RECOVERY_STOPS_CHARGING_AT_THE_SYSTEMS_LIMIT_AND_STARTS_AGAIN_A_FEW_PERCENT_BELOW_IT);
    }
    y = widgets::section(&mut c, t, f, y, S_DEBUGGING);
    let debug = widgets::list(&mut c, t, f, y, &[Row::new(TERMINAL).icon(SLATE, Icon::Terminal)]);
    out.extend(hits(&debug.rows, &[Action::Open(Screen::Terminal)]));
    y = widgets::section(&mut c, t, f, debug.bottom, S_ABOUT);
    let mut rows = vec![Row::new(ABOUT).trail(Trail::Value(VERSION))];
    let mut opens = vec![Action::Open(Screen::About)];
    if s.offers(Screen::SkywardUpdate) {
        let row = Row::new(SKYWARD_UPDATE);
        rows.push(match skyward_value(cx) {
            Some(value) => row.trail(Trail::Value(value)),
            None => row,
        });
        opens.push(Action::Open(Screen::SkywardUpdate));
    }
    let about = widgets::list(&mut c, t, f, y, &rows);
    out.extend(hits(&about.rows, &opens));
    Frame::new(t, fit(c, t, about.bottom), out, SETTINGS, top)
}

/// What the settings page says beside its Skyward update entry: an update
/// under way, or a newer release found.
fn skyward_value(cx: &Ctx) -> Option<&'static str> {
    let s = cx.state;
    if s.updating_skyward() {
        return Some(S_UPDATING);
    }
    match (s.skyward_newest(), &s.skyward_job) {
        (_, Some(SkywardJob { outcome: Some(SkywardEnd::Updated(_)), .. })) => Some(S_UPDATED),
        (Some(n), _) if n.newer && n.build.is_some() => Some(S_NEW_VERSION),
        _ => None,
    }
}

/// How long the screen stays on with nothing touched, opened from the
/// settings.
pub fn screen_off(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = page(cx);
    let top = t.m.content_top;
    let labels = SCREEN_OFF.map(|ms| fmt::screen_off(l, ms));
    let rows: Vec<Row> = labels.iter().enumerate().map(|(k, label)| Row::new(label).trail(Trail::Check(s.screen_off == k))).collect();
    let times = widgets::list(&mut c, t, f, top, &rows);
    let picks: Vec<Action> = (0..SCREEN_OFF.len()).map(Action::ScreenOff).collect();
    let note = S_AFTER_THE_TIME_CHOSEN_WITH_NOTHING_TOUCHED_THE_SCREEN_DIMS_AND_TURNS_OFF_TOUCH_IT_OR_PRESS_ANY_KEY_TO_LIGHT_IT_AGAIN;
    let y = widgets::footnote(&mut c, t, f, times.bottom, note);
    if SCREEN_OFF[s.screen_off].is_none() {
        let note = S_THE_SCREEN_STAYS_ON_SHOWING_THE_SAME_IMAGE_FOR_LONG_MAY_LEAVE_IT_BURNT_IN;
        widgets::footnote(&mut c, t, f, y, note);
    }
    Frame::new(t, c, hits(&times.rows, &picks), SCREEN_TIMEOUT, top)
}

/// Mount manager: the slot's partitions, each with a switch to mount it
/// read-only or unmount it; one that programs are using says which, and
/// stays.
pub fn mount(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;

    let y = widgets::section_first(&mut c, t, f, top, S_PARTITIONS);
    let bottom = match &s.mounts {
        Err(why) => {
            let y = widgets::list(&mut c, t, f, y, &[Row::new(S_COULDNT_READ_THE_PARTITIONS).trail(Trail::None)]).bottom;
            widgets::footnote(&mut c, t, f, y, why)
        }
        Ok(mounts) if mounts.is_empty() => {
            widgets::list(&mut c, t, f, y, &[Row::new(S_NO_PARTITIONS_TO_MOUNT).trail(Trail::None)]).bottom
        }
        Ok(mounts) => {
            let subs: Vec<String> = mounts
                .iter()
                .map(|m| {
                    let (point, holders) = (&m.point, m.holders.join(S_TEXT_4));
                    match m.holders.is_empty() {
                        true => point.clone(),
                        false => crate::t!(S_IN_USE_BY, point = point, holders = holders),
                    }
                })
                .collect();
            let rows: Vec<Row> = mounts
                .iter()
                .zip(&subs)
                .map(|(m, sub)| {
                    let trail = if m.holders.is_empty() { Trail::Switch(m.mounted) } else { Trail::Text(S_IN_USE) };
                    Row::new(&m.base).subtitle(sub).trail(trail)
                })
                .collect();
            let list = widgets::list(&mut c, t, f, y, &rows);
            out.extend(list.rows.iter().zip(mounts).enumerate().map(|(i, (&rect, m))| {
                let action = match m.holders.is_empty() {
                    true => Action::MountToggle(i),
                    false => Action::Unavailable(S_A_PARTITION_IN_USE_CANT_BE_UNMOUNTED),
                };
                Hit { rect, action }
            }));
            let y = match &s.mount_failure {
                Some(failure) => widgets::footnote(&mut c, t, f, list.bottom, &fmt::mount_failure(l, failure)),
                None => list.bottom,
            };
            let about = match cx.info.slot.map(|s| s.letter()) {
                Some(slot) => crate::t!(S_ONLY_THE_CURRENT_SLOT_S_PARTITIONS_IN_SUPER_SHOW_HERE_ALL_MOUNTED_READ_ONLY, slot = slot),
                None => S_ONLY_PARTITIONS_IN_SUPER_SHOW_HERE_ALL_MOUNTED_READ_ONLY
                    .into(),
            };
            widgets::footnote(&mut c, t, f, y, &about)
        }
    };

    Frame::new(t, fit(c, t, bottom), out, S_MOUNT, top)
}

/// Who made the recovery, as the About page names them under its version.
const AUTHOR: &str = "© 2026 Kirk Lin";

/// Whose fork of it this build is, said under the author: a name of its own,
/// so it is written the same in every language.
const FORK: &str = "Fork by @iljasrb";

/// About this device, modelled on the stock「My device」: the wordmark, the
/// version and its author over a moving colour field, and the summary,
/// hardware and link cards scrolling over it. Only the cards are drawn
/// here; the canvas is clear wherever the field shows.
pub fn about(cx: &Ctx) -> Frame {
    let (t, f) = (cx.t, cx.f);
    let m = &t.m;
    // The page is drawn whole, under the field: laid out first to learn
    // how tall it comes to, then drawn that tall.
    let (_, _, bottom) = about_cards(cx, &mut Canvas::band(m.w, m.h * 3, 0..0, 0..0, Rgba { r: 0, g: 0, b: 0, a: 0 }));
    let mut c = Canvas::new(m.w, fitted(t, bottom));
    let (out, summary_top, bottom) = about_cards(cx, &mut c);
    let canvas = fit(c, t, bottom);
    let art = HeroArt {
        wordmark: widgets::wordmark(t, f, "Skyward", m.dp(233.7), m.dp(37.8)),
        version: widgets::version_line(t, f, VERSION),
        author: widgets::author_line(t, f, AUTHOR),
        fork: widgets::fork_line(t, f, FORK),
        runs: canvas.runs(),
    };
    Frame { header: false, hero: Some(art), ..Frame::new(t, canvas, out, ABOUT, summary_top) }
}

const MODEL: Text = S_MODEL;
const PROCESSOR: Text = S_PROCESSOR;
const RAM: Text = S_RAM;
const BATTERY_CAPACITY: Text = S_BATTERY_CAPACITY;
const SCREEN_SIZE: Text = S_SCREEN_SIZE;
const RESOLUTION: Text = S_RESOLUTION;
const OS_VERSION: Text = S_OS_VERSION;
const ANDROID_VERSION: Text = S_ANDROID_VERSION;
const CURRENT_SLOT: Text = S_CURRENT_SLOT;
const DATA_PART: Text = S_DATA_PARTITION;

/// The phone's processor by its marketing name in `l`, from the phone's
/// settings, or else by its cores.
fn soc(_l: Lang, i: &DeviceInfo) -> Option<&str> {
    i.soc.as_deref()
}

/// The About page's cards on `c`: what touching them does, where the
/// first starts, and where the last ends.
fn about_cards(cx: &Ctx, c: &mut Canvas) -> (Vec<Hit>, f32, f32) {
    let (t, f, i, l) = (cx.t, cx.f, cx.info, cx.state.lang);
    let m = &t.m;
    let tone = Tone::hero(t);
    let mut out = Vec::new();

    let storage = fmt::storage_used(&i.storage).unwrap_or_else(|| fmt::capacity(i.storage.total));
    let mut facts = vec![
        Fact { title: MODEL, value: i.model.clone(), link: false },
        Fact { title: S_STORAGE, value: storage, link: true },
    ];
    if let Some(v) = &i.os_version {
        facts.push(Fact { title: OS_VERSION, value: v.clone(), link: false });
    }
    if let Some(r) = &i.android {
        facts.push(Fact { title: ANDROID_VERSION, value: r.clone(), link: false });
    }
    let summary = widgets::facts(c, t, f, m.dp(490.7), &facts, &tone);
    out.push(Hit { rect: summary.rows[1], action: Action::Open(Screen::Specs) });

    let processor = soc(l, i).map_or_else(|| fmt::cpu(l, &i.cpu), str::to_string);
    let mut specs = vec![(PROCESSOR, processor), (RAM, fmt::ram(i.ram_kib))];
    if let Some(design) = i.battery.as_ref().and_then(|b| b.design) {
        specs.push((BATTERY_CAPACITY, fmt::mah(design)));
    }
    if let Some(d) = i.screen {
        specs.push((SCREEN_SIZE, fmt::inches(d)));
    }
    if let Some(p) = i.panel {
        specs.push((RESOLUTION, fmt::panel(p)));
    }
    let y = widgets::spec_card(c, t, f, summary.bottom + m.card_gap, &fmt::model_lines(&i.model), &specs, &tone);
    let links = widgets::list_toned(c, t, f, y + m.card_gap, &[Row::new(SPECS), Row::new(LOGS)], &tone);
    out.extend(hits(&links.rows, &[Action::Open(Screen::Specs), Action::Open(Screen::Logs)]));
    (out, summary.rows[0].y, links.bottom)
}

/// Every reading the recovery has, modelled on the stock「All specs」.
pub fn specs(cx: &Ctx) -> Frame {
    let (t, f, i, l) = (cx.t, cx.f, cx.info, cx.state.lang);
    let gap = t.m.card_gap;
    let mut c = long_page(cx);
    let mut out = Vec::new();
    // This page leaves more room under the title than the settings pages.
    let top = t.m.dp(188.0);

    let mut grid = vec![Spec { icon: Outline::Phone, label: MODEL, value: fmt::model_lines(&i.model) }];
    if let Some(design) = i.battery.as_ref().and_then(|b| b.design) {
        grid.push(Spec { icon: Outline::Battery, label: BATTERY_CAPACITY, value: fmt::mah(design) });
    }
    let cpu = match soc(l, i) {
        Some(soc) => format!("{soc}\n{}", fmt::cpu(l, &i.cpu)),
        None => fmt::cpu(l, &i.cpu),
    };
    grid.push(Spec { icon: Outline::Chip, label: PROCESSOR, value: cpu });
    grid.push(Spec { icon: Outline::Memory, label: RAM, value: fmt::ram(i.ram_kib) });
    if let Some(p) = i.panel {
        grid.push(Spec { icon: Outline::Resolution, label: RESOLUTION, value: fmt::panel(p) });
    }
    if let Some(d) = i.screen {
        grid.push(Spec { icon: Outline::Screen, label: SCREEN_SIZE, value: fmt::inches(d) });
    }
    grid.push(Spec { icon: Outline::Storage, label: S_STORAGE_CAPACITY, value: fmt::capacity(i.storage.total) });
    grid.push(Spec { icon: Outline::Tag, label: S_CODENAME, value: i.codename.clone() });
    let mut y = widgets::spec_grid(&mut c, t, f, top, &grid);

    if let Some(v) = &i.os_version {
        y = widgets::info_card(&mut c, t, f, y + gap, &[(OS_VERSION, v.clone())]);
    }
    if let Some(r) = &i.android {
        y = widgets::info_card(&mut c, t, f, y + gap, &[(ANDROID_VERSION, fmt::android(l, r, i.security_patch.as_ref()))]);
    }
    y = widgets::info_card(&mut c, t, f, y + gap, &[
        (S_RECOVERY_VERSION, format!("Skyward {VERSION}")),
        (S_KERNEL_VERSION, i.kernel.clone()),
    ]);

    let mut boot = Vec::new();
    if let Some(s) = i.slot {
        boot.push((CURRENT_SLOT, s.letter().to_string()));
        boot.push((SYSTEM_UPDATE, fmt::snapshot(l, &i.snapshot).to_string()));
    }
    if let Some(unlocked) = i.boot.unlocked {
        let state = if unlocked { S_UNLOCKED } else { S_LOCKED };
        boot.push(("Bootloader", state.to_string()));
    }
    if let Some(v) = &i.boot.verified {
        boot.push((S_VERIFIED_BOOT, fmt::verified(l, v)));
    }
    boot.push((DATA_PART, fmt::data(l, &cx.state.data).to_string()));
    y = widgets::info_card(&mut c, t, f, y + gap, &boot);

    if let Some(b) = &i.battery {
        let mut rows = vec![(S_LEVEL, fmt::level(l, b))];
        if let Some(v) = b.temp {
            rows.push((S_TEMPERATURE, fmt::celsius(v)));
        }
        if let Some(v) = fmt::wear(l, b) {
            rows.push((S_HEALTH, v));
        }
        if let Some(v) = b.cycles {
            rows.push((S_CHARGE_CYCLES, crate::t!(S_TEXT_5, v = v)));
        }
        if let Some(v) = b.voltage {
            rows.push((S_VOLTAGE, fmt::volts(v)));
        }
        y = widgets::info_card(&mut c, t, f, y + gap, &rows);
    }

    let total = fmt::capacity(i.storage.total);
    let storage = match i.storage.free.map(fmt::gb) {
        Some(free) => crate::t!(S_FREE_OF, free = free, total = total),
        None => crate::t!(S_IN_ALL_DATA_PARTITION_NOT_MOUNTED, total = total),
    };
    y = widgets::info_card(&mut c, t, f, y + gap, &[(S_INTERNAL_STORAGE, storage)]);
    let links = widgets::list(&mut c, t, f, y + gap, &[
        Row::new(LICENSE).trail(Trail::Value("GPL-3.0")),
        Row::new(FONTS).trail(Trail::Value("MiSans")),
    ]);
    out.extend(hits(&links.rows, &[Action::Open(Screen::License), Action::Open(Screen::FontLicense)]));
    Frame::new(t, fit(c, t, links.bottom), out, SPECS, top)
}

/// The data partition and its users' own files. The page opens the
/// partition as it shows, which is how it learns each user's screen lock;
/// a user whose files wait on one is asked with the control that lock is
/// answered with, from the start.
pub fn decrypt(cx: &Ctx) -> Frame {
    let (c, _l) = (page(cx), cx.state.lang);
    match &cx.state.data {
        Data::Locked { user, keys, checking } => code(cx, c, *user, *keys, *checking),
        Data::Closed | Data::Opening => data_status(
            cx,
            c,
            Icon::Lock,
            S_DECRYPT_THE_DATA_PARTITION,
            Some(S_READING_THE_SCREEN_LOCK),
            None,
        ),
        Data::Open => data_status(
            cx,
            c,
            Icon::Folder,
            S_THE_DATA_PARTITION_IS_DECRYPTED,
            Some(S_YOU_CAN_SEE_THE_PHONES_FILES_IN_FILE_MANAGER),
            None,
        ),
        Data::Failed(why) => data_status(cx, c, Icon::Lock, S_COULDNT_OPEN_THE_DATA_PARTITION, None, Some(why)),
        Data::Erased => data_status(
            cx,
            c,
            Icon::Wipe,
            DATA_ERASED,
            Some(S_ITS_MADE_ANEW_WHEN_THE_PHONE_NEXT_STARTS),
            None,
        ),
    }
}

/// Where the partition stands, under its sign: a line on it, or why it
/// did not open and a way to try again; then the way off the page.
fn data_status(cx: &Ctx, mut c: Canvas, icon: Icon, title: &str, note: Option<&str>, why: Option<&str>) -> Frame {
    let (t, f, _l) = (cx.t, cx.f, cx.state.lang);
    let m = &t.m;
    let x = m.w as f32 / 2.0;
    let sign_cy = m.dp(150.0);
    c.fill_circle(x, sign_cy, m.dp(30.0), t.pal.card);
    icons::glyph(&mut c, icon, x, sign_cy, m.dp(40.0), t.pal.accent);
    c.text_center(&f.heading, title, x, sign_cy + m.dp(64.0), m.dp(24.0), t.pal.text);
    if let Some(note) = note {
        c.text_center(&f.body, note, x, sign_cy + m.dp(98.0), m.sub_px, t.pal.subtitle);
    }
    let mut out = Vec::new();
    if let Some(why) = why {
        widgets::paragraph(&mut c, t, f, sign_cy + m.dp(88.0), why);
        let again = widgets::button(&mut c, t, f, m.h as f32 * 0.62, S_TRY_AGAIN);
        out.push(Hit { rect: again, action: Action::OpenData });
    }
    let leave_cy = m.h as f32 - m.dp(64.0);
    let leave = if matches!(cx.state.data, Data::Open | Data::Erased) { S_DONE_2 } else { NOT_NOW };
    c.text_center(&f.heading, leave, x, leave_cy, m.row_title_px, t.pal.accent);
    out.push(Hit { rect: Rect::new(x - m.dp(80.0), leave_cy - m.dp(24.0), m.dp(160.0), m.dp(48.0)), action: Action::Skip });
    Frame { header: false, ..Frame::new(t, c, out, DECRYPT, 0.0) }
}

/// What leaves the decrypt page with the data partition left as it is.
const NOT_NOW: Text = S_NOT_NOW;

/// What a row whose work waits on the data partition says under it until
/// that is decrypted.
const DECRYPT_NEEDED: Text = S_DECRYPT_FIRST;

/// A user's own files wait on their screen lock: the control it is
/// answered with, which takes nothing while what was entered is checked.
fn code(cx: &Ctx, mut c: Canvas, user: u32, keys: Keys, checking: bool) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let m = &t.m;
    let x = m.w as f32 / 2.0;
    let mut out = Vec::new();
    let title = match (keys == Keys::Pattern, user) {
        (true, 0) => S_DRAW_THE_SCREEN_LOCK_PATTERN.to_string(),
        (true, user) => crate::t!(S_DRAW_USER_S_SCREEN_LOCK_PATTERN, user = user),
        (false, 0) => S_ENTER_THE_SCREEN_LOCK.to_string(),
        (false, user) => crate::t!(S_ENTER_USER_S_SCREEN_LOCK, user = user),
    };
    let note = if checking { S_CHECKING } else { S_DECRYPT_TO_SEE_THE_PHONES_FILES };
    c.text_center(&f.heading, &title, x, m.dp(130.0), m.dp(24.0), t.pal.text);
    c.text_center(&f.body, note, x, m.dp(166.0), m.sub_px, t.pal.subtitle);
    let mut pattern = None;
    let bottom = match keys {
        Keys::Letters => {
            let top = m.h as f32 - widgets::keyboard_height(t);
            let caps = widgets::keyboard(&mut c, t, f, l, top, s.layer, s.key_held(), DECRYPT);
            if !checking {
                out.extend(caps.into_iter().map(|(rect, cap)| Hit { rect, action: Action::Type(cap) }));
            }
            top
        }
        Keys::Pattern => {
            let span = m.dp(300.0);
            let cy = m.dp(300.0) + span / 2.0;
            widgets::pattern_lock(&mut c, t, x, cy, span, &s.pattern);
            if !checking {
                out.push(Hit { rect: Rect::new(x - span, cy - span, span * 2.0, span * 2.0), action: Action::Pattern });
                pattern = Some((x, cy, span));
            }
            m.h as f32 - m.dp(26.0)
        }
        Keys::Digits => {
            let top = m.h as f32 - widgets::keypad_height(t);
            let caps = widgets::keypad(&mut c, t, f, l, top, s.key_held());
            if !checking {
                out.extend(caps.into_iter().map(|(rect, cap)| Hit { rect, action: Action::Type(cap) }));
            }
            top
        }
    };
    // The dots sit just above whatever types into them; a pattern is its
    // own showing of what has been drawn.
    if keys != Keys::Pattern {
        widgets::code_dots(&mut c, t, x, bottom - m.dp(92.0), s.code.chars().count());
    }
    let skip_cy = bottom - m.dp(34.0);
    c.text_center(&f.heading, NOT_NOW, x, skip_cy, m.row_title_px, t.pal.accent);
    out.push(Hit { rect: Rect::new(x - m.dp(80.0), skip_cy - m.dp(24.0), m.dp(160.0), m.dp(48.0)), action: Action::Skip });
    Frame { header: false, pattern, ..Frame::new(t, c, out, DECRYPT, 0.0) }
}

/// What a slot is shown as where the boot control could not be read.
const UNREADABLE: Text = S_UNREADABLE;

/// Reboot target picker with a swipe-to-confirm control.
pub fn reboot(cx: &Ctx) -> Frame {
    let (t, f, s, _l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let mut y = widgets::section_first(&mut c, t, f, top, S_REBOOT_TO);
    let rows: Vec<Row> = REBOOT_TARGETS
        .iter()
        .enumerate()
        .map(|(k, &(title, sub))| {
            let r = Row::new(title).trail(Trail::Check(s.reboot == k));
            match sub {
                Some(text) => r.subtitle(text),
                None => r,
            }
        })
        .collect();
    let targets = widgets::list(&mut c, t, f, y, &rows);
    let picks: Vec<Action> = (0..TARGETS.len()).map(Action::RebootTarget).collect();
    out.extend(hits(&targets.rows, &picks));
    y = targets.bottom;
    if let Some(slot) = cx.info.slot {
        let next = match &s.boot_slots {
            Some(Ok(slots)) if slots.active == slot => {
                let slot = slot.letter();
                crate::t!(S_CURRENT, slot = slot)
            }
            Some(Ok(slots)) => slots.active.letter().to_string(),
            Some(Err(_)) => UNREADABLE.to_string(),
            None => String::new(),
        };
        y = widgets::section(&mut c, t, f, y, S_SLOT_2);
        let slots = widgets::list(&mut c, t, f, y, &[Row::new(S_NEXT_BOOT).trail(Trail::Value(&next))]);
        out.extend(hits(&slots.rows, &[Action::Open(Screen::Slot)]));
        y = slots.bottom;
    }
    let (label, danger) = confirm_label(Confirm::Reboot, cx.state);
    let track = widgets::confirm(&mut c, t, f, y + t.m.dp(28.0), label, danger, s.confirm);
    out.push(Hit { rect: track, action: Action::Confirm(Confirm::Reboot) });
    Frame::new(t, c, out, REBOOT, top)
}

/// Installing, opened from the home page: the system on the phone now, and
/// where a package can come from.
pub fn install(cx: &Ctx) -> Frame {
    let (t, f, s, i, l) = (cx.t, cx.f, cx.state, cx.info, cx.state.lang);
    let mut c = long_page(cx);
    let top = t.m.content_top;
    let mut out = Vec::new();
    let mut facts = Vec::new();
    if let Some(v) = &i.os_version {
        facts.push((S_CURRENT_SYSTEM, v.clone()));
    }
    if let Some(p) = &i.security_patch {
        facts.push((SECURITY_PATCH, fmt::patch(l, p)));
    }
    if let Some(s) = i.slot {
        let (slot, other) = (s.letter(), s.other().letter());
        let detail = crate::t!(S_AND_A_SYSTEM_UPDATE_INSTALLS_TO_SLOT, slot = slot, other = other);
        facts.push((CURRENT_SLOT, detail));
    }
    let ways = S_WAYS_TO_INSTALL;
    let mut y = if facts.is_empty() {
        top
    } else {
        let y = widgets::section_first(&mut c, t, f, top, S_THIS_PHONE);
        widgets::info_card(&mut c, t, f, y, &facts)
    };
    // The system update the phone was given, while one is in flight: a
    // package installed here leaves one that merges once the new system
    // starts, or from here.
    if s.update != Standing::None {
        let system = S_SYSTEM_UPDATE_2;
        y = if y == top { widgets::section_first(&mut c, t, f, y, system) } else { widgets::section(&mut c, t, f, y, system) };
        if let Standing::Merging(done) = s.update {
            let p = percent(done as f32);
            let detail = crate::t!(S_DONE, p = p);
            y = widgets::progress_card(&mut c, t, f, y, S_MERGING_THE_UPDATE, &detail, Some(done as f32));
            let note = S_PARTITIONS_IN_SUPER_ARENT_FLASHED_UNTIL_IT_MERGES_MERGING_GOES_ON_AFTER_GOING_BACK;
            y = widgets::footnote(&mut c, t, f, y, note);
        } else {
            let next = match &s.boot_slots {
                Some(Ok(slots)) => Some(slots.active),
                _ => None,
            };
            let (title, detail) = fmt::update(l, &s.update, next);
            let color = match s.update {
                Standing::None => SLATE,
                Standing::Merged => GREEN,
                Standing::Failed(_) | Standing::Unknown => RED,
                _ => ORANGE,
            };
            let mut row = Row::new(title).icon(color, Icon::Layers).trail(Trail::None);
            if let Some(detail) = &detail {
                row = row.subtitle(detail);
            }
            y = widgets::list(&mut c, t, f, y, &[row]).bottom;
            if let Some(note) = fmt::snapshot_note(l, &i.snapshot) {
                y = widgets::footnote(&mut c, t, f, y, note);
            }
            if s.update == Standing::Waiting {
                let (label, danger) = confirm_label(Confirm::Merge, cx.state);
                let track = widgets::confirm(&mut c, t, f, y + t.m.dp(28.0), label, danger, s.confirm);
                out.push(Hit { rect: track, action: Action::Confirm(Confirm::Merge) });
                let note = S_ONCE_MERGED_THE_SYSTEM_FROM_BEFORE_THE_UPDATE_CANT_COME_BACK;
                y = widgets::footnote(&mut c, t, f, track.y + track.h, note);
            }
        }
    }
    y = if y == top { widgets::section_first(&mut c, t, f, y, ways) } else { widgets::section(&mut c, t, f, y, ways) };
    let mut rows = Vec::new();
    // Internal storage opens with the data partition, which the decrypt
    // page opens or tells why it does not.
    let data = &cx.state.data;
    let (storage, from_storage) = if data.storage_open() {
        (
            S_CHOOSE_A_PACKAGE_OR_IMAGE_IN_INTERNAL_STORAGE,
            Action::Open(Screen::Choose(Place::Storage)),
        )
    } else if *data == Data::Erased {
        (DATA_ERASED, Action::Open(Screen::Decrypt))
    } else {
        (DECRYPT_NEEDED, Action::Open(Screen::Decrypt))
    };
    // The drive's page waits for one to be plugged in, and says why one
    // that is cannot be read.
    let usb = match &cx.state.usb_drive {
        Some(Medium::Shown) => S_CHOOSE_A_PACKAGE_OR_IMAGE_ON_THE_USB_DRIVE,
        Some(Medium::Unreadable(_)) => USB_DRIVE_UNREADABLE,
        None => USB_DRIVE_WANTED,
    };
    rows.push(Row::new(install_title(l, Source::File(Place::Storage))).icon(BLUE, Icon::Phone).subtitle(storage));
    rows.push(Row::new(install_title(l, Source::File(Place::UsbDrive))).icon(PURPLE, Icon::UsbDrive).subtitle(usb));
    // The card in the tray, on a phone whose config names one.
    if s.sdcard_supported {
        let card = medium_said(&s.sdcard, S_CHOOSE_A_PACKAGE_OR_IMAGE_ON_THE_SD_CARD, SD_CARD_UNREADABLE, SD_CARD_WANTED);
        rows.push(Row::new(install_title(l, Source::File(Place::SdCard))).icon(TEAL, Icon::SdCard).subtitle(card));
    }
    let sideload = S_RUN_ADB_SIDELOAD_ON_THE_COMPUTER;
    rows.push(Row::new(install_title(l, Source::Computer)).icon(GREEN, Icon::Usb).subtitle(sideload));
    // A whole system package taken apart: each image it holds comes out,
    // and is then flashed on its own.
    rows.push(
        Row::new(S_TAKE_IMAGES_OUT_OF_A_SYSTEM_PACKAGE)
            .icon(PURPLE, Icon::Layers)
            .subtitle(S_BOOT_INIT_BOOT_AND_THE_LIKE_THEN_FLASHED),
    );
    let methods = widgets::list(&mut c, t, f, y, &rows);
    let from_usb = Action::Open(Screen::Choose(Place::UsbDrive));
    let from_card = Action::Open(Screen::Choose(Place::SdCard));
    // The rows are the ways in the order they are listed: internal storage,
    // the drive, the card where there is one, sideloading, and the package's
    // images.
    let mut ways = vec![from_storage, from_usb];
    if s.sdcard_supported {
        ways.push(from_card);
    }
    ways.push(Action::Sideload);
    ways.push(Action::OpenPackageImages);
    out.extend(hits(&methods.rows, &ways));
    // A phone that can hold a second system installs one from here too.
    let what = match (i.slot.is_some(), cx.state.dual_boot) {
        (false, _) => S_INSTALLS_FLASHABLE_ZIPS_WITH_AN_INSTALL_SCRIPT_AND_FLASHES_BOOT_INIT_BOOT_VENDOR_BOOT_RECOVERY_DTBO_AND_VBMETA_PARTITION_IMAGES_CHOOSING_A_FILE_LISTS_ONLY_FOLDERS_AND_ZIP_AND_IMG_FILES,
        (true, true) => S_INSTALLS_A_B_FULL_OTA_PACKAGES_FLASHABLE_ZIPS_AND_A_SECOND_SYSTEMS_PACKAGE,
        (true, false) => S_INSTALLS_A_B_FULL_OTA_PACKAGES_AND_FLASHABLE_ZIPS_WITH_AN_INSTALL_SCRIPT_AND_FLASHES_BOOT_INIT_BOOT_VENDOR_BOOT_RECOVERY_DTBO_AND_VBMETA_PARTITION_IMAGES_A_SYSTEM_UPDATE_INSTALLS_TO_THE_OTHER_SLOT_WHICH_STARTS_AFTER_A_REBOOT_CHOOSING_A_FILE_LISTS_ONLY_FOLDERS_AND_ZIP_AND_IMG_FILES,
    };
    let y = widgets::footnote(&mut c, t, f, methods.bottom, what);
    let y = widgets::section(&mut c, t, f, y, S_OPTIONS);
    let check = Row::new(S_VERIFY_ZIP_SIGNATURES).trail(Trail::Switch(cx.state.check_signature));
    let signature = widgets::list(&mut c, t, f, y, &[check]);
    out.extend(hits(&signature.rows, &[Action::Toggle(Switch::Signature)]));
    let note = S_WHEN_ON_ONLY_ZIPS_SIGNED_WITH_THIS_PHONES_OFFICIAL_KEYS_INSTALL;
    let mut y = widgets::footnote(&mut c, t, f, signature.bottom, note);
    if cx.state.recovery_partitions {
        let row = Row::new(S_FLASH_SKYWARD_AGAIN).trail(Trail::Switch(cx.state.keep_recovery));
        let options = widgets::list(&mut c, t, f, y + t.m.card_gap, &[row]);
        out.extend(hits(&options.rows, &[Action::Toggle(Switch::KeepRecovery)]));
        let note = S_WHEN_ON_SKYWARD_GOES_BACK_INTO_THE_RECOVERY_PARTITIONS_A_SYSTEM_UPDATE_OR_FLASHABLE_ZIP_WRITES_OVER;
        y = widgets::footnote(&mut c, t, f, options.bottom, note);
    }
    Frame::new(t, fit(c, t, y), out, S_INSTALL_2, top)
}

/// The images a system package holds: each is named by its partition and
/// size, and picking one takes it out and hands it to the installer.
pub fn package_images(cx: &Ctx) -> Frame {
    let (t, f, s, _l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let title = S_IMAGES_IN_THE_PACKAGE;
    let Some(package) = &s.package else {
        let y = widgets::footnote(&mut c, t, f, top, S_NO_PACKAGE_WAS_CHOSEN);
        return Frame::new(t, fit(c, t, y), out, title, top);
    };
    let mut y = widgets::section_first(&mut c, t, f, top, &package.name);
    if let Some(error) = &package.error {
        y = widgets::footnote(&mut c, t, f, y, error);
        return Frame::new(t, fit(c, t, y), out, title, top);
    }
    if let Some((done, total)) = package.progress {
        let detail = format!("{} / {}", fmt::bytes(done), fmt::bytes(total));
        let heading = match &package.taking {
            Some(name) => crate::t!(S_TAKING_OUT, name = name),
            None => S_TAKING_THE_IMAGE_OUT.to_string(),
        };
        y = widgets::progress_card(&mut c, t, f, y, &heading, &detail, Some(share(done, total)));
    }
    if package.list.is_empty() {
        let y = widgets::footnote(&mut c, t, f, y, S_THE_PACKAGE_HOLDS_NO_IMAGE_TO_TAKE_OUT);
        return Frame::new(t, fit(c, t, y), out, title, top);
    }
    let sizes: Vec<String> = package.list.iter().map(|(_, size)| fmt::bytes(*size)).collect();
    let rows: Vec<Row> = package
        .list
        .iter()
        .enumerate()
        .map(|(at, (name, _))| Row::new(name.as_str()).subtitle(sizes[at].as_str()).trail(Trail::Arrow))
        .collect();
    let list = widgets::list(&mut c, t, f, y, &rows);
    if !package.extracting {
        let picks: Vec<Action> = (0..rows.len()).map(Action::PickPackageImage).collect();
        out.extend(hits(&list.rows, &picks));
    }
    y = list.bottom;
    if !package.left.is_empty() {
        let left: Vec<String> = package.left.iter().map(|(name, steps)| format!("{name}（{steps}）")).collect();
        let said = format!("{}：{}", S_NOT_TAKEN_OUT, left.join(", "));
        y = widgets::footnote(&mut c, t, f, y, &said);
    }
    let note = S_CHOOSE_ONE_IT_IS_TAKEN_OUT_THEN_FLASHED_TO_A_PARTITION_GOING_BACK_STOPS_IT;
    let y = widgets::footnote(&mut c, t, f, y, note);
    Frame::new(t, fit(c, t, y), out, title, top)
}

const SECURITY_PATCH: Text = S_SECURITY_PATCH;

/// How installing from `source` is named: on the install page, and as the
/// title of the page that installs.
fn install_title(_l: Lang, source: Source) -> &'static str {
    match source {
        Source::Computer => S_INSTALL_FROM_A_COMPUTER,
        Source::File(Place::Storage) => S_INSTALL_FROM_INTERNAL_STORAGE,
        Source::File(Place::UsbDrive) => S_INSTALL_FROM_A_USB_DRIVE,
        Source::File(Place::SdCard) => S_INSTALL_FROM_AN_SD_CARD,
        Source::Opened { .. } => S_INSTALL_FROM_FILE_MANAGER,
        Source::Asked { security: false } => S_INSTALL_A_SYSTEM_UPDATE,
        Source::Asked { security: true } => S_INSTALL_A_SECURITY_UPDATE,
        Source::Rescue => S_RESCUE_MODE,
    }
}

/// What a removable place's own line says of it: the medium found, said as
/// `shown`, or why it is not, or what to put in.
fn medium_said(medium: &Option<Medium>, shown: Text, unreadable: Text, wanted: Text) -> Text {
    match medium {
        Some(Medium::Shown) => shown,
        Some(Medium::Unreadable(_)) => unreadable,
        None => wanted,
    }
}

/// What a place is called at the top of its folders.
fn place_name(_l: Lang, place: Place) -> &'static str {
    match place {
        Place::Storage => S_INTERNAL_STORAGE,
        Place::UsbDrive => S_USB_DRIVE,
        Place::SdCard => S_SD_CARD,
    }
}

/// A one-line card on how things stand.
fn standing<'a>(title: &'a str, color: Rgba, icon: Icon, detail: &'a str) -> Row<'a> {
    Row::new(title).icon(color, icon).subtitle(detail).trail(Trail::None)
}

fn share(done: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (done as f64 / total as f64) as f32
    }
}

fn percent(share: f32) -> u32 {
    (share * 100.0).floor() as u32
}

/// Where an update stands, and the share of its step done when known.
fn update_stage(_l: Lang, stage: Option<&Stage>) -> (String, Option<f32>) {
    match stage {
        Some(Stage::Writing { done, total }) => {
            let s = share(*done, *total);
            let p = percent(s);
            (crate::t!(S_STEP_1_OF_3_WRITING_THE_NEW_SYSTEM, p = p), Some(s))
        }
        Some(Stage::Verifying { index, count, partition }) => {
            let n = index + 1;
            let said = crate::t!(S_STEP_2_OF_3_VERIFYING, partition = partition, n = n, count = count);
            (said, Some(*index as f32 / *count as f32))
        }
        Some(Stage::Finishing) => (S_STEP_3_OF_3_FINISHING.into(), None),
        Some(Stage::Reflashing { done, total }) => (REFLASHING.into(), Some(*done as f32 / *total as f32)),
        Some(Stage::Flashing { .. } | Stage::Script { .. } | Stage::Signature { .. } | Stage::Allocating { .. } | Stage::Root { .. } | Stage::Storing) | None => {
            (S_GETTING_READY.into(), None)
        }
    }
}

/// Where installing a system beside the installed one stands, and the share
/// of its step done when known.
fn system_stage(_l: Lang, stage: Option<&Stage>) -> (String, Option<f32>) {
    match stage {
        Some(Stage::Allocating { done, total }) => {
            let s = share(*done, *total);
            let p = percent(s);
            (crate::t!(S_STEP_1_OF_3_SETTING_ROOM_ASIDE_IN_THE_DATA_PARTITION, p = p), Some(s))
        }
        Some(Stage::Root { done, total }) => {
            let s = share(*done, *total);
            let p = percent(s);
            let said = match done * 2 <= *total {
                true => crate::t!(S_STEP_2_OF_3_WRITING_THE_SYSTEM, p = p),
                false => crate::t!(S_STEP_2_OF_3_VERIFYING_THE_SYSTEM, p = p),
            };
            (said, Some(s))
        }
        Some(Stage::Storing) => (S_STEP_3_OF_3_KEEPING_ITS_BOOT_IMAGES.into(), None),
        _ => (S_GETTING_READY.into(), None),
    }
}

const REFLASHING: Text = S_FLASHING_SKYWARD_AGAIN;

/// What the card says while an install script runs, and how far it is: the
/// last line the script showed and how far it says it is, then Skyward
/// going back into the recovery partitions it wrote over.
fn script_stage<'a>(_l: Lang, stage: Option<&Stage>, said: &'a [String]) -> (&'a str, Option<f32>) {
    let running = S_RUNNING_THE_INSTALL_SCRIPT;
    let last = said.iter().rev().map(|line| line.trim()).find(|line| !line.is_empty()).unwrap_or(running);
    match stage {
        Some(Stage::Reflashing { done, total }) => (REFLASHING, Some(*done as f32 / *total as f32)),
        Some(Stage::Script { thousandths }) => (last, Some(f32::from(*thousandths) / 1000.0)),
        _ => (last, None),
    }
}

/// How Skyward went back into the recovery partitions the install wrote
/// over, where it did.
fn reflashed(c: &mut Canvas, cx: &Ctx, y: f32) -> f32 {
    let (t, f, l) = (cx.t, cx.f, cx.state.lang);
    match &cx.state.reflash {
        None => y,
        Some(Reflash::Written(written)) => {
            let slots = fmt::slots(l, written);
            let said = match (l, written.len()) {
                (Lang::En, 1) => format!("Skyward went back into {slots}'s recovery partition."),
                // A phone without slots has one recovery partition, and no
                // slot was named: the recovery's own is where it went.
                (Lang::En, 0) => "Skyward went back into the recovery partition.".to_string(),
                _ => crate::t!(S_SKYWARD_WENT_BACK_INTO_THE_RECOVERY_PARTITIONS_OF, slots = slots),
            };
            widgets::footnote(c, t, f, y, &said)
        }
        Some(Reflash::Failed(why)) => {
            let note = S_COULDNT_FLASH_SKYWARD_BACK_INTO_THE_RECOVERY_PARTITIONS;
            let y = widgets::footnote(c, t, f, y, note);
            widgets::footnote(c, t, f, y, why)
        }
    }
}

/// What the install script showed, where there is any: its heading, and
/// under it the lines as the page's long list, the last thing on the page.
fn script_output(c: &mut Canvas, cx: &Ctx, y: f32, rows: &mut Option<Rows>) -> f32 {
    let lines = &cx.state.script_output;
    if lines.is_empty() {
        return y;
    }
    let y = widgets::section(c, cx.t, cx.f, y, S_INSTALL_SCRIPT_OUTPUT);
    *rows = Some(Rows { edges: widgets::output_edges(c, cx.t, cx.f, y, lines), action: None });
    y
}

fn pick_title(_l: Lang, pick: Pick, current: Option<Slot>) -> String {
    let letter = |s: Option<Slot>| s.map(|s| format!(" {}", s.letter())).unwrap_or_default();
    match pick {
        Pick::Current => format!("{}{}", CURRENT_SLOT, letter(current)),
        Pick::Other => format!("{}{}", S_OTHER_SLOT, letter(current.map(Slot::other))),
        Pick::Both => S_BOTH_SLOTS.into(),
    }
}

/// Words the installing page says of more than one of its steps.
const SWITCHING_USB: Text = S_SWITCHING_USB_MODE;
const READING_FILE: Text = S_READING_THE_FILE;
const WAITING: Text = S_WAITING_FOR_THE_COMPUTER;
const CONNECT: Text = S_CONNECT_A_COMPUTER_WITH_A_CABLE_AND_RUN_THESE_COMMANDS_ON_IT;
const RUN_ON_COMPUTER: Text = S_RUN_ON_THE_COMPUTER;
const KEEP_CABLE: Text = S_KEEP_THE_CABLE_IN_UNTIL_ITS_READ;
const KEEP_DRIVE: Text = S_KEEP_THE_USB_DRIVE_IN_UNTIL_ITS_READ;
const KEEP_CARD: Text = S_KEEP_THE_SD_CARD_IN_UNTIL_ITS_READ;
const GETTING_READY: Text = S_GETTING_READY;

/// Installing a package step by step: receiving what the computer sends
/// with `adb sideload`, or reading the file chosen, then installing it.
pub fn installing(cx: &Ctx) -> Frame {
    let (t, f, s, i, l) = (cx.t, cx.f, cx.state, cx.info, cx.state.lang);
    let mut c = long_page(cx);
    let (mut out, mut rows) = (Vec::new(), None);
    let top = t.m.content_top;
    let computer = s.source == Source::Computer;
    let bottom = match &s.install {
        Install::Idle | Install::Starting | Install::Ending if computer => {
            let row = standing(SWITCHING_USB, GREEN, Icon::Usb, S_FROM_ADB_DEBUGGING_TO_SIDELOAD);
            widgets::list(&mut c, t, f, top, &[row]).bottom
        }
        Install::Idle | Install::Starting | Install::Ending if s.source == Source::Rescue => {
            let row = standing(SWITCHING_USB, GREEN, Icon::Usb, S_FROM_ADB_DEBUGGING_TO_RESCUE_MODE);
            widgets::list(&mut c, t, f, top, &[row]).bottom
        }
        Install::Idle | Install::Starting | Install::Ending => {
            widgets::list(&mut c, t, f, top, &[Row::new(READING_FILE).icon(GREEN, Icon::Install).trail(Trail::None)]).bottom
        }
        Install::Opening => {
            let opening = Row::new(S_OPENING_THE_DATA_PARTITION).icon(GREEN, Icon::Lock).trail(Trail::None);
            let y = widgets::list(&mut c, t, f, top, &[opening]).bottom;
            asked_package(&mut c, cx, y)
        }
        Install::Locked(_) => {
            let row = standing(S_THE_PACKAGE_CANT_BE_READ_YET, ORANGE, Icon::Lock, DECRYPT_NEEDED);
            let y = widgets::list(&mut c, t, f, top, &[row]).bottom;
            let open = widgets::list(&mut c, t, f, y + t.m.card_gap, &[Row::new(DECRYPT)]);
            out.extend(hits(&open.rows, &[Action::Open(Screen::Decrypt)]));
            let y = widgets::footnote(&mut c, t, f, open.bottom, S_IT_INSTALLS_ONCE_THE_DATA_PARTITION_IS_DECRYPTED);
            asked_package(&mut c, cx, y)
        }
        Install::Waiting if s.source == Source::Rescue => {
            let row = standing(WAITING, GREEN, Icon::Usb, CONNECT);
            let y = widgets::list(&mut c, t, f, top, &[row]).bottom;
            let y = widgets::section(&mut c, t, f, y, RUN_ON_COMPUTER);
            let y = widgets::info_card(&mut c, t, f, y, &[
                (
                    S_ADB_RESCUE_INSTALL_FILE,
                    S_WITH_A_SYSTEM_UPDATE_PACKAGE_FLASHABLE_ZIP_OR_PARTITION_IMAGE_AS_FILE
                    .into(),
                ),
                ("adb rescue getprop", S_READS_THIS_PHONES_VERSIONS_AND_DEVICE_DETAILS.into()),
                ("adb rescue wipe userdata", S_WIPES_ALL_OF_THIS_PHONES_DATA.into()),
            ]);
            let note = S_THE_COMPUTER_NEEDS_ANDROID_SDK_PLATFORM_TOOLS_GOING_BACK_LEAVES_RESCUE_MODE_AND_SWITCHES_USB_BACK_TO_ADB_DEBUGGING;
            widgets::footnote(&mut c, t, f, y, note)
        }
        Install::Waiting => {
            let row = standing(WAITING, GREEN, Icon::Usb, CONNECT);
            let y = widgets::list(&mut c, t, f, top, &[row]).bottom;
            let y = widgets::section(&mut c, t, f, y, RUN_ON_COMPUTER);
            let y = widgets::info_card(&mut c, t, f, y, &[(
                S_ADB_SIDELOAD_FILE,
                S_WITH_A_SYSTEM_UPDATE_PACKAGE_FLASHABLE_ZIP_OR_PARTITION_IMAGE_AS_FILE_SUCH_AS_BOOT_IMG
                .into(),
            )]);
            let note = S_THE_COMPUTER_NEEDS_ANDROID_SDK_PLATFORM_TOOLS_GOING_BACK_STOPS_WAITING_AND_SWITCHES_USB_BACK_TO_ADB_DEBUGGING;
            widgets::footnote(&mut c, t, f, y, note)
        }
        Install::Checking { stage: Some(Stage::Signature { done, total }), .. } => {
            let read = share(*done, *total);
            let p = percent(read);
            let detail = crate::t!(S_READ, p = p);
            let y = widgets::progress_card(&mut c, t, f, top, S_VERIFYING_THE_SIGNATURE, &detail, Some(read));
            let note = match s.source {
                Source::Computer => S_KEEP_THE_CABLE_IN_UNTIL_ITS_READ_GOING_BACK_CANCELS_THE_INSTALL_AND_ENDS_ADB_SIDELOAD_ON_THE_COMPUTER,
                Source::Rescue => S_KEEP_THE_CABLE_IN_UNTIL_ITS_READ_GOING_BACK_CANCELS_THE_INSTALL_AND_LEAVES_RESCUE_MODE,
                Source::File(Place::Storage) | Source::Opened { place: Place::Storage } | Source::Asked { .. } => {
                    S_GOING_BACK_CANCELS_THE_INSTALL
                }
                Source::File(Place::UsbDrive) | Source::Opened { place: Place::UsbDrive } => {
                    S_KEEP_THE_USB_DRIVE_IN_UNTIL_ITS_READ_GOING_BACK_CANCELS_THE_INSTALL
                }
                Source::File(Place::SdCard) | Source::Opened { place: Place::SdCard } => {
                    S_KEEP_THE_SD_CARD_IN_UNTIL_ITS_READ_GOING_BACK_CANCELS_THE_INSTALL
                }
            };
            widgets::footnote(&mut c, t, f, y, note)
        }
        Install::Checking { size, .. } => {
            let size = fmt::bytes(*size);
            let detail = crate::t!(S_FILE_SIZE, size = size);
            let y = widgets::list(&mut c, t, f, top, &[standing(READING_FILE, GREEN, Icon::Install, &detail)]).bottom;
            match s.source {
                Source::Computer | Source::Rescue => widgets::footnote(&mut c, t, f, y, KEEP_CABLE),
                Source::File(Place::Storage) | Source::Opened { place: Place::Storage } | Source::Asked { .. } => y,
                Source::File(Place::UsbDrive) | Source::Opened { place: Place::UsbDrive } => widgets::footnote(&mut c, t, f, y, KEEP_DRIVE),
                Source::File(Place::SdCard) | Source::Opened { place: Place::SdCard } => widgets::footnote(&mut c, t, f, y, KEEP_CARD),
            }
        }
        Install::Choosing(choice) => {
            let (name, holds) = fmt::image_kind(l, choice.kind);
            let size = fmt::bytes(choice.size);
            let detail = crate::t!(S_TEXT_6, size = size, holds = holds);
            let y = widgets::list(&mut c, t, f, top, &[standing(name, GREEN, Icon::Install, &detail)]).bottom;
            let y = widgets::section(&mut c, t, f, y, S_PARTITION);
            let rows: Vec<Row> = choice
                .partitions
                .iter()
                .enumerate()
                .map(|(k, p)| Row::new(p).subtitle(fmt::partition_role(l, p)).trail(Trail::Check(k == choice.partition)))
                .collect();
            let parts = widgets::list(&mut c, t, f, y, &rows);
            let choose: Vec<Action> = (0..choice.partitions.len()).map(Action::FlashPartition).collect();
            out.extend(hits(&parts.rows, &choose));
            // A phone without slots has one of each partition, so there is
            // one place an image can go and nothing to choose between.
            let below = match i.slot.is_some() {
                true => {
                    let y = widgets::section(&mut c, t, f, parts.bottom, S_SLOT_2);
                    let titles = Pick::ALL.map(|p| pick_title(l, p, i.slot));
                    let rows: Vec<Row> =
                        Pick::ALL.iter().zip(&titles).map(|(p, title)| Row::new(title).trail(Trail::Check(*p == choice.pick))).collect();
                    let picks = widgets::list(&mut c, t, f, y, &rows);
                    out.extend(hits(&picks.rows, &Pick::ALL.map(Action::FlashSlots)));
                    picks.bottom
                }
                false => parts.bottom,
            };
            let (label, danger) = confirm_label(Confirm::Flash, cx.state);
            let track = widgets::confirm(&mut c, t, f, below + t.m.dp(28.0), label, danger, s.confirm);
            out.push(Hit { rect: track, action: Action::Confirm(Confirm::Flash) });
            let note = match s.source {
                Source::Computer => S_FLASHING_WRITES_OVER_WHAT_THE_PARTITION_HOLDS_GOING_BACK_CANCELS_IT_AND_ENDS_ADB_SIDELOAD_ON_THE_COMPUTER,
                Source::Rescue => S_FLASHING_WRITES_OVER_WHAT_THE_PARTITION_HOLDS_GOING_BACK_CANCELS_IT_AND_LEAVES_RESCUE_MODE,
                Source::File(_) | Source::Opened { .. } | Source::Asked { .. } => S_FLASHING_WRITES_OVER_WHAT_THE_PARTITION_HOLDS_GOING_BACK_CANCELS_IT,
            };
            widgets::footnote(&mut c, t, f, track.y + track.h, note)
        }
        Install::Confirming { version, wipe, downgrade } => {
            let warning = (!downgrade.is_none()).then(|| fmt::downgrade(l, downgrade, s.time_zone));
            let (title, color, icon, detail, which) = if *wipe {
                let reset = factory_reset(s);
                let mut detail = match version {
                    Some(v) => crate::t!(S_INSTALLING_THEN, v = v, reset = reset),
                    None => crate::t!(S_INSTALLING_THIS_UPDATE_THEN, reset = reset),
                };
                if let Some(warning) = &warning {
                    detail.push(' ');
                    detail.push_str(warning);
                }
                (S_ALL_DATA_HAS_TO_BE_WIPED.to_string(), ORANGE, Icon::Wipe, detail, Confirm::InstallWipe)
            } else {
                let title = match version {
                    Some(v) => crate::t!(S_INSTALL, v = v),
                    None => S_INSTALL_THE_SYSTEM_UPDATE.into(),
                };
                let mut detail = match i.slot.map(|s| s.other().letter()) {
                    Some(slot) => crate::t!(S_THE_NEW_SYSTEM_INSTALLS_TO_SLOT_WHICH_STARTS_AFTER_A_REBOOT, slot = slot),
                    None => S_THE_NEW_SYSTEM_STARTS_AFTER_A_REBOOT.into(),
                };
                if let Some(warning) = &warning {
                    detail.push(' ');
                    detail.push_str(warning);
                }
                let (color, which) = match warning.is_some() {
                    true => (ORANGE, Confirm::InstallOlder),
                    false => (GREEN, Confirm::Install),
                };
                (title, color, Icon::Install, detail, which)
            };
            let y = widgets::list(&mut c, t, f, top, &[standing(&title, color, icon, &detail)]).bottom;
            let (label, danger) = confirm_label(which, cx.state);
            let track = widgets::confirm(&mut c, t, f, y + t.m.dp(28.0), label, danger, s.confirm);
            out.push(Hit { rect: track, action: Action::Confirm(which) });
            let note = S_GOING_BACK_CANCELS_THE_INSTALL_WITHOUT_WRITING_ANYTHING;
            widgets::footnote(&mut c, t, f, track.y + track.h, note)
        }
        Install::Updating { version, stage, overwriting } => {
            let title = match version {
                Some(v) => crate::t!(S_INSTALLING, v = v),
                None => S_INSTALLING_THE_SYSTEM_UPDATE.into(),
            };
            let (detail, done) = update_stage(l, stage.as_ref());
            let y = widgets::progress_card(&mut c, t, f, top, &title, &detail, done);
            let until = until_installed(l, s.source);
            let note = match i.slot.map(|s| s.other().letter()) {
                Some(slot) => crate::t!(S_THE_NEW_SYSTEM_INSTALLS_TO_SLOT, until = until, slot = slot),
                None => until.to_string(),
            };
            let y = widgets::footnote(&mut c, t, f, y, &note);
            if *overwriting {
                let note = S_THE_CURRENT_SYSTEM_IS_BEING_WRITTEN_OVER_SO_A_REBOOT_BEFORE_THE_INSTALL_FINISHES_STARTS_NO_SYSTEM;
                widgets::footnote(&mut c, t, f, y, note)
            } else {
                y
            }
        }
        Install::Flashing { partition, stage } => {
            let (detail, done) = match stage {
                Some(Stage::Flashing { done, total }) => {
                    let s = share(*done, *total);
                    let p = percent(s);
                    let detail = match done * 2 < *total {
                        true => crate::t!(S_WRITING, p = p),
                        false => crate::t!(S_VERIFYING, p = p),
                    };
                    (detail, Some(s))
                }
                _ => (GETTING_READY.into(), None),
            };
            let title = crate::t!(S_FLASHING, partition = partition);
            let y = widgets::progress_card(&mut c, t, f, top, &title, &detail, done);
            let keep = match s.source {
                Source::Computer | Source::Rescue => S_KEEP_THE_CABLE_IN_UNTIL_ITS_FLASHED,
                Source::File(Place::Storage) | Source::Opened { place: Place::Storage } | Source::Asked { .. } => {
                    S_DONT_POWER_OFF_UNTIL_ITS_FLASHED
                }
                Source::File(Place::UsbDrive) | Source::Opened { place: Place::UsbDrive } => {
                    S_KEEP_THE_USB_DRIVE_IN_UNTIL_ITS_FLASHED
                }
                Source::File(Place::SdCard) | Source::Opened { place: Place::SdCard } => {
                    S_KEEP_THE_SD_CARD_IN_UNTIL_ITS_FLASHED
                }
            };
            widgets::footnote(&mut c, t, f, y, keep)
        }
        Install::SystemChoosing(choice) => {
            let version = &choice.version;
            let detail = crate::t!(S_TO_INSTALL_AS_THE_SECOND_SYSTEM, version = version);
            let y = widgets::list(&mut c, t, f, top, &[standing(&choice.name, PURPLE, Icon::Systems, &detail)]).bottom;
            let y = widgets::section(&mut c, t, f, y, S_ROOM_FOR_IT);
            let sizes: Vec<String> = choice.sizes.iter().map(|&b| fmt::bytes(b)).collect();
            let least = S_THE_LEAST_THE_SYSTEM_TAKES;
            let rows: Vec<Row> = sizes
                .iter()
                .enumerate()
                .map(|(k, size)| {
                    let row = Row::new(size).trail(Trail::Check(k == choice.size));
                    if k == 0 { row.subtitle(least) } else { row }
                })
                .collect();
            let list = widgets::list(&mut c, t, f, y, &rows);
            let picks: Vec<Action> = (0..choice.sizes.len()).map(Action::SystemSize).collect();
            out.extend(hits(&list.rows, &picks));
            let note = S_THE_ROOM_CHOSEN_IS_SET_ASIDE_IN_THE_DATA_PARTITION_FOR_THE_SECOND_SYSTEMS_FILES_CANT_BE_CHANGED_ONCE_ITS_INSTALLED_AND_COMES_BACK_WHEN_THE_SECOND_SYSTEM_IS_REMOVED_NOTHING_OF_THE_MAIN_SYSTEMS_PARTITIONS_IS_WRITTEN;
            let y = widgets::footnote(&mut c, t, f, list.bottom, note);
            let (label, danger) = confirm_label(Confirm::InstallSystem, cx.state);
            let track = widgets::confirm(&mut c, t, f, y + t.m.dp(24.0), label, danger, s.confirm);
            out.push(Hit { rect: track, action: Action::Confirm(Confirm::InstallSystem) });
            let note = S_GOING_BACK_CANCELS_THE_INSTALL_WITHOUT_WRITING_ANYTHING;
            widgets::footnote(&mut c, t, f, track.y + track.h, note)
        }
        Install::Systeming { name, stage } => {
            let title = crate::t!(S_INSTALLING, name = name);
            let (detail, done) = system_stage(l, stage.as_ref());
            let y = widgets::progress_card(&mut c, t, f, top, &title, &detail, done);
            widgets::footnote(&mut c, t, f, y, until_installed(l, s.source))
        }
        Install::ConfirmingScript => {
            let name = s.package_name.as_deref().unwrap_or(S_FLASHABLE_ZIP);
            let detail = S_THIS_ZIP_RUNS_THE_INSTALL_SCRIPT_IT_HOLDS_WHICH_CAN_REWRITE_ANY_PARTITION_OF_THIS_PHONE;
            let mut y = widgets::list(&mut c, t, f, top, &[standing(name, ORANGE, Icon::Install, detail)]).bottom;
            if let Some(note) = fmt::snapshot_script_note(l, &i.snapshot) {
                y = widgets::footnote(&mut c, t, f, y, note);
            }
            let (label, danger) = confirm_label(Confirm::RunScript, cx.state);
            let track = widgets::confirm(&mut c, t, f, y + t.m.dp(28.0), label, danger, s.confirm);
            out.push(Hit { rect: track, action: Action::Confirm(Confirm::RunScript) });
            let note = S_GOING_BACK_CANCELS_THE_INSTALL_WITHOUT_RUNNING_THE_SCRIPT;
            widgets::footnote(&mut c, t, f, track.y + track.h, note)
        }
        Install::Scripting { stage, .. } => {
            let (detail, done) = script_stage(l, stage.as_ref(), &s.script_output);
            let y = widgets::progress_card(&mut c, t, f, top, S_INSTALLING_2, detail, done);
            let y = widgets::footnote(&mut c, t, f, y, until_installed(l, s.source));
            script_output(&mut c, cx, y, &mut rows)
        }
        Install::Wiping => {
            let y = widgets::progress_card(&mut c, t, f, top, WIPING, &capitalized(l, &factory_reset(s)), None);
            widgets::footnote(&mut c, t, f, y, KEEP_POWER)
        }
        Install::Wiped(Ok(())) => {
            let made_anew = S_THE_DATA_PARTITION_IS_MADE_ANEW_WHEN_THE_PHONE_NEXT_STARTS;
            let y = widgets::list(&mut c, t, f, top, &[standing(S_WIPED, GREEN, Icon::Wipe, made_anew)]).bottom;
            widgets::footnote(&mut c, t, f, y, MORE_COMMANDS)
        }
        Install::Wiped(Err(stop)) => {
            let (what, detail) = fmt::erase_stop(l, stop);
            let mut y = widgets::list(&mut c, t, f, top, &[standing(WIPE_INCOMPLETE, RED, Icon::Wipe, &what)]).bottom;
            if let Some(d) = detail {
                y = widgets::footnote(&mut c, t, f, y, &d);
            }
            widgets::footnote(&mut c, t, f, y, MORE_COMMANDS)
        }
        Install::Finished { outcome, damage } => finished(&mut c, cx, top, outcome, *damage, &mut out, &mut rows),
    };
    // What the system asked for says why, where it said.
    let bottom = match (s.source, &s.asked.reason) {
        (Source::Asked { .. }, Some(reason)) => widgets::info_card(&mut c, t, f, bottom + t.m.dp(12.0), &[(REASON, reason.clone())]),
        _ => bottom,
    };
    Frame { rows, ..Frame::new(t, fit(c, t, bottom), out, install_title(l, s.source), top) }
}

/// What a request the system made gives as why.
const REASON: Text = S_REASON;

const WIPING: Text = S_WIPING;
const WIPE_INCOMPLETE: Text = S_WIPE_INCOMPLETE;
const KEEP_POWER: Text = S_DONT_POWER_OFF_UNTIL_ITS_WIPED;

/// What rescue says once a command is done with: the computer may send
/// another.
const MORE_COMMANDS: Text = S_YOU_CAN_SEND_MORE_COMMANDS_FROM_THE_COMPUTER;

/// The package the system asked for, by the path it gave, at `y`.
fn asked_package(c: &mut Canvas, cx: &Ctx, y: f32) -> f32 {
    let name = cx.state.package_name.clone().unwrap_or_default();
    widgets::info_card(c, cx.t, cx.f, y + cx.t.m.dp(12.0), &[(S_PACKAGE, name)])
}

/// What not to do while a package from `source` is installed.
fn until_installed(_l: Lang, source: Source) -> &'static str {
    match source {
        Source::Computer | Source::Rescue => {
            S_KEEP_THE_CABLE_IN_AND_DONT_POWER_OFF_UNTIL_ITS_INSTALLED
        }
        Source::File(Place::Storage) | Source::Opened { place: Place::Storage } | Source::Asked { .. } => {
            S_DONT_POWER_OFF_UNTIL_ITS_INSTALLED
        }
        Source::File(Place::UsbDrive) | Source::Opened { place: Place::UsbDrive } => {
            S_KEEP_THE_USB_DRIVE_IN_AND_DONT_POWER_OFF_UNTIL_ITS_INSTALLED
        }
        Source::File(Place::SdCard) | Source::Opened { place: Place::SdCard } => {
            S_KEEP_THE_SD_CARD_IN_AND_DONT_POWER_OFF_UNTIL_ITS_INSTALLED
        }
    }
}

/// A phrase that opens a line on its own: in English with its first letter
/// a capital.
fn capitalized(l: Lang, phrase: &str) -> String {
    let mut chars = phrase.chars();
    match (l, chars.next()) {
        (Lang::En, Some(first)) => first.to_uppercase().chain(chars).collect(),
        _ => phrase.to_string(),
    }
}

/// How an install ended, and where to go from there: another package from
/// the computer, or back to choose another file.
fn finished(
    c: &mut Canvas,
    cx: &Ctx,
    top: f32,
    outcome: &Outcome,
    damage: Option<Damage>,
    out: &mut Vec<Hit>,
    rows: &mut Option<Rows>,
) -> f32 {
    let (t, f, source, l) = (cx.t, cx.f, cx.state.source, cx.state.lang);
    let gap = t.m.card_gap;
    let choose_another = S_CHOOSE_ANOTHER_FILE;
    let (again, another) = match source {
        Source::Computer => (Row::new(S_SEND_AGAIN).icon(GREEN, Icon::Usb), Action::Sideload),
        Source::File(_) | Source::Opened { .. } => (Row::new(choose_another).icon(GREEN, Icon::Folder), Action::Back),
        Source::Asked { .. } => (Row::new(choose_another).icon(GREEN, Icon::Folder), Action::Open(Screen::Install)),
        Source::Rescue => (Row::new(S_LEAVE_RESCUE_MODE).icon(GREEN, Icon::Usb), Action::Back),
    };
    let restart = Row::new(S_REBOOT_TO_SYSTEM).icon(ORANGE, Icon::Power);
    let installed = S_INSTALLED;
    let nothing_written = S_NOTHING_WAS_WRITTEN;
    match outcome {
        Outcome::Installed { slot, .. } => {
            let slot = slot.letter();
            let detail = crate::t!(S_THE_NEW_SYSTEM_IS_INSTALLED_TO_SLOT_WHICH_STARTS_AFTER_A_REBOOT, slot = slot);
            let y = widgets::list(c, t, f, top, &[standing(installed, GREEN, Icon::Install, &detail)]).bottom;
            let next = widgets::list(c, t, f, y + gap, &[restart]);
            out.extend(hits(&next.rows, &[Action::RebootTo(Target::System)]));
            let y = reflashed(c, cx, next.bottom);
            afterwards_of(c, cx, y, true)
        }
        Outcome::Flashed { partition, slots } => {
            let slots = fmt::slots(l, slots);
            let detail = crate::t!(S_FLASHED_TO_IN, partition = partition, slots = slots);
            let y = widgets::list(c, t, f, top, &[standing(S_FLASHED, GREEN, Icon::Install, &detail)]).bottom;
            let mut rows = vec![restart];
            let mut acts = vec![Action::RebootTo(Target::System)];
            if *partition == "recovery" {
                rows.push(Row::new(S_REBOOT_TO_RECOVERY).icon(ORANGE, Icon::Power));
                acts.push(Action::RebootTo(Target::Recovery));
            }
            rows.push(again);
            acts.push(another);
            let next = widgets::list(c, t, f, y + gap, &rows);
            out.extend(hits(&next.rows, &acts));
            afterwards_of(c, cx, next.bottom, true)
        }
        Outcome::Ran { .. } => {
            let ran = S_THE_INSTALL_SCRIPT_RAN_TO_ITS_END;
            let y = widgets::list(c, t, f, top, &[standing(installed, GREEN, Icon::Install, ran)]).bottom;
            let next = widgets::list(c, t, f, y + gap, &[restart, again]);
            out.extend(hits(&next.rows, &[Action::RebootTo(Target::System), another]));
            let y = reflashed(c, cx, next.bottom);
            let y = afterwards_of(c, cx, y, true);
            script_output(c, cx, y, rows)
        }
        Outcome::SystemInstalled { name } => {
            let detail = crate::t!(S_IS_INSTALLED_AS_THE_SECOND_SYSTEM_SWITCH_TO_IT_FROM_DUAL_BOOT, name = name);
            let y = widgets::list(c, t, f, top, &[standing(installed, GREEN, Icon::Systems, &detail)]).bottom;
            let next = widgets::list(c, t, f, y + gap, &[Row::new(DUAL_BOOT).icon(PURPLE, Icon::Systems)]);
            out.extend(hits(&next.rows, &[Action::Open(Screen::Systems)]));
            afterwards_of(c, cx, next.bottom, true)
        }
        Outcome::Cancelled => {
            let y = widgets::list(c, t, f, top, &[standing(S_CANCELLED, SLATE, Icon::Install, nothing_written)]).bottom;
            let next = widgets::list(c, t, f, y + gap, &[again]);
            out.extend(hits(&next.rows, &[another]));
            next.bottom
        }
        Outcome::Refused(r) => {
            let reason = fmt::refusal(l, r, &cx.info.codename, source, cx.state.time_zone);
            let y = widgets::list(c, t, f, top, &[standing(S_CANT_INSTALL, ORANGE, Icon::Install, &reason)]).bottom;
            // What the system asked for and did not go in leaves the phone
            // where it was, from which it may go back to the system.
            let (rows, acts) = match source {
                Source::Asked { .. } => (vec![again, restart], vec![another, Action::RebootTo(Target::System)]),
                _ => (vec![again], vec![another]),
            };
            let next = widgets::list(c, t, f, y + gap, &rows);
            out.extend(hits(&next.rows, &acts));
            let y = widgets::footnote(c, t, f, next.bottom, nothing_written);
            afterwards_of(c, cx, y, false)
        }
        Outcome::Failed(failure) => {
            let (what, detail) = fmt::failure(l, failure, source);
            let y = widgets::list(c, t, f, top, &[standing(S_INSTALL_FAILED, RED, Icon::Install, &what)]).bottom;
            let next = widgets::list(c, t, f, y + gap, &[again, restart]);
            out.extend(hits(&next.rows, &[another, Action::RebootTo(Target::System)]));
            // The package is sent again, or installed or flashed again.
            let (install, flash) = match source {
                Source::Computer | Source::Rescue => (S_SEND, S_SEND),
                Source::File(_) | Source::Opened { .. } | Source::Asked { .. } => (S_INSTALL_2, S_FLASH),
            };
            let (install, flash) = (install, flash);
            let warning = match damage {
                Some(Damage::System) => crate::t!(S_THE_CURRENT_SYSTEM_WAS_PARTLY_WRITTEN_OVER_SO_A_REBOOT_STARTS_NO_SYSTEM_THE_FULL_OTA_PACKAGE_AGAIN_AND_REBOOT_ONCE_ITS_INSTALLED, install = install),
                Some(Damage::Partition(p)) => crate::t!(S_WAS_ONLY_PARTLY_WRITTEN_SO_THE_PHONE_MAY_NOT_START_PROPERLY_AFTER_A_REBOOT_THE_IMAGE_AGAIN_AND_REBOOT_ONCE_ITS_FLASHED, p = p, flash = flash),
                Some(Damage::Script) => crate::t!(S_THIS_PHONES_PARTITIONS_MAY_HAVE_BEEN_PARTLY_REWRITTEN_SO_IT_MAY_NOT_START_PROPERLY_AFTER_A_REBOOT_THE_ZIP_AGAIN_AND_REBOOT_ONCE_ITS_INSTALLED, install = install),
                None => S_THE_CURRENT_SYSTEM_IS_UNTOUCHED.into(),
            };
            let mut y = widgets::footnote(c, t, f, next.bottom, &warning);
            if let Some(d) = detail {
                y = widgets::footnote(c, t, f, y, &d);
            }
            let y = reflashed(c, cx, y);
            let y = afterwards_of(c, cx, y, false);
            script_output(c, cx, y, rows)
        }
    }
}

/// What comes after an install, where it is not the user's to choose:
/// rescue waits for the computer's next command, and a package the system
/// asked for, once in, has the phone leave on its own.
fn afterwards_of(c: &mut Canvas, cx: &Ctx, y: f32, installed: bool) -> f32 {
    let _l = cx.state.lang;
    let leaving = match cx.state.asked.shutdown_after {
        true => S_POWERING_OFF_SHORTLY,
        false => S_REBOOTING_SHORTLY,
    };
    match cx.state.source {
        Source::Rescue => widgets::footnote(c, cx.t, cx.f, y, MORE_COMMANDS),
        Source::Asked { .. } if installed => widgets::footnote(c, cx.t, cx.f, y, leaving),
        _ => y,
    }
}

/// What the wipe page lists of [`WIPES`] beyond everything, under the
/// partition each is in.
const WIPE_GROUPS: [(Text, &[Wipe]); 2] = [
    (DATA_PART, &[Wipe::AppData, Wipe::Media, Wipe::AppCaches, Wipe::SystemCaches, Wipe::Art, Wipe::Logs, Wipe::Updates]),
    (S_CACHE_PARTITION, &[Wipe::Cache]),
];

/// What a factory reset erases, as the wipe page and an update that asks
/// for one tell it; in English as it goes on from what leads to it, its
/// first letter small.
fn factory_reset(s: &State) -> String {
    let also = match s.cache_partition {
        true => S_THE_METADATA_AND_CACHE_PARTITIONS,
        false => S_THE_METADATA_PARTITION,
    };
    crate::t!(S_EMPTIES_THE_DATA_PARTITION_APPS_ACCOUNTS_SETTINGS_AND_INTERNAL_STORAGE_INCLUDED_AND_FORMATS, also = also)
}

/// A choice of the wipe page as it lists it: its title, and exactly what it
/// erases.
fn wipe_row(w: Wipe, s: &State) -> (&'static str, String) {
    let l = s.lang;
    let (title, what): (Text, String) = match w {
        Wipe::All => (S_ALL_DATA, capitalized(l, &factory_reset(s))),
        Wipe::AppData => (
            S_APP_DATA,
            S_DATA_DATA_DATA_USER_DATA_USER_DE_AND_ANDROID_DATA_AND_ANDROID_MEDIA_IN_INTERNAL_STORAGE_APPS_GO_BACK_TO_HOW_THEY_WERE_FIRST_INSTALLED
            .into(),
        ),
        Wipe::Media => (
            S_INTERNAL_STORAGE,
            S_DATA_MEDIA_THE_PHOTOS_VIDEOS_AND_FILES_IN_SDCARD.into(),
        ),
        Wipe::AppCaches => (
            S_APP_CACHES,
            S_CACHE_AND_CODE_CACHE_IN_EACH_APPS_DATA_AND_CACHE_IN_ANDROID_DATA_APPS_MAKE_THEM_AGAIN_AS_THEY_NEED_THEM
            .into(),
        ),
        Wipe::SystemCaches => (
            S_SYSTEM_CACHES,
            S_DATA_SYSTEM_PACKAGE_CACHE_DATA_RESOURCE_CACHE_AND_DATA_CACHE_MADE_AGAIN_AT_THE_NEXT_START
            .into(),
        ),
        Wipe::Art => (
            S_ART_CACHE,
            S_DATA_DALVIK_CACHE_DATA_MISC_APEXDATA_COM_ANDROID_ART_DALVIK_CACHE_AND_DATA_OTA_COMPILED_AGAIN_AT_THE_NEXT_START_WHICH_THEN_TAKES_LONGER
            .into(),
        ),
        Wipe::Logs => {
            let own = match (l, s.phone_logs) {
                (_, 0) => String::new(),
                (Lang::En, 1) => ", and this phone's own log folder".into(),
                (_, n) => crate::t!(S_AND_THIS_PHONES_OWN_LOG_FOLDERS, n = n),
            };
            let what = crate::t!(S_SYSTEM_LOGS_AND_CRASH_RECORDS_SUCH_AS_DATA_TOMBSTONES_DATA_ANR_DATA_SYSTEM_DROPBOX_AND_DATA_MISC_LOGD_EACH_USERS_BUG_REPORTS, own = own);
            (S_LOGS, what)
        }
        Wipe::Updates => (
            S_DOWNLOADED_UPDATES,
            S_SYSTEM_UPDATE_PACKAGES_IN_DATA_OTA_PACKAGE.into(),
        ),
        Wipe::Cache => (
            S_EVERYTHING,
            S_FORMATS_THE_CACHE_PARTITION_KEEPING_THE_LOGS_IN_ITS_RECOVERY_FOLDER.into(),
        ),
    };
    (title, what)
}

/// What to erase: everything, as a factory reset does, or parts of it on
/// their own, the cache partition among them where the phone has one; then
/// a swipe erases what is chosen.
pub fn wipe(cx: &Ctx) -> Frame {
    let (t, f, s, _l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let chosen = |w: Wipe| s.wipe[WIPES.iter().position(|&x| x == w).expect("every choice is one of WIPES")];
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let y = widgets::section_first(&mut c, t, f, top, S_FACTORY_RESET);
    let (name, sub) = wipe_row(Wipe::All, s);
    let all = widgets::list(&mut c, t, f, y, &[Row::new(name).subtitle(&sub).trail(Trail::Check(chosen(Wipe::All)))]);
    out.extend(hits(&all.rows, &[Action::WipeItem(Wipe::All)]));
    let note = S_THE_SYSTEM_AND_INSTALLED_UPDATES_STAY_THE_PHONE_HAS_TO_BE_SET_UP_AGAIN_AFTERWARDS;
    let mut y = widgets::footnote(&mut c, t, f, all.bottom, note);
    for (title, items) in WIPE_GROUPS {
        let shown: Vec<Wipe> = items.iter().copied().filter(|&w| w != Wipe::Cache || s.cache_partition).collect();
        if shown.is_empty() {
            continue;
        }
        let below = widgets::section(&mut c, t, f, y, title);
        let texts: Vec<(&str, String)> = shown.iter().map(|&w| wipe_row(w, s)).collect();
        let rows: Vec<Row> =
            shown.iter().zip(&texts).map(|(&w, (name, sub))| Row::new(name).subtitle(sub).trail(Trail::Check(chosen(w)))).collect();
        let list = widgets::list(&mut c, t, f, below, &rows);
        out.extend(hits(&list.rows, &shown.iter().map(|&w| Action::WipeItem(w)).collect::<Vec<_>>()));
        y = list.bottom;
    }
    let note = S_WIPING_APP_DATA_OR_APP_CACHES_NEEDS_THE_DATA_PARTITION_DECRYPTED_FIRST_WHAT_IS_WIPED_CANT_BE_BROUGHT_BACK;
    let y = widgets::footnote(&mut c, t, f, y, note);
    let (label, danger) = confirm_label(Confirm::Wipe, cx.state);
    let track = widgets::confirm(&mut c, t, f, y + t.m.dp(24.0), label, danger, s.confirm);
    out.push(Hit { rect: track, action: Action::Confirm(Confirm::Wipe) });
    Frame::new(t, fit(c, t, track.y + track.h), out, S_WIPE_DATA, top)
}

/// A page laid out above the on-screen keyboard: where the keyboard's top
/// is, and its keys as touch targets.
fn keyboard_hits(c: &mut Canvas, t: &Theme, f: &Fonts, s: &State, enter: &str, out: &mut Vec<Hit>) -> f32 {
    let top = t.m.h as f32 - widgets::keyboard_height(t);
    let keys = widgets::keyboard(c, t, f, s.lang, top, s.layer, s.key_held(), enter);
    out.extend(keys.into_iter().map(|(rect, cap)| Hit { rect, action: Action::Type(cap) }));
    top
}

/// Finding a feature or partition: the field where the title would be, the
/// button that leaves the search beside it, what it finds, and the
/// keyboard.
pub fn search(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let m = &t.m;
    let mut c = page(cx);
    let mut out = Vec::new();
    let (at, cancel) = widgets::search_layout(t);
    let field = widgets::search_field(&mut c, t, f, at.x, at.w, at.y + at.h / 2.0, &s.search, SEARCH_HINT);
    widgets::cancel_button(&mut c, t, cancel);
    out.push(Hit { rect: cancel, action: Action::CloseSearch });
    let top = field.field.y + field.field.h + m.dp(12.0);
    let kb_top = m.h as f32 - widgets::keyboard_height(t);
    if s.search.trim().is_empty() {
        let how = S_TYPE_THE_NAME_OF_A_FEATURE_OR_PARTITION_SUCH_AS_BACKUP_TO_FIND_BACKUP;
        widgets::footnote(&mut c, t, f, top, how);
    } else {
        let found = search::find_on(&s.search, l, |screen| s.offers(screen));
        if found.is_empty() {
            let typed = s.search.trim();
            let none = crate::t!(S_NOTHING_FOUND_FOR, typed = typed);
            widgets::list(&mut c, t, f, top, &[Row::new(&none).trail(Trail::None)]);
        } else {
            let rows: Vec<Row> = found.iter().map(|e| Row::new(e.title).subtitle(e.place)).collect();
            let list = widgets::list(&mut c, t, f, top, &rows);
            // The keyboard covers what does not fit above it.
            let shown = list.rows.iter().zip(&found).filter(|(r, _)| r.y + r.h <= kb_top);
            out.extend(shown.map(|(&rect, e)| Hit { rect, action: Action::Open(e.screen) }));
        }
    }
    let find = S_SEARCH;
    keyboard_hits(&mut c, t, f, s, find, &mut out);
    if let Some(rect) = field.clear {
        out.push(Hit { rect, action: Action::ClearSearch });
    }
    Frame { back: false, header: false, caret: Some(field.caret), ..Frame::new(t, c, out, find, 0.0) }
}

/// Backing up: what to take, where it goes, and how.
pub fn backup(cx: &Ctx) -> Frame {
    let (t, f, s, i, l) = (cx.t, cx.f, cx.state, cx.info, cx.state.lang);
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    // Said above the choices, not in the notes under them: a backup holds
    // nothing of the phone's secure storage, so the time to take one is
    // while no screen lock is set.
    let sound = S_MAKE_THE_BACKUP_WITH_NO_SCREEN_LOCK_SET_SO_THAT_RESTORING_IT_CANNOT_LEAVE_THE_PHONE_LOCKED_OUT;
    let y = widgets::warning(&mut c, t, f, top, sound) + t.m.dp(12.0);
    let y = widgets::section_first(&mut c, t, f, y, S_CONTENTS);
    let system = fmt::snapshot_backup_note(l, &i.snapshot).unwrap_or(S_THE_CURRENT_SLOTS_PARTITIONS_IN_SUPER);
    let data = match s.data == Data::Open {
        true => S_APPS_ACCOUNTS_AND_SETTINGS_WITHOUT_INTERNAL_STORAGE_THE_SCREEN_LOCK_OR_FINGERPRINTS,
        false => S_APPS_ACCOUNTS_AND_SETTINGS_WITHOUT_INTERNAL_STORAGE_THE_SCREEN_LOCK_OR_FINGERPRINTS_DECRYPT_FIRST,
    };
    let boot = S_BOOT_INIT_BOOT_VENDOR_BOOT_DTBO_VBMETA_AND_RECOVERY;
    let modem = S_IMEI_MODEM_SETTINGS_AND_SENSOR_CALIBRATION;
    let subs = [boot, system, data, modem];
    let rows: Vec<Row> = BACKUP_PARTS
        .iter()
        .zip(subs)
        .enumerate()
        .map(|(k, (name, sub))| Row::new(name).subtitle(sub).trail(Trail::Check(s.backup[k])))
        .collect();
    let parts = widgets::list(&mut c, t, f, y, &rows);
    out.extend(hits(&parts.rows, &[0, 1, 2, 3].map(Action::BackupPart)));
    let y = widgets::section(&mut c, t, f, parts.bottom, S_SAVE_TO);
    let storage = match (s.data.storage_open(), i.storage.free.map(fmt::gb)) {
        (true, Some(free)) => crate::t!(S_FREE, free = free),
        (true, None) => S_FREE_SPACE_UNKNOWN.to_string(),
        (false, _) if s.data == Data::Erased => DATA_ERASED.to_string(),
        (false, _) => DECRYPT_NEEDED.to_string(),
    };
    let usb = medium_said(&s.usb_drive, S_CONNECTED, USB_DRIVE_UNREADABLE, USB_DRIVE_WANTED);
    let mut rows = vec![
        Row::new(place_name(l, Place::Storage)).subtitle(&storage).trail(Trail::Check(s.backup_to == Place::Storage)),
        Row::new(place_name(l, Place::UsbDrive)).subtitle(usb).trail(Trail::Check(s.backup_to == Place::UsbDrive)),
    ];
    let mut ways = vec![Action::BackupTo(Place::Storage), Action::BackupTo(Place::UsbDrive)];
    // The card in the tray, on a phone whose config names one.
    if s.sdcard_supported {
        let card = medium_said(&s.sdcard, S_CONNECTED, SD_CARD_UNREADABLE, SD_CARD_WANTED);
        rows.push(Row::new(place_name(l, Place::SdCard)).subtitle(card).trail(Trail::Check(s.backup_to == Place::SdCard)));
        ways.push(Action::BackupTo(Place::SdCard));
    }
    let places = widgets::list(&mut c, t, f, y, &rows);
    out.extend(hits(&places.rows, &ways));
    let y = widgets::section(&mut c, t, f, places.bottom, S_OPTIONS);
    let options = widgets::list(&mut c, t, f, y, &[
        Row::new(S_COMPRESS).subtitle(S_SMALLER_AND_SLOWER_TO_MAKE).trail(Trail::Switch(s.compress)),
        Row::new(S_VERIFY_WHEN_DONE).subtitle(S_CHECKS_THE_BACKUPS_FILES_ARE_WHOLE).trail(Trail::Switch(s.verify)),
        Row::new(S_SKIP_EMPTY_SPACE)
            .subtitle(S_ZEROS_TAKE_NO_ROOM_IN_THE_FILES)
            .trail(Trail::Switch(s.sparse)),
    ]);
    out.extend(hits(
        &options.rows,
        &[Action::Toggle(Switch::Compress), Action::Toggle(Switch::Verify), Action::Toggle(Switch::Sparse)],
    ));
    let note = S_BACKUPS_GO_IN_THE_SKYWARD_BACKUPS_FOLDER_NAMED_BY_WHEN_THEY_BEGAN;
    let y = widgets::footnote(&mut c, t, f, options.bottom, note);
    let (label, danger) = confirm_label(Confirm::Backup, cx.state);
    let track = widgets::confirm(&mut c, t, f, y + t.m.dp(24.0), label, danger, s.confirm);
    out.push(Hit { rect: track, action: Action::Confirm(Confirm::Backup) });
    Frame::new(t, fit(c, t, track.y + track.h), out, S_BACKUP, top)
}

/// The backups found, newest first, to restore one; those that cannot be
/// read are listed after them, to be deleted.
pub fn restore(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let y = widgets::section_first(&mut c, t, f, top, S_BACKUPS);
    let bottom = if s.backups.is_empty() {
        widgets::list(&mut c, t, f, y, &[Row::new(S_NO_BACKUPS).trail(Trail::None)]).bottom
    } else {
        let texts: Vec<(String, String)> = s
            .backups
            .iter()
            .map(|b| match &b.held {
                Ok(h) => (fmt::datetime(l, h.made, s.time_zone), format!("{} · {} · {}", fmt::parts(l, &h.parts), fmt::bytes(h.size), place_name(l, b.place))),
                Err(_) => (b.name.clone(), format!("{} · {}", UNREADABLE, place_name(l, b.place))),
            })
            .collect();
        let rows: Vec<Row> = texts.iter().map(|(title, sub)| Row::new(title).subtitle(sub)).collect();
        let list = widgets::list(&mut c, t, f, y, &rows);
        out.extend(list.rows.iter().enumerate().map(|(k, &rect)| Hit { rect, action: Action::OpenBackup(k) }));
        list.bottom
    };
    let note = S_BACKUPS_ARE_LOOKED_FOR_IN_THE_SKYWARD_BACKUPS_FOLDERS_OF_INTERNAL_STORAGE_AND_THE_USB_DRIVE_INTERNAL_STORAGE_NEEDS_THE_DATA_PARTITION_DECRYPTED_FIRST;
    let bottom = widgets::footnote(&mut c, t, f, bottom, note);
    Frame::new(t, fit(c, t, bottom), out, S_RESTORE, top)
}

/// One backup: when and where it was made, and which of its parts to
/// restore. One made on another phone is not restored here, and one that
/// cannot be read can only be deleted.
pub fn restore_backup(cx: &Ctx) -> Frame {
    let (t, f, s, i, l) = (cx.t, cx.f, cx.state, cx.info, cx.state.lang);
    let (entry, chosen) = s.restore.as_ref().expect("the restore page shows a backup");
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let location = S_LOCATION;
    let y = match &entry.held {
        Err(why) => {
            let facts = [(S_FOLDER, entry.name.clone()), (location, place_name(l, entry.place).to_string())];
            let y = widgets::info_card(&mut c, t, f, top, &facts);
            let note = crate::t!(S_THIS_BACKUP_CANT_BE_READ_IT_MAY_NOT_HAVE_FINISHED_OR_IT_MAY_BE_DAMAGED, why = why);
            widgets::footnote(&mut c, t, f, y, &note)
        }
        Ok(held) => {
            let mut facts = vec![
                (S_MADE, fmt::datetime(l, held.made, s.time_zone)),
                (S_SIZE, fmt::bytes(held.size)),
                (location, place_name(l, entry.place).to_string()),
            ];
            if let Some(system) = &held.system {
                facts.push((S_SYSTEM_VERSION, system.clone()));
            }
            if let Some(slot) = held.slot {
                facts.push((S_SLOT_2, slot.letter().to_string()));
            }
            let y = widgets::info_card(&mut c, t, f, top, &facts);
            if held.device != i.codename {
                let device = &held.device;
                let note = crate::t!(S_THIS_BACKUP_IS_FROM_ANOTHER_DEVICE_AND_CANT_BE_RESTORED_TO_THIS_PHONE, device = device);
                widgets::footnote(&mut c, t, f, y, &note)
            } else {
                let y = widgets::section(&mut c, t, f, y, S_CONTENTS);
                let kept: Vec<usize> = (0..BACKUP_PARTS.len()).filter(|&k| held.parts[k]).collect();
                let rows: Vec<Row> = kept.iter().map(|&k| Row::new(BACKUP_PARTS[k]).trail(Trail::Check(chosen[k]))).collect();
                let list = widgets::list(&mut c, t, f, y, &rows);
                out.extend(list.rows.iter().zip(&kept).map(|(&rect, &k)| Hit { rect, action: Action::RestorePart(k) }));
                let note = S_RESTORING_WRITES_THE_BACKUP_OVER_WHAT_THE_PARTS_CHOSEN_HOLD_NOW;
                let mut y = widgets::footnote(&mut c, t, f, list.bottom, note);
                if chosen[2] {
                    let note = S_THE_DATA_PARTITION_GETS_ITS_APPS_ACCOUNTS_AND_SETTINGS_BACK_AND_INTERNAL_STORAGE_THE_SCREEN_LOCK_AND_FINGERPRINTS_STAY_AS_THEY_ARE_AFTER_A_FACTORY_RESET_AN_EARLIER_BACKUP_OF_THE_DATA_PARTITION_CANT_BE_RESTORED;
                    y = widgets::footnote(&mut c, t, f, y, note);
                }
                // The boot partitions' vbmeta checks the system's: one of
                // another version without the other may not start.
                if let (Some(was), Some(now)) = (&held.system, &i.os_version)
                    && was != now && chosen[0] != chosen[1] {
                        let warn = crate::t!(S_THIS_BACKUP_IS_FROM_AND_THIS_PHONE_RUNS_RESTORING_ONLY_ONE_OF_THE_BOOT_AND_SYSTEM_PARTITIONS_MAY_KEEP_THE_PHONE_FROM_STARTING, was = was, now = now);
                        y = widgets::footnote(&mut c, t, f, y, &warn);
                    }
                let (label, danger) = confirm_label(Confirm::Restore, cx.state);
                let track = widgets::confirm(&mut c, t, f, y + t.m.dp(24.0), label, danger, s.confirm);
                out.push(Hit { rect: track, action: Action::Confirm(Confirm::Restore) });
                track.y + track.h
            }
        }
    };
    let delete = widgets::list(&mut c, t, f, y + t.m.dp(28.0), &[Row::new(S_DELETE_BACKUP).tint(t.pal.danger).trail(Trail::None)]);
    out.extend(hits(&delete.rows, &[Action::AskDelete]));
    let mut bottom = delete.bottom;
    if s.asking_delete {
        let ask = S_DELETE_THIS_BACKUP_IT_CANT_BE_BROUGHT_BACK;
        let y = widgets::footnote(&mut c, t, f, delete.bottom, ask);
        let (label, danger) = confirm_label(Confirm::DeleteBackup, cx.state);
        let track = widgets::confirm(&mut c, t, f, y + t.m.dp(24.0), label, danger, s.confirm);
        out.push(Hit { rect: track, action: Action::Confirm(Confirm::DeleteBackup) });
        bottom = track.y + track.h;
    }
    Frame::new(t, fit(c, t, bottom), out, S_RESTORE_BACKUP, top)
}

/// The share of its pass the part under way has done.
fn part_share(a: &BackupAt) -> f32 {
    if a.of == 0 {
        1.0
    } else {
        share(a.done, a.of).min(1.0)
    }
}

/// How far backing up or restoring has come over all, 0 to 1, and the
/// bytes of the parts the pass under way has gone through, the part under
/// way by its share. Each part counts by its bytes in each pass: a restore
/// checks all its parts first, and a backup checks them last.
fn job_progress(job: &BackupJob, a: &BackupAt) -> (f32, u64) {
    let total: u64 = job.plan.iter().map(|(_, n)| n).sum();
    let before: u64 = job.plan[..a.item].iter().map(|(_, n)| n).sum();
    let passed = before as f64 + f64::from(part_share(a)) * job.plan[a.item].1 as f64;
    let second = job.checks && a.checking != job.restore;
    let passes = if job.checks { 2.0 } else { 1.0 };
    let over = (if second { total as f64 } else { 0.0 } + passed) / (passes * total.max(1) as f64);
    (over as f32, passed as u64)
}

/// What a backup, or a restore, has done to a part, and does to it, in
/// each language.
fn job_words(restore: bool) -> (Text, Text) {
    match restore {
        true => (S_RESTORED_2, S_RESTORING),
        false => (S_BACKED_UP_2, S_BACKING_UP),
    }
}

const VERIFIED: Text = S_VERIFIED;
const VERIFYING: Text = S_VERIFYING_2;
const WAITING_TURN: Text = S_WAITING;
const DONE: Text = S_DONE_2;

/// Where part `k` of a backup or restore stands, as its row says it.
fn part_state(_l: Lang, job: &BackupJob, k: usize) -> String {
    use std::cmp::Ordering;
    let (did, doing) = job_words(job.restore);
    let ended = job.outcome.is_some();
    let a = match (&job.outcome, job.at) {
        (Some(BackupEnd::Saved(_)), _) => return if job.checks { VERIFIED } else { did }.to_string(),
        (Some(BackupEnd::Restored { .. }), _) => return did.to_string(),
        (_, None) => return if ended { S_NOT_STARTED } else { WAITING_TURN }.to_string(),
        (_, Some(a)) => a,
    };
    let state = match k.cmp(&a.item) {
        Ordering::Less if a.checking => VERIFIED,
        Ordering::Less => did,
        Ordering::Equal if ended => S_UNFINISHED,
        Ordering::Equal => return format!("{} {}%", if a.checking { VERIFYING } else { doing }, percent(part_share(&a))),
        // Ahead of the pass under way: a backup checks its parts once all
        // are written, and a restore writes them once all are checked.
        Ordering::Greater => match (a.checking, job.restore, ended) {
            (true, false, false) => did,
            (true, false, true) => S_NOT_VERIFIED,
            (false, false, false) | (true, true, false) => WAITING_TURN,
            (false, false, true) => S_NOT_BACKED_UP,
            (false, true, false) => VERIFIED,
            (_, true, true) => S_NOT_RESTORED,
        },
    };
    state.to_string()
}

/// Backing up or restoring: how far it has come over all, each part and
/// where it stands, and a way to stop while stopping breaks nothing. Once
/// it has ended, how, and where to go.
pub fn backup_job(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let job = s.backup_job.as_ref().expect("the backup page shows work");
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let (did, doing) = job_words(job.restore);
    let icon = if job.restore { Icon::Restore } else { Icon::Backup };
    let writing = job.at.is_some_and(|a| !a.checking);
    let mut y = match &job.outcome {
        None => {
            let (title, detail, done) = match job.at {
                Some(a) => {
                    let (over, passed) = job_progress(job, &a);
                    let total = fmt::bytes(job.plan.iter().map(|(_, n)| n).sum());
                    let (verb, past) = if a.checking { (VERIFYING, VERIFIED) } else { (doing, did) };
                    let (name, passed) = (&job.plan[a.item].0, fmt::bytes(passed));
                    (format!("{verb} {name}"), crate::t!(S_OF, past = past, passed = passed, total = total), over)
                }
                None if job.restore => (
                    S_READING_THE_BACKUP.to_string(),
                    S_CHECKING_THE_BACKUP_AGAINST_THIS_PHONE.to_string(),
                    0.0,
                ),
                None => (GETTING_READY.to_string(), S_WORKING_OUT_THE_BACKUPS_SIZE.to_string(), 0.0),
            };
            widgets::progress_card(&mut c, t, f, top, &title, &detail, Some(done))
        }
        Some(end) => {
            let gone = if job.restore { S_NOTHING_WAS_WRITTEN } else { S_NO_BACKUP_WAS_SAVED };
            let failed = if job.restore { RESTORE_FAILED } else { S_BACKUP_FAILED };
            let (title, color, detail) = match end {
                BackupEnd::Saved(name) => {
                    let saved = match job.place {
                        Place::Storage => crate::t!(S_SAVED_TO_SKYWARD_BACKUPS_IN_INTERNAL_STORAGE, name = name),
                        Place::UsbDrive => crate::t!(S_SAVED_TO_SKYWARD_BACKUPS_ON_THE_USB_DRIVE, name = name),
                        Place::SdCard => crate::t!(S_SAVED_TO_SKYWARD_BACKUPS_ON_THE_SD_CARD, name = name),
                    };
                    (BACKUP_COMPLETE, GREEN, saved)
                }
                BackupEnd::Restored { .. } => {
                    (RESTORE_COMPLETE, GREEN, S_THE_PHONE_USES_WHAT_WAS_RESTORED_ONCE_IT_REBOOTS.to_string())
                }
                BackupEnd::Stopped => (S_STOPPED, SLATE, gone.to_string()),
                BackupEnd::Refused(r) => {
                    let title = if job.restore { S_CANT_RESTORE } else { S_CANT_BACK_UP };
                    (title, ORANGE, fmt::backup_refusal(l, r, job.restore))
                }
                BackupEnd::Failed(_) if job.restore && writing => {
                    let detail = S_PART_OF_IT_WAS_WRITTEN_SO_THE_PHONE_MAY_NOT_START_PROPERLY_AFTER_A_REBOOT_RESTORE_AGAIN_AND_REBOOT_ONCE_ITS_DONE;
                    (failed, RED, detail.to_string())
                }
                BackupEnd::Failed(_) => (failed, RED, gone.to_string()),
            };
            widgets::list(&mut c, t, f, top, &[standing(title, color, icon, &detail)]).bottom
        }
    };
    let keep = match job.place {
        Place::Storage => S_DONT_POWER_OFF,
        Place::UsbDrive => S_KEEP_THE_USB_DRIVE_IN_AND_DONT_POWER_OFF,
        Place::SdCard => S_KEEP_THE_SD_CARD_IN_AND_DONT_POWER_OFF,
    };
    match &job.outcome {
        None => {
            let note = match (job.restore, job.at) {
                (false, _) => crate::t!(S_UNTIL_THE_BACKUP_IS_DONE_STOPPING_DELETES_THE_UNFINISHED_BACKUP, keep = keep),
                (true, Some(a)) if !a.checking => crate::t!(S_UNTIL_THE_RESTORE_IS_DONE, keep = keep),
                (true, _) => S_NOTHING_IS_WRITTEN_UNTIL_THE_BACKUP_IS_VERIFIED.to_string(),
            };
            y = widgets::footnote(&mut c, t, f, y, &note);
            // A restore that has begun writing goes on to its end.
            if !(job.restore && writing) {
                let label = if job.stopping { S_STOPPING } else { STOP };
                let stop = widgets::list(&mut c, t, f, y + t.m.card_gap, &[Row::new(label).trail(Trail::None)]);
                if !job.stopping {
                    out.extend(hits(&stop.rows, &[Action::StopBackup]));
                }
                y = stop.bottom;
            }
        }
        Some(end) => {
            match end {
                BackupEnd::Restored { unmade } if !unmade.is_empty() => {
                    let folders = unmade.join(S_TEXT_4);
                    let note = crate::t!(S_THE_USERS_THESE_FOLDERS_BELONGED_TO_NO_LONGER_EXIST_SO_WHAT_THEY_HOLD_WASNT_RESTORED, folders = folders);
                    y = widgets::footnote(&mut c, t, f, y, &note);
                }
                BackupEnd::Failed(why) => y = widgets::footnote(&mut c, t, f, y, why),
                _ => {}
            }
            let (rows, acts): (Vec<Row>, Vec<Action>) = match end {
                BackupEnd::Restored { .. } => (
                    vec![Row::new(S_REBOOT_TO_SYSTEM).icon(ORANGE, Icon::Power), Row::new(DONE).trail(Trail::None)],
                    vec![Action::RebootTo(Target::System), Action::Back],
                ),
                _ => (vec![Row::new(DONE).trail(Trail::None)], vec![Action::Back]),
            };
            let next = widgets::list(&mut c, t, f, y + t.m.card_gap, &rows);
            out.extend(hits(&next.rows, &acts));
            y = next.bottom;
        }
    }
    // The parts come after what can be done, which a long list would push
    // off the screen.
    if !job.plan.is_empty() {
        let heading = S_CONTENTS;
        let y0 = widgets::section(&mut c, t, f, y, heading);
        let states: Vec<String> = (0..job.plan.len()).map(|k| part_state(l, job, k)).collect();
        let rows: Vec<Row> = job.plan.iter().zip(&states).map(|((name, _), state)| Row::new(name).trail(Trail::Text(state))).collect();
        y = widgets::list(&mut c, t, f, y0, &rows).bottom;
    }
    let title = match (&job.outcome, job.restore) {
        (None, restore) => job_words(restore).1,
        (Some(BackupEnd::Saved(_)), _) => BACKUP_COMPLETE,
        (Some(BackupEnd::Restored { .. }), _) => RESTORE_COMPLETE,
        (Some(_), false) => S_BACKUP_INCOMPLETE,
        (Some(_), true) => S_RESTORE_INCOMPLETE,
    };
    Frame { back: job.outcome.is_some(), ..Frame::new(t, fit(c, t, y), out, title, top) }
}

const BACKUP_COMPLETE: Text = S_BACKUP_COMPLETE;
const RESTORE_COMPLETE: Text = S_RESTORE_COMPLETE;
const RESTORE_FAILED: Text = S_RESTORE_FAILED;
const STOP: Text = S_STOP;

/// Tools for the experienced.
pub fn advanced(cx: &Ctx) -> Frame {
    let (t, f, _l) = (cx.t, cx.f, cx.state.lang);
    let mut c = page(cx);
    let top = t.m.content_top;
    let build = update_value(cx);
    let mut rows = vec![
        Row::new(FILES).icon(BLUE, Icon::Folder),
        Row::new(TERMINAL).icon(SLATE, Icon::Terminal),
        Row::new(RELABEL).icon(PURPLE, Icon::Shield),
        Row::new(FILESYSTEMS).icon(RED, Icon::Mount),
        Row::new(LOGS).icon(ORANGE, Icon::Log),
    ];
    let mut acts = vec![
        Action::Open(Screen::Files),
        Action::Open(Screen::Terminal),
        Action::Open(Screen::Relabel),
        Action::Open(Screen::Filesystems),
        Action::Open(Screen::Logs),
    ];
    // A phone whose ROM publishes no feed has no system update to offer.
    if cx.state.offers(Screen::Update) {
        rows.push(Row::new(UPDATE).icon(GREEN, Icon::Layers).trail(Trail::Value(&build)));
        acts.push(Action::Open(Screen::Update));
    }
    let tools = widgets::list(&mut c, t, f, top, &rows);
    let out = hits(&tools.rows, &acts);
    Frame::new(t, c, out, ADVANCED, top)
}

const RELABEL: Text = S_FIX_SELINUX_CONTEXTS;
const FILESYSTEMS: Text = S_FILESYSTEMS;
const STOPPED: Text = S_STOPPED;
const STOPPING: Text = S_STOPPING;

/// Giving the data partition's files the SELinux labels the installed
/// system's rules give them: what it does, and a swipe to start it; then
/// how far it has come, a way to stop it, and how it ended.
pub fn relabel(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let title = RELABEL;
    let Some(job) = &s.relabel else {
        let what = S_GIVES_THE_FILES_IN_THE_DATA_PARTITION_THE_SELINUX_CONTEXTS_THE_INSTALLED_SYSTEMS_RULES_GIVE_THEM_APP_DATA_STAYS_AS_IT_IS;
        let mut y = widgets::paragraph(&mut c, t, f, top, what);
        let note = match s.data {
            Data::Open | Data::Locked { .. } => None,
            Data::Erased => Some(S_THE_DATA_PARTITION_WAS_WIPED_2),
            Data::Closed | Data::Opening | Data::Failed(_) => {
                Some(S_OPEN_THE_DATA_PARTITION_IN_DECRYPT_FIRST_2)
            }
        };
        if let Some(note) = note {
            y = widgets::footnote(&mut c, t, f, y, note);
        }
        let (label, danger) = confirm_label(Confirm::Relabel, cx.state);
        let track = widgets::confirm(&mut c, t, f, y + t.m.dp(24.0), label, danger, s.confirm);
        out.push(Hit { rect: track, action: Action::Confirm(Confirm::Relabel) });
        return Frame::new(t, c, out, title, top);
    };
    let repair = Verb::of(&crate::app::PartKind::Repair);
    let heading = match &job.outcome {
        None => repair.going(l),
        Some(JobEnd::Done) => repair.complete(l),
        Some(JobEnd::Stopped) => STOPPED.into(),
        Some(JobEnd::Failed(_)) => repair.failed(l),
    };
    let detail = match (job.total, job.checked) {
        (0, _) if job.outcome.is_none() => GETTING_READY.to_string(),
        (total, checked) => crate::t!(S_OF_CHECKED, checked = checked, total = total),
    };
    let y = widgets::progress_card(&mut c, t, f, top, &heading, &detail, Some(share(job.checked, job.total)));
    match &job.outcome {
        None => {
            let label = if job.stopping { STOPPING } else { STOP };
            let stop = widgets::list(&mut c, t, f, y + t.m.card_gap, &[Row::new(label).trail(Trail::None)]);
            if !job.stopping {
                out.extend(hits(&stop.rows, &[Action::StopRelabel]));
            }
        }
        Some(end) => {
            let changed = job.changed;
            let note = match end {
                JobEnd::Done if changed == 0 => S_EVERY_FILES_SELINUX_CONTEXT_IS_RIGHT_SO_NOTHING_NEEDED_FIXING
                    .to_string(),
                JobEnd::Done => crate::t!(S_ENTRIES_GOT_THE_SELINUX_CONTEXT_THE_RULES_GIVE_THEM, changed = changed),
                JobEnd::Stopped => crate::t!(S_STOPPED_WITH_ENTRIES_SET_AGAIN, changed = changed),
                JobEnd::Failed(why) => crate::t!(S_REPAIR_FAILED, why = why),
            };
            let y = widgets::footnote(&mut c, t, f, y, &note);
            let finish = widgets::list(&mut c, t, f, y + t.m.card_gap, &[Row::new(DONE).trail(Trail::None)]);
            out.extend(hits(&finish.rows, &[Action::Back]));
        }
    }
    Frame { back: job.outcome.is_some(), ..Frame::new(t, c, out, title, top) }
}

/// The partitions whose filesystems can be repaired or made afresh as
/// another, each with what it holds and its size.
pub fn filesystems(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let y = widgets::section_first(&mut c, t, f, top, S_PARTITIONS);
    let bottom = match &s.filesystems {
        None => widgets::list(&mut c, t, f, y, &[Row::new(S_READING_THE_PARTITIONS).trail(Trail::None)]).bottom,
        Some(Err(why)) => {
            let y = widgets::list(&mut c, t, f, y, &[Row::new(S_COULDNT_READ_THE_PARTITIONS).trail(Trail::None)]).bottom;
            widgets::footnote(&mut c, t, f, y, why)
        }
        Some(Ok(rows)) if rows.is_empty() => {
            widgets::list(&mut c, t, f, y, &[Row::new(S_NO_PARTITIONS_TO_REPAIR).trail(Trail::None)]).bottom
        }
        Some(Ok(rows)) => {
            let subs: Vec<String> = rows.iter().map(|r| format!("{} · {}", fs_held(l, r, &s.data), fmt::bytes(r.size))).collect();
            let shown: Vec<Row> = rows.iter().zip(&subs).map(|(r, sub)| Row::new(&r.name).subtitle(sub)).collect();
            let list = widgets::list(&mut c, t, f, y, &shown);
            out.extend(hits(&list.rows, &(0..rows.len()).map(Action::OpenFilesystem).collect::<Vec<_>>()));
            let note = S_ONLY_THE_PARTITIONS_IN_THIS_PHONES_PARTITION_TABLE_THAT_CAN_BE_FORMATTED_SHOW_HERE;
            widgets::footnote(&mut c, t, f, list.bottom, note)
        }
    };
    Frame::new(t, fit(c, t, bottom), out, FILESYSTEMS, top)
}

/// What a partition holds, as its row and page name it: the data
/// partition's is read only once it is open, and until then the row says
/// where it stands, as the home page does.
fn fs_held(l: Lang, row: &FsRow, data: &Data) -> String {
    match (&row.filesystem, row.name == DATA_PARTITION) {
        (Some(filesystem), _) => filesystem.clone(),
        (None, true) => fmt::data(l, data).into(),
        (None, false) => S_UNRECOGNIZED.into(),
    }
}

/// What the page of one partition shows when the list it came from was not
/// read, or no longer holds it.
fn filesystem_missing(cx: &Ctx) -> Frame {
    let (t, f, _l) = (cx.t, cx.f, cx.state.lang);
    let mut c = long_page(cx);
    let top = t.m.content_top;
    let y = widgets::footnote(&mut c, t, f, top, S_COULDNT_READ_THE_PARTITIONS);
    Frame::new(t, fit(c, t, y), Vec::new(), FILESYSTEMS, top)
}

/// One partition: what it holds, what can be done to it, and a swipe to do
/// what is chosen.
pub fn filesystem(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let Some(Ok(rows)) = &s.filesystems else { return filesystem_missing(cx) };
    let Some(row) = rows.get(s.fs_chosen) else { return filesystem_missing(cx) };
    let data = row.name == DATA_PARTITION;
    let facts = [
        (S_MOUNT_POINT, row.mount_point.clone()),
        (S_FILESYSTEM, fs_held(l, row, &s.data)),
        (S_SIZE, fmt::bytes(row.size)),
    ];
    let y = widgets::info_card(&mut c, t, f, top, &facts);
    let y = widgets::section(&mut c, t, f, y, ACTIONS);
    let chosen = |action: &FsAction| s.fs_action == *action;
    let remakes: Vec<String> = row.others.iter().map(|fs| crate::t!(S_CHANGE_TO, fs = fs)).collect();
    let made: Vec<String> = row.others.iter().map(|_| remade(row, s)).collect();
    let repair = Row::new(S_REPAIR_THE_FILESYSTEM)
        .subtitle(S_CHECKS_THE_WHOLE_FILESYSTEM_AND_FIXES_THE_ERRORS_FOUND)
        .trail(Trail::Check(chosen(&FsAction::Repair)));
    let mut shown = vec![repair];
    let mut actions = vec![Action::ChooseFs(FsChoice::Repair)];
    for (k, fs) in row.others.iter().enumerate() {
        shown.push(Row::new(&remakes[k]).subtitle(&made[k]).trail(Trail::Check(chosen(&FsAction::Remake(fs.clone())))));
        actions.push(Action::ChooseFs(FsChoice::Remake(k)));
    }
    let list = widgets::list(&mut c, t, f, y, &shown);
    out.extend(hits(&list.rows, &actions));
    let mut y = list.bottom;
    if row.others.is_empty() {
        let note = S_THIS_PARTITIONS_FILESYSTEM_CANT_BE_CHANGED;
        y = widgets::footnote(&mut c, t, f, y, note);
    }
    if data {
        let note = S_THE_DATA_PARTITION_CLOSES_FOR_THE_REPAIR_AND_OPENS_AGAIN_FROM_DECRYPT_ONCE_ITS_DONE;
        y = widgets::footnote(&mut c, t, f, y, note);
    }
    if !row.others.is_empty() {
        y = widgets::footnote(&mut c, t, f, y, S_ONCE_CHANGED_WHAT_THE_PARTITION_HELD_CANT_BE_BROUGHT_BACK);
    }
    let (label, danger) = confirm_label(Confirm::Filesystem, s);
    let track = widgets::confirm(&mut c, t, f, y + t.m.dp(24.0), label, danger, s.confirm);
    out.push(Hit { rect: track, action: Action::Confirm(Confirm::Filesystem) });
    Frame::new(t, fit(c, t, track.y + track.h), out, partition_title(l, &row.name), top)
}

/// The title of the page of a partition the filesystem page lists.
fn partition_title(_l: Lang, name: &str) -> &'static str {
    match name {
        DATA_PARTITION => DATA_PART,
        "metadata" => S_METADATA_PARTITION,
        "cache" => S_CACHE_PARTITION,
        _ => S_PARTITION,
    }
}

/// What making `row` afresh as another filesystem erases: the data
/// partition goes as a factory reset goes, with /metadata and the cache
/// partition beside it.
fn remade(row: &FsRow, s: &State) -> String {
    match row.name == DATA_PARTITION {
        true => {
            let parts = match s.cache_partition {
                true => S_THE_DATA_CACHE_AND_METADATA_PARTITIONS,
                false => S_THE_DATA_AND_METADATA_PARTITIONS,
            };
            crate::t!(S_FORMATS_EMPTYING_ALL_OF_THIS_PHONES_DATA, parts = parts)
        }
        false => {
            let name = &row.name;
            crate::t!(S_FORMATS_THE_PARTITION_EMPTYING_ALL_IT_HOLDS, name = name)
        }
    }
}

/// Work on whole partitions: each partition and where it stands, what the
/// program said of the last, and once it has ended, how. What the system
/// asked for leaves on its own once it went through every partition, and
/// offers where to go otherwise; what the user chose goes back to its page.
pub fn part_work(cx: &Ctx) -> Frame {
    use crate::app::{ItemEnd, ItemState, PartKind};
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let work = s.part_work.as_ref().expect("the page of partition work shows work");
    let mut c = long_page(cx);
    let (mut out, mut rows) = (Vec::new(), None);
    let top = t.m.content_top;
    let verb = Verb::of(&work.kind);
    // Work on one partition fails; work through several may get partway.
    let (title, several) = match &work.kind {
        PartKind::Disable => (S_WIPE_PARTITIONS, true),
        PartKind::Unshare => (S_UNSHARE_BLOCKS, true),
        PartKind::Repair => (S_REPAIR_FILESYSTEM, false),
        PartKind::Remake(_) => (S_CHANGE_FILESYSTEM, false),
    };
    let failed = |state: &ItemState| matches!(state, ItemState::Ended(ItemEnd::Failed(_)));
    let short = matches!(work.outcome, Some(Err(_))) || work.items.iter().any(|(_, state)| failed(state));
    let heading = match (&work.outcome, short, several) {
        (None, _, _) => verb.going(l),
        (Some(_), false, _) => verb.complete(l),
        (Some(_), true, true) => verb.incomplete(l),
        (Some(_), true, false) => verb.failed(l),
    };
    let mut y = widgets::section_first(&mut c, t, f, top, &heading);
    if !work.items.is_empty() {
        let states: Vec<String> = work
            .items
            .iter()
            .map(|(_, state)| match state {
                ItemState::Waiting if work.outcome.is_some() => verb.not_done(l),
                ItemState::Waiting => WAITING_TURN.into(),
                ItemState::Working => verb.going(l),
                ItemState::Ended(end) => fmt::item_end(l, end, verb),
            })
            .collect();
        let held: Vec<Option<&str>> = work
            .items
            .iter()
            .map(|(_, state)| match state {
                ItemState::Ended(ItemEnd::NotShared(fs)) if !fs.is_empty() => Some(fs.as_str()),
                _ => None,
            })
            .collect();
        let shown: Vec<Row> = work
            .items
            .iter()
            .zip(&states)
            .zip(&held)
            .map(|(((name, _), state), held)| {
                let row = Row::new(name).trail(Trail::Text(state));
                match held {
                    Some(fs) => row.subtitle(fs),
                    None => row,
                }
            })
            .collect();
        y = widgets::list(&mut c, t, f, y, &shown).bottom;
    }
    let leaving = match s.asked.shutdown_after {
        true => S_POWERS_OFF_ON_ITS_OWN_ONCE_DONE,
        false => S_REBOOTS_ON_ITS_OWN_ONCE_DONE,
    };
    let mut notes: Vec<String> = Vec::new();
    match &work.outcome {
        None if work.asked => notes.push(leaving.into()),
        None => notes.push(verb.hold_on(l)),
        Some(Err(stop)) => {
            let (what, detail) = fmt::part_stop(l, stop, &cx.info.codename);
            notes.push(what);
            notes.extend(detail);
        }
        Some(Ok(())) => {}
    }
    for (name, state) in &work.items {
        if let ItemState::Ended(ItemEnd::Failed(Some(why))) = state {
            let failed = verb.failed_on(l, name);
            notes.push(crate::t!(S_TEXT_7, failed = failed, why = why));
        }
    }
    for note in &notes {
        y = widgets::footnote(&mut c, t, f, y, note);
    }
    if let (true, Some(reason)) = (work.asked, &s.asked.reason) {
        y = widgets::info_card(&mut c, t, f, y + t.m.dp(12.0), &[(REASON, reason.clone())]);
    }
    match (&work.outcome, work.asked) {
        (None, _) => {}
        (Some(_), true) if !short => {}
        // The system's work ran without the slot's partitions, which going
        // on in the recovery brings up first, as leaving fastboot does.
        (Some(_), true) => {
            let below = widgets::section(&mut c, t, f, y, ACTIONS);
            let rows: Vec<Row> = AFTERWARDS.iter().map(|&(title, _)| Row::new(title).trail(Trail::None)).collect();
            let actions = AFTERWARDS.map(|(_, action)| if action == Action::Dismiss { Action::EnterRecovery } else { action });
            let choices = widgets::list(&mut c, t, f, below, &rows);
            out.extend(hits(&choices.rows, &actions));
            y = choices.bottom;
        }
        (Some(_), false) => {
            let finish = widgets::list(&mut c, t, f, y + t.m.card_gap, &[Row::new(DONE).trail(Trail::None)]);
            out.extend(hits(&finish.rows, &[Action::Back]));
            y = finish.bottom;
        }
    }
    if !work.output.is_empty() {
        let below = widgets::section(&mut c, t, f, y, S_PROGRAM_OUTPUT);
        rows = Some(Rows { edges: widgets::output_edges(&c, t, f, below, &work.output), action: None });
        y = below;
    }
    let back = work.outcome.is_some() && !work.asked;
    Frame { rows, back, ..Frame::new(t, fit(c, t, y), out, title, top) }
}

const ACTIONS: Text = S_ACTIONS;

/// The system on the phone now; the newest system the phone's ROM
/// publishes, and downloading its full package to install it; and where an
/// update the system installed stands, merging it into the slot it went to:
/// how far a merge under way has come, and a swipe to merge one that waits
/// there for the system to start it.
pub fn update(cx: &Ctx) -> Frame {
    let (t, f, s) = (cx.t, cx.f, cx.state);
    let mut c = long_page(cx);
    let top = t.m.content_top;
    let mut out = Vec::new();
    let mut y = top;
    if let Some(rom) = &s.rom {
        let name = &rom.name;
        let heading = crate::t!(S_SYSTEM_UPDATE, name = name);
        y = widgets::section_first(&mut c, t, f, y, &heading);
        y = rom_update(&mut c, cx, y, &mut out);
    } else {
        y = widgets::footnote(&mut c, t, f, y, S_NO_SYSTEM_UPDATE_IS_OFFERED);
    }
    Frame::new(t, fit(c, t, y), out, UPDATE, top)
}

/// What downloading or checking for updates needs first, while the phone
/// has joined no network that reaches the internet.
const WLAN_FIRST: Text = S_CONNECT_TO_WI_FI_FIRST;

/// A package download under way: how far it has come, how fast, and the
/// control that pauses it. `stop` is the action that pauses this download.
fn download_under_way(c: &mut Canvas, cx: &Ctx, y: f32, out: &mut Vec<Hit>, job: &DownloadJob, stop: Action) -> f32 {
    let (t, f, _l) = (cx.t, cx.f, cx.state.lang);
    let (heading, detail, share) = match job.progress {
        None => (GETTING_READY, String::new(), None),
        Some(Progress::Checking { done, total }) => (
            S_VERIFYING_WHAT_WAS_DOWNLOADED,
            format!("{} / {}", fmt::file_size(done), fmt::file_size(total)),
            Some(share(done, total)),
        ),
        Some(Progress::Downloading { done, total }) => (
            S_DOWNLOADING,
            format!("{} / {} · {}/s", fmt::file_size(done), fmt::file_size(total), fmt::file_size(job.rate)),
            Some(share(done, total)),
        ),
    };
    let y = widgets::progress_card(c, t, f, y + t.m.card_gap, heading, &detail, share);
    let label = if job.stopping { S_PAUSING } else { S_PAUSE };
    let pause = widgets::list(c, t, f, y + t.m.card_gap, &[Row::new(label).trail(Trail::None)]);
    if !job.stopping {
        out.extend(hits(&pause.rows, &[stop]));
    }
    pause.bottom
}

/// The row that downloads a package, or carries on downloading it, with why
/// the last attempt stopped and what the page needs first.
fn download_offer(c: &mut Canvas, cx: &Ctx, y: f32, out: &mut Vec<Hit>, have: Have, job: Option<&DownloadJob>, action: Action) -> f32 {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut y = y;
    match job.and_then(|j| j.outcome.as_ref()) {
        Some(DownloadEnd::Failed(why)) => y = widgets::footnote(c, t, f, y, &fmt::download_failure(l, why)),
        Some(DownloadEnd::Stopped) => y = widgets::footnote(c, t, f, y, S_DOWNLOAD_PAUSED),
        _ => {}
    }
    let title = if matches!(have, Have::Part(_)) { S_RESUME_DOWNLOAD } else { S_DOWNLOAD };
    let had = match have {
        Have::Part(done) => {
            let done = fmt::file_size(done);
            Some(crate::t!(S_DOWNLOADED, done = done))
        }
        _ => None,
    };
    let sub = if !s.data.storage_open() {
        Some(DECRYPT_NEEDED)
    } else if !s.online() {
        Some(WLAN_FIRST)
    } else {
        had.as_deref()
    };
    let row = Row::new(title);
    let download = widgets::list(c, t, f, y + t.m.card_gap, &[sub.map_or(row, |sub| row.subtitle(sub))]);
    out.extend(hits(&download.rows, &[action]));
    download.bottom
}

/// Asking the phone's ROM for its newest system, then its package:
/// downloading it, how far that has come, and installing it through the
/// package installer once it is whole.
fn rom_update(c: &mut Canvas, cx: &Ctx, y: f32, out: &mut Vec<Hit>) -> f32 {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let rom = s.rom.as_ref().expect("the system section shows on a phone with a feed");
    let release = match &rom.check {
        RomCheck::Asking => {
            return widgets::list(c, t, f, y, &[Row::new(S_CHECKING_FOR_UPDATES).trail(Trail::None)]).bottom;
        }
        RomCheck::Other => {
            let note = S_THE_INSTALLED_SYSTEM_IS_NOT_INFINITY_X_SO_NO_SYSTEM_UPDATE_IS_OFFERED;
            return widgets::footnote(c, t, f, y, note);
        }
        RomCheck::Answered(Some(release)) => release,
        other => {
            let mut row = Row::new(S_CHECK_FOR_UPDATES);
            if !s.online() {
                row = row.subtitle(WLAN_FIRST);
            }
            let check = widgets::list(c, t, f, y, &[row]);
            out.extend(hits(&check.rows, &[Action::CheckRom]));
            return match other {
                RomCheck::Failed(why) => widgets::footnote(c, t, f, check.bottom, &fmt::unanswered(l, why)),
                RomCheck::Answered(None) => {
                    let none = S_THE_SYSTEM_IS_THE_NEWEST_ONE_PUBLISHED;
                    widgets::footnote(c, t, f, check.bottom, none)
                }
                _ => check.bottom,
            };
        }
    };
    let size = fmt::file_size(release.image.size);
    let image = crate::t!(S_SYSTEM_IMAGE, size = size);
    let y = widgets::list(c, t, f, y, &[Row::new(&release.build).icon(GREEN, Icon::Layers).subtitle(&image).trail(Trail::None)]).bottom;
    match (&rom.download, rom.have) {
        (Some(job), _) if job.outcome.is_none() => download_under_way(c, cx, y, out, job, Action::StopRomDownload),
        (_, Have::Whole(_)) => {
            let row = Row::new(S_INSTALL_2).subtitle(S_DOWNLOADED_TO_INTERNAL_STORAGE_IT_INSTALLS_TO_THE_OTHER_SLOT_AND_STARTS_AFTER_A_RESTART);
            let install = widgets::list(c, t, f, y + t.m.card_gap, &[row]);
            out.extend(hits(&install.rows, &[Action::InstallRom]));
            install.bottom
        }
        (job, have) => download_offer(c, cx, y, out, have, job.as_ref(), Action::DownloadRom),
    }
}

/// What a progress card says of a download, and the share of it done:
/// what an earlier download left being checked, or the rest coming at
/// `rate` bytes a second.
fn downloaded(_l: Lang, progress: Progress, rate: u64) -> (&'static str, String, Option<f32>) {
    match progress {
        Progress::Checking { done, total } => (CHECKING_PART, format!("{} / {}", fmt::file_size(done), fmt::file_size(total)), Some(share(done, total))),
        Progress::Downloading { done, total } => (
            DOWNLOADING,
            format!("{} / {} · {}/s", fmt::file_size(done), fmt::file_size(total), fmt::file_size(rate)),
            Some(share(done, total)),
        ),
    }
}

/// Skyward's own update: the build running now; Skyward's newest release
/// on GitHub, and updating to it.
pub fn skyward_update(cx: &Ctx) -> Frame {
    let (t, f, _l) = (cx.t, cx.f, cx.state.lang);
    let mut c = long_page(cx);
    let top = t.m.content_top;
    let mut out = Vec::new();
    let y = widgets::section_first(&mut c, t, f, top, S_THIS_RECOVERY);
    let y = widgets::info_card(&mut c, t, f, y, &[(S_CURRENT_BUILD, VERSION.to_string())]);
    let y = widgets::section(&mut c, t, f, y, S_ONLINE_UPDATE);
    let y = skyward_online(&mut c, cx, y, &mut out);
    Frame::new(t, fit(c, t, y), out, SKYWARD_UPDATE, top)
}

/// Asking GitHub for Skyward's newest release, then updating to it: its
/// build for the phone downloaded and written into the recovery partitions,
/// how far that has come, and how it ended.
fn skyward_online(c: &mut Canvas, cx: &Ctx, y: f32, out: &mut Vec<Hit>) -> f32 {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let gap = t.m.card_gap;
    let newest = match &s.skyward_check {
        SkywardCheck::Asking => return widgets::list(c, t, f, y, &[Row::new(CHECKING).trail(Trail::None)]).bottom,
        SkywardCheck::Answered(n) => n,
        other => {
            let mut row = Row::new(CHECK_FOR_UPDATES);
            if !s.online() {
                row = row.subtitle(WLAN_FIRST);
            }
            let check = widgets::list(c, t, f, y, &[row]);
            out.extend(hits(&check.rows, &[Action::CheckSkyward]));
            return match other {
                SkywardCheck::Failed(why) => widgets::footnote(c, t, f, check.bottom, &fmt::unanswered(l, why)),
                _ => check.bottom,
            };
        }
    };
    let version = &newest.version;
    let (title, detail) = match (newest.newer, &newest.build) {
        (true, Some(build)) => {
            let size = fmt::file_size(build.size);
            (crate::t!(S_NEW_VERSION_NAMED, version = version), crate::t!(S_PACKAGE_SIZE, size = size))
        }
        (true, None) => (crate::t!(S_NEW_VERSION_NAMED, version = version), S_NO_PACKAGE_FOR_THIS_PHONE.to_string()),
        (false, _) => (S_UP_TO_DATE.to_string(), crate::t!(S_THE_NEWEST_RELEASE_IS, version = version)),
    };
    let y = widgets::list(c, t, f, y, &[Row::new(&title).icon(GREEN, Icon::Install).subtitle(&detail).trail(Trail::None)]).bottom;
    if !newest.newer || newest.build.is_none() {
        return y;
    }
    match &s.skyward_job {
        Some(job) if job.outcome.is_none() => {
            let (heading, detail, share) = match job.stage {
                None => (S_GETTING_READY, String::new(), None),
                Some(SkywardStage::Download(p)) => downloaded(l, p, job.rate),
                Some(SkywardStage::Write { done, total }) => {
                    let part = share(done, total);
                    let p = percent(part);
                    (S_WRITING_THE_RECOVERY_PARTITIONS, crate::t!(S_DONE_PERCENT, p = p), Some(part))
                }
            };
            let y = widgets::progress_card(c, t, f, y + gap, heading, &detail, share);
            if matches!(job.stage, Some(SkywardStage::Write { .. })) {
                return widgets::footnote(c, t, f, y, S_DONT_POWER_OFF_UNTIL_ITS_WRITTEN);
            }
            let stop = widgets::list(c, t, f, y + gap, &[Row::new(if job.stopping { STOPPING } else { STOP }).trail(Trail::None)]);
            if !job.stopping {
                out.extend(hits(&stop.rows, &[Action::StopSkywardUpdate]));
            }
            stop.bottom
        }
        Some(SkywardJob { outcome: Some(SkywardEnd::Updated(slots)), .. }) => {
            let names = fmt::slots(l, slots);
            let detail = if slots.len() == 1 {
                crate::t!(S_SKYWARD_VERSION_IS_IN_THE_RECOVERY_PARTITION, version = version, names = names)
            } else {
                crate::t!(S_SKYWARD_VERSION_IS_IN_THE_RECOVERY_PARTITIONS, version = version, names = names)
            };
            let y = widgets::list(c, t, f, y + gap, &[standing(S_UPDATED, GREEN, Icon::Install, &detail)]).bottom;
            let restart = widgets::list(c, t, f, y + gap, &[Row::new(S_REBOOT_TO_RECOVERY).icon(ORANGE, Icon::Power)]);
            out.extend(hits(&restart.rows, &[Action::RebootTo(Target::Recovery)]));
            restart.bottom
        }
        job => {
            let mut y = y;
            match job.as_ref().and_then(|j| j.outcome.as_ref()) {
                Some(SkywardEnd::Stopped) => y = widgets::footnote(c, t, f, y, S_DOWNLOAD_STOPPED),
                Some(SkywardEnd::Download(why)) => y = widgets::footnote(c, t, f, y, &fmt::skyward_download_failure(l, why)),
                Some(SkywardEnd::NotWritten(why)) => {
                    y = widgets::footnote(c, t, f, y, S_COULDNT_UPDATE_NOTHING_WAS_WRITTEN);
                    y = widgets::footnote(c, t, f, y, why);
                }
                Some(SkywardEnd::WriteFailed(slots, why)) => {
                    let names = fmt::slots(l, slots);
                    let warning = crate::t!(S_THE_RECOVERY_PARTITIONS_OF_WERE_ONLY_PARTLY_WRITTEN, names = names);
                    y = widgets::footnote(c, t, f, y, &warning);
                    y = widgets::footnote(c, t, f, y, why);
                }
                Some(SkywardEnd::Updated(_)) => unreachable!("an update that went in shows how it did"),
                None => {}
            }
            let mut row = Row::new(S_DOWNLOAD_AND_UPDATE);
            if !s.online() {
                row = row.subtitle(WLAN_FIRST);
            }
            let update = widgets::list(c, t, f, y + gap, &[row]);
            out.extend(hits(&update.rows, &[Action::UpdateSkyward]));
            update.bottom
        }
    }
}

/// The WLAN switch; then the network chosen and how joining it goes, and
/// the networks in range, strongest first.
pub fn wlan(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let w = s.wlan.as_ref().expect("the WLAN page shows on a phone whose WLAN is brought up");
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let on = !matches!(w.power, Power::Off | Power::Failed(_));
    let switch = widgets::list(&mut c, t, f, top, &[Row::new(WLAN).trail(Trail::Switch(on))]);
    out.extend(hits(&switch.rows, &[Action::Toggle(Switch::Wlan)]));
    let mut y = switch.bottom;
    if matches!(s.data, Data::Closed | Data::Opening | Data::Failed(_)) {
        let note = S_SAVED_NETWORKS_CONNECT_AFTER_THE_DATA_PARTITION_IS_DECRYPTED;
        y = widgets::footnote(&mut c, t, f, y, note);
    }
    match &w.power {
        Power::Off => {
            let note = S_WHEN_ON_SYSTEM_UPDATES_CAN_DOWNLOAD_OVER_WI_FI;
            y = widgets::footnote(&mut c, t, f, y, note)
        }
        Power::Failed(why) => y = widgets::footnote(&mut c, t, f, y, &crate::t!(S_COULDNT_TURN_ON_WI_FI, why = why)),
        Power::Starting => y = widgets::footnote(&mut c, t, f, y, S_TURNING_ON_WI_FI),
        Power::On => {
            let locked = |s: Security| !matches!(s, Security::Open | Security::Owe);
            // The network chosen stands out in the accent, as the stock
            // page shows the one it is on.
            if let Some(j) = &w.joined {
                let found = w.networks.iter().find(|n| n.ssid == j.ssid);
                let (signal, bands) = match (&j.stage, found) {
                    (Joining::Connected(link), found) => (Some(link.signal), found.map_or(Bands::of(link.frequency), |n| n.bands)),
                    (_, Some(n)) => (Some(n.signal), n.bands),
                    (_, None) => (None, Bands::default()),
                };
                let tag = fmt::band_tag(bands);
                let mut row = Row::new(&j.name).fan(signal.map_or(0, fmt::signal_level)).subtitle(fmt::joining(l, &j.stage));
                row = row.trail(Trail::Network { locked: locked(j.security) });
                if let Some(tag) = &tag {
                    row = row.tag(tag);
                }
                let joined = widgets::list_toned(&mut c, t, f, y + t.m.card_gap, &[row], &Tone::lit(t));
                out.extend(hits(&joined.rows, &[Action::OpenJoined]));
                out.push(Hit { rect: widgets::round_button(t, joined.rows[0]), action: Action::OpenJoined });
                y = joined.bottom;
                if let Joining::Connected(link) = &j.stage {
                    if let Some(note) = link.reach.as_ref().and_then(|r| fmt::reach(l, r)) {
                        y = widgets::footnote(&mut c, t, f, y, &note);
                    }
                    // adbd on the network joined, so a computer reaches the recovery with no cable. Not while a
                    // package is being written: restarting adbd ends the transfer.
                    let busy = !matches!(s.install, Install::Idle | Install::Finished { .. }) || s.updating_skyward();
                    if !busy {
                        let on = matches!(s.adb_wifi, AdbWifi::On);
                        let hint = match &s.adb_wifi {
                            AdbWifi::Off => String::new(),
                            AdbWifi::On => format!("adb connect {}:{ADB_WIFI_PORT}", link.address),
                            AdbWifi::Failed(why) => crate::t!(S_COULDNT_TURN_IT_ON, why = why),
                        };
                        let mut row = Row::new(ADB_OVER_WLAN).trail(Trail::Switch(on));
                        if !hint.is_empty() {
                            row = row.subtitle(&hint);
                        }
                        let switch = widgets::list(&mut c, t, f, y + t.m.card_gap, &[row]);
                        out.extend(hits(&switch.rows, &[Action::Toggle(Switch::AdbWifi)]));
                        y = switch.bottom;
                        if on {
                            y = widgets::footnote(&mut c, t, f, y, ADB_LISTENING);
                        }
                    }
                }
            }
            let others: Vec<(usize, &Network)> =
                w.networks.iter().enumerate().filter(|(_, n)| w.joined.as_ref().is_none_or(|j| j.ssid != n.ssid)).collect();
            let nearby = S_CHOOSE_A_NEARBY_NETWORK;
            let (below, rescan) = widgets::section_action(&mut c, t, f, y, nearby, S_REFRESH);
            out.push(Hit { rect: rescan, action: Action::Rescan });
            y = below;
            if others.is_empty() {
                let looking = S_LOOKING_FOR_NEARBY_NETWORKS;
                y = widgets::footnote(&mut c, t, f, y - t.m.dp(10.0), looking) + t.m.card_gap;
            }
            let tags: Vec<Option<String>> = others.iter().map(|(_, n)| fmt::band_tag(n.bands)).collect();
            let mut rows: Vec<Row> = others
                .iter()
                .zip(&tags)
                .map(|((_, n), tag)| {
                    let row = Row::new(&n.name).fan(fmt::signal_level(n.signal)).trail(Trail::Network { locked: locked(n.security) });
                    match tag {
                        Some(tag) => row.tag(tag),
                        None => row,
                    }
                })
                .collect();
            // The list ends with adding a network by hand, as the stock one
            // does.
            rows.push(Row::new(ADD_NETWORK).tint(t.pal.accent).trail(Trail::None));
            let list = widgets::list(&mut c, t, f, y, &rows);
            let mut picks: Vec<Action> = others.iter().map(|(k, _)| Action::JoinNetwork(*k)).collect();
            picks.push(Action::AddNetwork);
            out.extend(hits(&list.rows, &picks));
            // The round buttons come after the rows they sit on, and a touch
            // finds the last control under it.
            for (rect, (k, _)) in list.rows.iter().zip(&others) {
                out.push(Hit { rect: widgets::round_button(t, *rect), action: Action::NetworkDetails(*k) });
            }
            y = list.bottom;
        }
    }
    Frame::new(t, fit(c, t, y), out, WLAN, top)
}

const ADD_NETWORK: Text = S_ADD_NETWORK;
const ADB_OVER_WLAN: Text = S_ADB_OVER_WI_FI;
const ADB_LISTENING: Text = S_ANYONE_ON_THIS_NETWORK_CAN_CONTROL_THIS_RECOVERY_WITHOUT_A_PASSWORD;
const SHOW_PASSWORD: Text = S_SHOW_PASSWORD;
const CONNECT_KEY: Text = S_CONNECT;
const SECURITY: Text = S_SECURITY;

/// The password of the network chosen: its name, what is typed, a switch
/// to show it, and the keyboard.
pub fn wlan_password(cx: &Ctx) -> Frame {
    let (t, f, s, _l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let network = s.joining.as_ref().expect("the password page is for a network");
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let y = widgets::section_first(&mut c, t, f, top, &network.name);
    let shown = if s.show_password { s.password.clone() } else { "•".repeat(s.password.chars().count()) };
    let (y, caret) = widgets::text_field(&mut c, t, f, y, &shown);
    let show = widgets::list(&mut c, t, f, y + t.m.card_gap, &[Row::new(SHOW_PASSWORD).trail(Trail::Switch(s.show_password))]);
    out.extend(hits(&show.rows, &[Action::Toggle(Switch::ShowPassword)]));
    keyboard_hits(&mut c, t, f, s, CONNECT_KEY, &mut out);
    Frame { caret: Some(caret), ..Frame::new(t, c, out, S_ENTER_PASSWORD, top) }
}

/// A network added by hand: its name, how it is protected, its password
/// where it takes one with the switch that shows it, and the keyboard,
/// which types into the field touched. The protections open over the page
/// from their row, which alone then answers a touch.
pub fn wlan_add(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let m = &t.m;
    let a = s.adding.as_ref().expect("the page adds a network");
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = m.content_top;
    let field = |y: f32| Rect::new(m.margin, y, m.w as f32 - m.margin * 2.0, m.dp(56.0));
    let y = widgets::section_first(&mut c, t, f, top, S_NETWORK_NAME);
    let (below, mut caret) = widgets::text_field(&mut c, t, f, y, &a.name);
    out.push(Hit { rect: field(y), action: Action::TypeInto(Field::Name) });
    // Held down while its list is open, as the stock row stays.
    let tone = if a.choosing { Tone::page(&t.pressed()) } else { Tone::page(t) };
    let protection = [Row::new(SECURITY).trail(Trail::Choice(fmt::security(l, a.security)))];
    let security = widgets::list_toned(&mut c, t, f, below + m.card_gap, &protection, &tone);
    out.extend(hits(&security.rows, &[Action::ChooseSecurity]));
    if a.security.password() {
        let y = widgets::section(&mut c, t, f, security.bottom, S_PASSWORD);
        let shown = if s.show_password { s.password.clone() } else { "•".repeat(s.password.chars().count()) };
        let (below, typed) = widgets::text_field(&mut c, t, f, y, &shown);
        out.push(Hit { rect: field(y), action: Action::TypeInto(Field::Password) });
        if a.field == Field::Password {
            caret = typed;
        }
        let show = widgets::list(&mut c, t, f, below + m.card_gap, &[Row::new(SHOW_PASSWORD).trail(Trail::Switch(s.show_password))]);
        out.extend(hits(&show.rows, &[Action::Toggle(Switch::ShowPassword)]));
    }
    keyboard_hits(&mut c, t, f, s, CONNECT_KEY, &mut out);
    if !a.choosing {
        return Frame { caret: Some(caret), ..Frame::new(t, c, out, ADD_NETWORK, top) };
    }
    // A touch anywhere off the list closes it.
    out.clear();
    out.push(Hit { rect: Rect::new(0.0, 0.0, m.w as f32, m.h as f32), action: Action::CloseChoices });
    let names = ADDABLE.map(|p| fmt::security(l, p));
    let chosen = ADDABLE.iter().position(|&p| p == a.security);
    let held = ADDABLE.iter().position(|&p| s.held == Some(Action::PickSecurity(p)));
    let list = widgets::choices(&mut c, t, f, security.rows[0], &names, chosen, held);
    out.extend(hits(&list, &ADDABLE.map(Action::PickSecurity)));
    Frame::new(t, c, out, ADD_NETWORK, top)
}

/// A network's details, in the stock page's grid: for the one chosen, how
/// joining it goes and what it gave, and forgetting it; for any other, how
/// strong and how protected it is.
pub fn wlan_network(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let network = s.details.as_ref().expect("the details page is of a network");
    let joined = s.wlan.as_ref().and_then(|w| w.joined.as_ref()).filter(|j| j.ssid == network.ssid);
    let y = widgets::section_first(&mut c, t, f, top, &network.name);
    let detail = |icon, title, value: String| Detail { icon, title, value };
    let security = detail(Outline::Shield, SECURITY, fmt::security(l, network.security).to_string());
    let signal = S_SIGNAL_STRENGTH;
    let title = S_NETWORK_DETAILS;
    let Some(j) = joined else {
        let items = [
            detail(Outline::Wifi, S_BANDS, fmt::bands(l, network.bands)),
            detail(Outline::Signal, signal, fmt::signal(l, network.signal).to_string()),
            security,
        ];
        let y = widgets::detail_grid(&mut c, t, f, y, &items);
        return Frame::new(t, fit(c, t, y), out, title, top);
    };
    let mut items = vec![detail(Outline::Info, S_STATUS, fmt::joining(l, &j.stage).to_string())];
    match &j.stage {
        Joining::Connected(link) => {
            if let Some(n) = link.generation {
                items.push(detail(Outline::Wifi, S_WI_FI_STANDARD, fmt::generation(l, n)));
            }
            if let Some(speed) = link.speed {
                items.push(detail(Outline::Speed, S_LINK_SPEED, format!("{speed} Mbps")));
            }
            items.push(detail(Outline::Signal, signal, fmt::signal(l, link.signal).to_string()));
            items.push(security);
            items.push(detail(Outline::Address, S_IP_ADDRESS, link.address.to_string()));
            items.push(detail(Outline::Mask, S_SUBNET_MASK, link.mask.to_string()));
            if let Some(router) = link.router {
                items.push(detail(Outline::Router, S_ROUTER, router.to_string()));
            }
        }
        _ => {
            items.push(detail(Outline::Signal, signal, fmt::signal(l, network.signal).to_string()));
            items.push(security);
        }
    }
    let y = widgets::detail_grid(&mut c, t, f, y, &items);
    let forget = widgets::list(&mut c, t, f, y + t.m.card_gap, &[Row::new(S_FORGET_NETWORK).tint(t.pal.danger).trail(Trail::None)]);
    out.extend(hits(&forget.rows, &[Action::LeaveNetwork]));
    Frame::new(t, fit(c, t, forget.bottom), out, title, top)
}

/// The file manager, every entry of the folder shown, folders first.
pub fn files(cx: &Ctx) -> Frame {
    let (t, f, s, _l) = (cx.t, cx.f, cx.state, cx.state.lang);
    // An entry picked to copy or move goes to the folder shown, from the
    // bar held at the bottom.
    let dock = s.clip.as_ref().map(|clip| {
        let go = match clip.op {
            FileOp::Copy => S_COPY_HERE,
            FileOp::Move => S_MOVE_HERE,
        };
        let (canvas, rects) = widgets::dock_bar(t, f, files::page(t), &[CANCEL, go]);
        Dock { canvas, hits: hits(&rects, &[Action::Unpick, Action::Paste]) }
    });
    let button = Some((widgets::Glyph::Terminal, Action::TerminalHere));
    let button2 = None;
    let root = S_ROOT;
    Frame { dock, button, button2, ..listing(cx, &s.files, root, None, S_NO_FILES, Action::OpenEntry, FILES) }
}

const CANCEL: Text = S_CANCEL;

/// A place's folders and the files in them that can be installed, to
/// choose one. A USB drive's page waits for one to be plugged in, and the
/// drive's files show as soon as it is.
pub fn choose(cx: &Ctx, place: Place) -> Frame {
    let (s, l) = (cx.state, cx.state.lang);
    let medium = match place {
        Place::UsbDrive => &s.usb_drive,
        Place::SdCard => &s.sdcard,
        Place::Storage => &None,
    };
    // What the page says in place of the files: what to put in, or why the
    // medium there cannot be read.
    let (wanted, detail, unreadable_name, hint) = match place {
        Place::UsbDrive => (
            S_NO_USB_DRIVE,
            S_CONNECT_A_USB_DRIVE_THROUGH_OTG_TO_SEE_ITS_FILES_HERE,
            USB_DRIVE_UNREADABLE,
            S_USE_A_USB_DRIVE_FORMATTED_AS_FAT32_OR_EXFAT,
        ),
        _ => (SD_CARD_WANTED, SD_CARD_WANTED_DETAIL, SD_CARD_UNREADABLE, S_USE_AN_SD_CARD_FORMATTED_AS_FAT32_OR_EXFAT),
    };
    let unreadable;
    // Internal storage's page says nothing of a medium: it has its files or
    // none.
    let absent = match (place, medium) {
        (Place::Storage, _) => None,
        (_, None) => Some((wanted, detail)),
        (_, Some(Medium::Unreadable(why))) => {
            unreadable = crate::t!(hint, why = why);
            Some((unreadable_name, unreadable.as_str()))
        }
        (_, Some(Medium::Shown)) => None,
    };
    let empty = S_NOTHING_HERE_TO_INSTALL;
    listing(cx, s.folder(Screen::Choose(place)), place_name(l, place), absent, empty, Action::ChooseEntry, S_CHOOSE_A_FILE)
}

/// A folder laid out as the stock file manager lays one out: the path as
/// capsules from `top`, held under the bar once scrolled there, then the
/// entries in one list on the plain page, each row drawn as it scrolls into
/// view and touched for `action`. In place of the list, `absent` says why
/// nothing of the folder can be shown, and `empty` that it holds nothing.
fn listing(
    cx: &Ctx,
    folder: &Folder,
    top: &str,
    absent: Option<(&str, &str)>,
    empty: &str,
    action: fn(usize) -> Action,
    title: &'static str,
) -> Frame {
    let (t, f) = (cx.t, cx.f);
    let bg = files::page(t);
    let mut c = sheet(cx, t.m.h, bg);
    let rows = match (absent, &folder.entries) {
        (Some((note, why)), _) => {
            files::note(&mut c, t, f, note, Some(why));
            None
        }
        (None, Err(why)) => {
            files::note(&mut c, t, f, S_THIS_FOLDER_CANT_BE_READ, Some(why));
            None
        }
        (None, Ok(entries)) if entries.is_empty() => {
            files::note(&mut c, t, f, empty, None);
            None
        }
        (None, Ok(entries)) => Some(files::rows(t, f, entries, action)),
    };
    let pinned = Some(files::strip(t, f, top, &folder.path));
    Frame { bg, rows, pinned, ..Frame::new(t, c, Vec::new(), title, t.m.dp(files::STRIP_TOP_DP)) }
}

/// Row `i` of the page's long list over the whole of `c`, sized for it,
/// drawn as the row comes into view.
pub fn row(screen: Screen, cx: &Ctx, i: usize, c: &mut Canvas) {
    match screen {
        Screen::Files | Screen::Choose(_) => {
            let folder = cx.state.folder(screen);
            let entries = folder.entries.as_ref().expect("a folder listed draws its rows");
            files::row(c, cx.t, cx.f, cx.state.lang, &entries[i], folder.listed, cx.state.time_zone);
        }
        Screen::Installing => widgets::output_row(c, cx.t, cx.f, &cx.state.script_output, i),
        Screen::PartWork => {
            let work = cx.state.part_work.as_ref().expect("the page of partition work shows work");
            widgets::output_row(c, cx.t, cx.f, &work.output, i)
        }
        other => unreachable!("{other:?} draws no list row by row"),
    }
}

/// One entry of a folder, a file or a folder: what it is, and what can be
/// done with it. Deleting asks first, with a swipe.
pub fn file_info(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let e = s.file.as_ref().expect("the file page shows an entry");
    let mut c = long_page(cx);
    let top = t.m.content_top;
    let folder = matches!(e.kind, FileKind::Folder { .. });
    let mut facts = vec![
        (NAME, e.name.clone()),
        (S_LOCATION, Path::new(&s.files.path).join(&e.name).to_string_lossy().into_owned()),
    ];
    let holds = S_HOLDS;
    match &e.kind {
        FileKind::Link { target, .. } => facts.push((S_POINTS_TO, target.clone())),
        FileKind::Folder { items: Some(n) } => {
            let items = match (l, n) {
                (Lang::En, 1) => "1 item".to_string(),
                (_, n) => crate::t!(S_ITEMS, n = n),
            };
            facts.push((holds, items))
        }
        FileKind::Folder { items: None } => facts.push((holds, UNREADABLE.to_string())),
        FileKind::File | FileKind::Special => {
            let (size, bytes) = (fmt::file_size(e.size), e.size);
            facts.push((S_SIZE, crate::t!(S_BYTES, size = size, bytes = bytes)))
        }
    }
    facts.push((S_MODIFIED, fmt::datetime(l, e.modified, s.time_zone)));
    let (mode, uid, gid) = (fmt::mode(e.mode), e.uid, e.gid);
    facts.push((PERMISSIONS, crate::t!(S_OWNER_GROUP, mode = mode, uid = uid, gid = gid)));
    let y = widgets::info_card(&mut c, t, f, top, &facts);
    let mut rows = Vec::new();
    let mut acts = Vec::new();
    // Images are flashed; packages and zips that install themselves are
    // installed.
    if installs(e) {
        let image = Path::new(&e.name).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("img"));
        rows.push(Row::new(if image { S_FLASH } else { S_INSTALL_2 }));
        acts.push(Action::InstallEntry);
    }
    if e.kind == FileKind::File {
        rows.push(Row::new(S_EDIT));
        acts.push(Action::EditEntry);
    }
    rows.extend([Row::new(S_COPY), Row::new(S_MOVE), Row::new(RENAME)]);
    acts.extend([Action::PickEntry(FileOp::Copy), Action::PickEntry(FileOp::Move), Action::RenameEntry]);
    // A link's own permissions mean nothing; its target is changed where
    // it is.
    if !matches!(e.kind, FileKind::Link { .. }) {
        rows.push(Row::new(CHANGE_PERMISSIONS));
        acts.push(Action::OpenPermissions);
    }
    rows.push(Row::new(S_DELETE).tint(t.pal.danger).trail(Trail::None));
    acts.push(Action::AskDelete);
    let actions = widgets::list(&mut c, t, f, y + t.m.card_gap, &rows);
    let mut out = hits(&actions.rows, &acts);
    let mut bottom = actions.bottom;
    if s.asking_delete {
        let name = &e.name;
        let what = match folder {
            true => crate::t!(S_DELETE_AND_EVERYTHING_IN_IT_IT_CANT_BE_BROUGHT_BACK, name = name),
            false => crate::t!(S_DELETE_IT_CANT_BE_BROUGHT_BACK, name = name),
        };
        let y = widgets::footnote(&mut c, t, f, actions.bottom, &what);
        let (label, danger) = confirm_label(Confirm::DeleteEntry, cx.state);
        let track = widgets::confirm(&mut c, t, f, y + t.m.dp(24.0), label, danger, s.confirm);
        out.push(Hit { rect: track, action: Action::Confirm(Confirm::DeleteEntry) });
        bottom = track.y + track.h;
    }
    let title = if folder { S_FOLDER_INFO } else { S_FILE_INFO };
    Frame::new(t, fit(c, t, bottom), out, title, top)
}

const NAME: Text = S_NAME;
const PERMISSIONS: Text = S_PERMISSIONS;
const CHANGE_PERMISSIONS: Text = S_CHANGE_PERMISSIONS;
const RENAME: Text = S_RENAME;

/// The permission bits of the entry the file page shows, a switch each that
/// takes effect at once: reading, writing and running for its owner, its
/// group and everyone else, then the set-ID and sticky bits.
pub fn permissions(cx: &Ctx) -> Frame {
    let (t, f, s, _l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let e = s.file.as_ref().expect("the permissions page shows an entry");
    let mut c = long_page(cx);
    let top = t.m.content_top;
    let mode = e.mode & 0o7777;
    let bits = fmt::mode(e.mode);
    let facts = [(NAME, e.name.clone()), (PERMISSIONS, crate::t!(S_TEXT_8, bits = bits, mode = mode))];
    let mut y = widgets::info_card(&mut c, t, f, top, &facts);
    let mut out = Vec::new();
    let classes = [(S_OWNER, 6), (S_GROUP, 3), (S_OTHERS, 0)];
    let (read, write, run) = (S_READ_2, S_WRITE, S_EXECUTE);
    let groups = classes.map(|(title, shift)| (title, [(read, 4 << shift), (write, 2 << shift), (run, 1 << shift)]));
    let special = (S_SPECIAL, [("SUID", 0o4000), ("SGID", 0o2000), (S_STICKY, 0o1000)]);
    for (title, bits) in groups.into_iter().chain([special]) {
        y = widgets::section(&mut c, t, f, y, title);
        let rows = bits.map(|(name, bit)| Row::new(name).trail(Trail::Switch(mode & bit != 0)));
        let list = widgets::list(&mut c, t, f, y, &rows);
        out.extend(hits(&list.rows, &bits.map(|(_, bit)| Action::ModeBit(bit))));
        y = list.bottom;
    }
    Frame::new(t, fit(c, t, y), out, CHANGE_PERMISSIONS, top)
}

/// A new name for the entry the file page shows, typed on the keyboard.
pub fn rename(cx: &Ctx) -> Frame {
    let (t, f, s, _l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let (y, caret) = widgets::text_field(&mut c, t, f, top, &s.new_name);
    widgets::footnote(&mut c, t, f, y, S_A_NAME_CANT_HOLD_TAP_DONE_TO_SAVE_IT);
    keyboard_hits(&mut c, t, f, s, DONE, &mut out);
    Frame { caret: Some(caret), ..Frame::new(t, c, out, RENAME, top) }
}

/// The folder an entry goes to already holds its name: what to do.
pub fn clash(cx: &Ctx) -> Frame {
    let (t, f, s, _l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let (clip, into, _) = s.clash.as_ref().expect("the clash page shows a clash");
    let name = Path::new(&clip.path).file_name().map_or_else(|| clip.path.clone(), |n| n.to_string_lossy().into_owned());
    let verb = match clip.op {
        FileOp::Copy => S_COPY_2,
        FileOp::Move => S_MOVE_2,
    };
    let mut c = page(cx);
    let top = t.m.content_top;
    let there = crate::t!(S_ALREADY_HOLDS_AN_ENTRY_NAMED, into = into, name = name);
    let y = widgets::paragraph(&mut c, t, f, top, &there);
    let skip = crate::t!(S_KEEPS_THE_ENTRY_THERE_AND_DOESNT_THIS_ONE, verb = verb);
    let choices = widgets::list(&mut c, t, f, y + t.m.dp(12.0), &[
        Row::new(S_REPLACE).subtitle(S_WRITES_OVER_THE_ENTRY_THERE),
        Row::new(S_SKIP).subtitle(&skip),
        Row::new(S_KEEP_BOTH).subtitle(S_GIVES_THIS_ONE_A_NEW_NAME),
    ]);
    let out = hits(&choices.rows, &[
        Action::Resolve(ClashChoice::Replace),
        Action::Resolve(ClashChoice::Skip),
        Action::Resolve(ClashChoice::KeepBoth),
    ]);
    Frame::new(t, c, out, S_NAME_TAKEN, top)
}

/// How work on files is told of: the Chinese verb its sentences build on,
/// in simplified and in traditional characters, then English's own words
/// for what is done and for doing it; the page's titles while it goes on
/// and once it ended short; and what stopping it may have left.
struct FileWords {
    verb: [&'static str; 2],
    done: &'static str,
    base: &'static str,
    titles: (Text, Text),
    stopped: Text,
}

fn file_words(task: &FileTask) -> FileWords {
    match task {
        FileTask::Copy { .. } => FileWords {
            verb: ["copy", "copy"],
            done: "copied",
            base: "copy",
            titles: (S_COPYING, S_COPY_INCOMPLETE),
            stopped: S_STOPPED_PART_OF_IT_MAY_HAVE_BEEN_COPIED_INTO_THE_FOLDER,
        },
        FileTask::Move { .. } => FileWords {
            verb: ["move", "move"],
            done: "moved",
            base: "move",
            titles: (S_MOVING, S_MOVE_INCOMPLETE),
            stopped: S_STOPPED_PART_OF_IT_MAY_HAVE_BEEN_MOVED_INTO_THE_FOLDER,
        },
        FileTask::Delete { .. } => FileWords {
            verb: ["Delete", "Delete"],
            done: "deleted",
            base: "delete",
            titles: (S_DELETING, S_DELETE_INCOMPLETE),
            stopped: S_STOPPED_PART_OF_IT_MAY_HAVE_BEEN_DELETED,
        },
    }
}

/// Copying, moving or deleting: how far it has come and a way to stop it,
/// then, if it did not finish, why.
pub fn file_job(cx: &Ctx) -> Frame {
    let (t, f, s, _l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let job = s.job.as_ref().expect("the job page shows work on files");
    let path = match &job.task {
        FileTask::Copy { path, .. } | FileTask::Move { path, .. } | FileTask::Delete { path } => path,
    };
    let words = file_words(&job.task);
    let ([_hans, _hant], done_en, base) = (words.verb, words.done, words.base);
    let name = Path::new(path).file_name().map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
    let share = |(done, total): (u64, u64)| if total == 0 { 0.0 } else { done as f32 / total as f32 };
    // The bar stays while the work measures, so that the button under it
    // does not move. Deleting, and copying entries that hold no bytes, is
    // counted in entries.
    let (detail, done) = match (job.progress, &job.task) {
        (None, _) => (GETTING_READY.to_string(), 0.0),
        (Some((bytes, _)), FileTask::Copy { .. } | FileTask::Move { .. }) if bytes.1 > 0 => {
            let (did, total) = (fmt::file_size(bytes.0), fmt::file_size(bytes.1));
            (crate::t!(S_OF_2, did = did, total = total, done_en = done_en), share(bytes))
        }
        (Some((_, entries)), _) => {
            let (did, total) = entries;
            (crate::t!(S_OF_ITEMS, did = did, total = total, done_en = done_en), share(entries))
        }
    };
    let mut c = page(cx);
    let top = t.m.content_top;
    let y = widgets::progress_card(&mut c, t, f, top, &name, &detail, Some(done));
    let mut out = Vec::new();
    match &job.outcome {
        None => {
            let label = if job.stopping { STOPPING } else { STOP };
            let stop = widgets::list(&mut c, t, f, y + t.m.card_gap, &[Row::new(label).trail(Trail::None)]);
            if !job.stopping {
                out.extend(hits(&stop.rows, &[Action::StopJob]));
            }
        }
        Some(end) => {
            let why = match end {
                JobEnd::Stopped => words.stopped.to_string(),
                JobEnd::Failed(why) => crate::t!(S_COULDNT_IT, base = base, why = why),
                JobEnd::Done => unreachable!("work done leaves its page"),
            };
            let y = widgets::footnote(&mut c, t, f, y, &why);
            let finish = widgets::list(&mut c, t, f, y + t.m.card_gap, &[Row::new(DONE).trail(Trail::None)]);
            out.extend(hits(&finish.rows, &[Action::Back]));
        }
    }
    let title = if job.outcome.is_none() { words.titles.0 } else { words.titles.1 };
    Frame { back: job.outcome.is_some(), ..Frame::new(t, c, out, title, top) }
}

/// A shell on a terminal: the screen it writes, the keys a phone keyboard
/// lacks, and the keyboard, every key of which goes to the shell.
pub fn terminal(cx: &Ctx) -> Frame {
    let (t, f, s) = (cx.t, cx.f, cx.state);
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    keyboard_hits(&mut c, t, f, s, S_ENTER, &mut out);
    let l = term::layout(t, f, top);
    term::draw(&mut c, t, f, &l, cx.term);
    let held = cx.term.is_some_and(|term| term.ctrl);
    let keys = term::keys(&mut c, t, f, &l, &term::TERM_KEYS, held);
    out.extend(keys.into_iter().map(|(rect, key)| Hit { rect, action: Action::TermKey(key) }));
    Frame { scrollback: Some((l.card, l.pitch)), ..Frame::new(t, c, out, TERMINAL, top) }
}

/// A text file being edited: where it is, its lines wrapped on a card, the
/// keys a phone keyboard lacks for moving the cursor, and the keyboard.
/// Once it is changed, the round button saves it.
pub fn editor(cx: &Ctx) -> Frame {
    let (t, f, s) = (cx.t, cx.f, cx.state);
    let ed = s.editor.as_ref().expect("the editor page shows a file");
    let mut c = page(cx);
    let mut out = Vec::new();
    keyboard_hits(&mut c, t, f, s, S_RETURN, &mut out);
    editor::caption(&mut c, t, f, &ed.path);
    let l = editor::layout(t, f);
    let caret = editor::draw(&mut c, t, f, &l, ed);
    let keys = term::keys(&mut c, t, f, &l, &editor::EDITOR_KEYS, false);
    out.extend(keys.into_iter().map(|(rect, key)| Hit { rect, action: Action::TermKey(key) }));
    let button = ed.changed.then_some((widgets::Glyph::Done, Action::SaveText));
    Frame { scrollback: Some((l.card, l.pitch)), caret, button, ..Frame::new(t, c, out, S_EDIT, t.m.content_top) }
}

/// The time zone the status bar's clock and the times on the pages keep.
pub fn time_zone(cx: &Ctx) -> Frame {
    let (t, f, s, _l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = long_page(cx);
    let top = t.m.content_top;
    let rows: Vec<Row> = TIME_ZONES
        .iter()
        .enumerate()
        .map(|(k, &(name, cities, _))| Row::new(name).subtitle(cities).trail(Trail::Check(k == s.time_zone)))
        .collect();
    let list = widgets::list(&mut c, t, f, top, &rows);
    let picks: Vec<Action> = (0..TIME_ZONES.len()).map(Action::TimeZone).collect();
    let out = hits(&list.rows, &picks);
    let note = S_THE_STATUS_BAR_AND_THE_PAGES_TELL_TIMES_IN_THE_ZONE_CHOSEN;
    let bottom = widgets::footnote(&mut c, t, f, list.bottom, note);
    Frame::new(t, fit(c, t, bottom), out, TIME_ZONE, top)
}

/// Saving the logs, as a bug report asks for them.
pub fn logs(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let y = widgets::section_first(&mut c, t, f, top, S_LOGS);
    let parts = widgets::list(&mut c, t, f, y, &[
        Row::new(S_RECOVERY_LOG)
            .subtitle(S_WHAT_THE_PAGES_INSTALLS_AND_MOUNTS_DID)
            .trail(Trail::Check(s.logs[0])),
        Row::new(S_KERNEL_LOG)
            .subtitle(S_THE_KERNELS_MESSAGES_SINCE_THIS_START)
            .trail(Trail::Check(s.logs[1])),
    ]);
    out.extend(hits(&parts.rows, &[Action::LogPart(0), Action::LogPart(1)]));
    let y = widgets::section(&mut c, t, f, parts.bottom, S_SAVE_TO);
    let storage = match cx.state.data.storage_open() {
        true => S_THE_SKYWARD_LOGS_FOLDER,
        false => DECRYPT_NEEDED,
    };
    let places = widgets::list(&mut c, t, f, y, &[
        Row::new(S_TEMPORARY_FOLDER)
            .subtitle(S_TMP_SKYWARD_LOGS_WHICH_ADB_PULL_COPIES_TO_A_COMPUTER)
            .trail(Trail::Check(s.logs_to == 0)),
        Row::new(place_name(l, Place::Storage)).subtitle(storage).trail(Trail::Check(s.logs_to == 1)),
    ]);
    out.extend(hits(&places.rows, &[Action::LogsTo(0), Action::LogsTo(1)]));
    let note = S_EACH_EXPORT_GOES_IN_A_FOLDER_NAMED_BY_WHEN_IT_WAS_MADE_A_REBOOT_EMPTIES_THE_TEMPORARY_FOLDER;
    let y = widgets::footnote(&mut c, t, f, places.bottom, note);
    let export = widgets::button(&mut c, t, f, y + t.m.dp(24.0), S_EXPORT);
    out.push(Hit { rect: export, action: Action::ExportLogs });
    Frame::new(t, c, out, LOGS, top)
}

const LICENSE_ORIGINAL: &str = "Skyward\nCopyright (C) 2026 Kirk Lin\n\n\
This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.\n\n\
This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.\n\n\
You should have received a copy of the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.";

const LICENSE_TERMS: Text = S_LICENSE;

/// Skyward's own licence.
pub fn license(cx: &Ctx) -> Frame {
    let (t, f) = (cx.t, cx.f);
    let mut c = long_page(cx);
    let top = t.m.content_top;
    let terms = S_GNU_GENERAL_PUBLIC_LICENSE_VERSION_3_OR_LATER_GPL_3_0_OR_LATER;
    let y = widgets::info_card(&mut c, t, f, top, &[(LICENSE_TERMS, terms.to_string())]);
    let bottom = widgets::paragraph(&mut c, t, f, y + t.m.dp(4.0), LICENSE_ORIGINAL);
    Frame::new(t, fit(c, t, bottom), Vec::new(), LICENSE, top)
}

/// The monospaced typeface's licence, which names the copyright holders on
/// its first lines; the image carries the same file beside the font.
const MONO_LICENSE: &str = include_str!("../../../resources/fonts/KirkMono-OFL.txt");

/// The typefaces, as they name themselves, and the terms each is used under.
pub fn font_license(cx: &Ctx) -> Frame {
    let (t, f, _l) = (cx.t, cx.f, cx.state.lang);
    let mut c = long_page(cx);
    let top = t.m.content_top;
    let (typeface, version, copyright) = (S_TYPEFACE, S_VERSION, S_COPYRIGHT);
    let y = widgets::section_first(&mut c, t, f, top, S_INTERFACE_TEXT);
    let y = widgets::info_card(&mut c, t, f, y, &[
        (typeface, f.info.family.clone()),
        (version, f.info.version.clone()),
        (copyright, f.info.copyright.clone().expect("MiSans names its copyright holder")),
    ]);
    let misans = S_THE_INTERFACE_TEXT_IS_SET_IN_XIAOMIS_MISANS_USED_FREE_OF_CHARGE_UNDER_THE_MISANS_FONT_INTELLECTUAL_PROPERTY_LICENSE_AGREEMENT_THE_FONT_FILES_ARE_UNCHANGED_AND_COME_WITH_SKYWARD_ALONE_NEVER_ON_THEIR_OWN_THE_AGREEMENT_IN_FULL_IS_ON_THE_MISANS_WEBSITE;
    let y = widgets::paragraph(&mut c, t, f, y + t.m.dp(4.0), misans);
    let y = widgets::section(&mut c, t, f, y, S_TERMINAL_TEXT);
    let holders: Vec<&str> = MONO_LICENSE.lines().take_while(|l| l.starts_with("Copyright")).collect();
    let y = widgets::info_card(&mut c, t, f, y, &[
        (typeface, f.mono_info.family.clone()),
        (version, f.mono_info.version.clone()),
        (copyright, holders.join("\n")),
        (LICENSE_TERMS, S_SIL_OPEN_FONT_LICENSE_VERSION_1_1_OFL_1_1.to_string()),
    ]);
    let mono = S_THE_TERMINAL_TEXT_IS_SET_IN_THE_MONOSPACED_KIRK_MONO_ITS_LICENSE_LETS_THE_FONT_BE_USED_CHANGED_AND_PASSED_ON_FREELY_BUT_NOT_SOLD_ON_ITS_OWN_THE_LICENSE_IN_FULL_IS_AT_SKYWARD_KIRKMONO_OFL_TXT_AND_ON_OPENFONTLICENSE_ORG;
    let bottom = widgets::paragraph(&mut c, t, f, y + t.m.dp(4.0), mono);
    Frame::new(t, fit(c, t, bottom), Vec::new(), FONTS, top)
}

const BOOT_SLOT: Text = S_BOOT_SLOT;

/// Which slot the phone starts from next, as the boot control holds it,
/// and what it holds of each; picking the other and swiping switches.
/// While an update is in flight the slot stays as the update set it.
pub fn slot(cx: &Ctx) -> Frame {
    let (t, f, s, i, l) = (cx.t, cx.f, cx.state, cx.info, cx.state.lang);
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let title = BOOT_SLOT;
    let Some(current) = i.slot else {
        widgets::list(&mut c, t, f, top, &[Row::new(S_THIS_PHONE_HAS_NO_A_B_SLOTS).trail(Trail::None)]);
        return Frame::new(t, c, out, title, top);
    };
    let slots = match &s.boot_slots {
        Some(Ok(slots)) => *slots,
        Some(Err(why)) => {
            let list = widgets::list(&mut c, t, f, top, &[Row::new(S_COULDNT_READ_THE_BOOT_SLOTS).trail(Trail::None)]);
            widgets::footnote(&mut c, t, f, list.bottom, why);
            return Frame::new(t, c, out, title, top);
        }
        None => return Frame::new(t, c, out, title, top),
    };
    let chosen = s.next_slot.unwrap_or(slots.active);
    let y = widgets::section_first(&mut c, t, f, top, S_NEXT_BOOT);
    let titles = [Slot::A, Slot::B].map(|slot| {
        let slot = slot.letter();
        crate::t!(S_SLOT, slot = slot)
    });
    let subs = [Slot::A, Slot::B].map(|slot| {
        let k = slot.number() as usize;
        let which = if slot == current { S_THE_SYSTEM_RUNNING_NOW } else { S_THE_OTHER_SLOT };
        match (slots.bootable[k], slots.successful[k]) {
            (false, _) => crate::t!(S_MARKED_UNBOOTABLE, which = which),
            (true, false) => crate::t!(S_NOT_YET_FULLY_STARTED, which = which),
            (true, true) => which.to_string(),
        }
    });
    let rows: Vec<Row> =
        [Slot::A, Slot::B].iter().enumerate().map(|(k, &slot)| Row::new(&titles[k]).subtitle(&subs[k]).trail(Trail::Check(slot == chosen))).collect();
    let list = widgets::list(&mut c, t, f, y, &rows);
    out.extend(hits(&list.rows, &[Slot::A, Slot::B].map(Action::PickSlot)));
    if let Some(note) = fmt::snapshot_slot_note(l, &i.snapshot) {
        widgets::footnote(&mut c, t, f, list.bottom, note);
        return Frame::new(t, c, out, title, top);
    }
    let note = S_IF_THE_OTHER_SLOT_HOLDS_NO_WHOLE_SYSTEM_THE_PHONE_WONT_START_AFTER_SWITCHING;
    let y = widgets::footnote(&mut c, t, f, list.bottom, note);
    let (label, danger) = confirm_label(Confirm::Slot, cx.state);
    let track = widgets::confirm(&mut c, t, f, y + t.m.dp(24.0), label, danger, s.confirm);
    out.push(Hit { rect: track, action: Action::Confirm(Confirm::Slot) });
    Frame::new(t, c, out, title, top)
}

const MAIN_SYSTEM: Text = S_MAIN_SYSTEM;
const SECOND_SYSTEM: Text = S_SECOND_SYSTEM;

/// What the home page and the systems page say once the second system's
/// start stopped.
const SECOND_FAILED: Text = S_SECOND_FAILED;

/// The two systems as big cards one over the other, laid out as the boot
/// slot page lays out its slots: the main system, in the phone's own
/// partitions, and the second, whose files lie in the data partition, or
/// the place to install one, the one that starts next checked. Picking the
/// other and swiping writes its boot images into the slot that starts
/// next, and restarts into it. Under them, the second system, and removing
/// it.
pub fn systems(cx: &Ctx) -> Frame {
    let (t, f, s, i, l) = (cx.t, cx.f, cx.state, cx.info, cx.state.lang);
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let title = DUAL_BOOT;
    // While the boot partitions are written, or the second system removed,
    // the page only says so: of a switch the system asked for, perhaps
    // before the systems are read.
    if let Some(work) = s.systems_work {
        let (heading, detail) = match work {
            SystemsWork::Switching(to) => {
                let name = match (&s.systems, to) {
                    (Some(Ok(systems)), Who::Installed) => Some(systems.installed.name.as_str()),
                    (Some(Ok(systems)), Who::Other) => systems.other.as_ref().map(|o| o.name.as_str()),
                    _ => None,
                };
                let heading = match name {
                    Some(name) => crate::t!(S_SWITCHING_TO, name = name),
                    None => S_SWITCHING_SYSTEMS.to_string(),
                };
                (heading, S_WRITING_ITS_BOOT_IMAGES)
            }
            SystemsWork::Removing => (
                S_REMOVING_THE_SECOND_SYSTEM.to_string(),
                S_GIVING_ITS_ROOM_BACK_TO_THE_DATA_PARTITION,
            ),
        };
        let y = widgets::progress_card(&mut c, t, f, top, &heading, detail, None);
        let y = widgets::footnote(&mut c, t, f, y, S_DONT_POWER_OFF_UNTIL_ITS_DONE);
        return Frame::new(t, fit(c, t, y), out, title, top);
    }
    let systems = match &s.systems {
        Some(Ok(systems)) => systems,
        Some(Err(why)) => {
            let list = widgets::list(&mut c, t, f, top, &[Row::new(S_COULDNT_READ_THE_SYSTEMS).trail(Trail::None)]);
            let y = widgets::footnote(&mut c, t, f, list.bottom, why);
            return Frame::new(t, fit(c, t, y), out, title, top);
        }
        None => return Frame::new(t, fit(c, t, top), out, title, top),
    };
    let starts = starting(systems);
    let chosen = s.system_pick.unwrap_or(starts);
    let second_role = SECOND_SYSTEM;
    let main = SystemCard {
        role: MAIN_SYSTEM,
        name: &systems.installed.name,
        detail: systems.installed.version.as_deref().unwrap_or_default(),
        mark: SystemMark::Check(chosen == Who::Installed),
    };
    let second = match &systems.other {
        Some(other) => SystemCard { role: second_role, name: &other.name, detail: &other.version, mark: SystemMark::Check(chosen == Who::Other) },
        None => SystemCard {
            role: second_role,
            name: S_INSTALL_A_SYSTEM,
            detail: S_CHOOSE_THE_SECOND_SYSTEMS_PACKAGE,
            mark: SystemMark::Opens,
        },
    };
    let y = widgets::section_first(&mut c, t, f, top, S_NEXT_BOOT);
    let shown = widgets::system_cards(&mut c, t, f, y, &[main, second]);
    let second_opens = match systems.other {
        Some(_) => Action::PickSystem(Who::Other),
        None => Action::Open(Screen::Install),
    };
    out.extend(hits(&shown.rows, &[Action::PickSystem(Who::Installed), second_opens]));
    let mut y = shown.bottom + t.m.dp(12.0);
    if let Some(why) = &s.system_notice {
        y = widgets::info_card(&mut c, t, f, y, &[(SECOND_FAILED, why.clone())]);
    }
    if chosen != starts {
        match (chosen, fmt::snapshot_system_note(l, &i.snapshot)) {
            (Who::Other, Some(note)) => y = widgets::footnote(&mut c, t, f, y, note),
            _ => {
                let (label, danger) = confirm_label(Confirm::SwitchSystem, s);
                let track = widgets::confirm(&mut c, t, f, y + t.m.dp(12.0), label, danger, s.confirm);
                out.push(Hit { rect: track, action: Action::Confirm(Confirm::SwitchSystem) });
                y = track.y + track.h;
            }
        }
    }
    let what = S_TAP_A_CARD_TO_PICK_A_SYSTEM_SWITCHING_REWRITES_ONLY_THE_BOOT_AND_INIT_BOOT_PARTITIONS_OF_THE_SLOT_THAT_STARTS_NEXT_AND_LEAVES_THE_MAIN_SYSTEMS_OTHER_PARTITIONS_AS_THEY_ARE_THE_SECOND_SYSTEMS_FILES_ARE_KEPT_IN_THE_DATA_PARTITION;
    y = widgets::footnote(&mut c, t, f, y, what);
    if systems.other.is_some() && systems.changed {
        let note = S_THE_MAIN_SYSTEMS_VENDOR_BOOT_OR_DTBO_PARTITION_HAS_CHANGED_SINCE_THE_SECOND_SYSTEM_WAS_INSTALLED_SO_THE_SECOND_SYSTEM_MAY_NOT_START_PROPERLY;
        y = widgets::footnote(&mut c, t, f, y, note);
    }
    if let Some(other) = &systems.other {
        y = widgets::section(&mut c, t, f, y, second_role);
        let size = fmt::bytes(other.size);
        let facts = widgets::list(&mut c, t, f, y, &[Row::new(S_SPACE_USED).trail(Trail::Text(&size))]);
        let mounted = matches!(s.data, Data::Open | Data::Locked { .. });
        let remove = Row::new(S_REMOVE_THE_SECOND_SYSTEM).tint(t.pal.danger);
        let (remove, act) = match mounted {
            true => (remove.trail(Trail::None), Action::AskDelete),
            false => (remove.subtitle(DECRYPT_NEEDED), Action::Open(Screen::Decrypt)),
        };
        let removing = widgets::list(&mut c, t, f, facts.bottom + t.m.card_gap, &[remove]);
        out.extend(hits(&removing.rows, &[act]));
        y = removing.bottom;
        if s.asking_delete && mounted {
            let ask = match starts {
                Who::Other => S_THE_PHONE_GOES_BACK_TO_THE_MAIN_SYSTEM_FIRST_THEN_THE_SECOND_SYSTEM_AND_ALL_ITS_DATA_ARE_REMOVED_FOR_GOOD,
                Who::Installed => S_REMOVE_THE_SECOND_SYSTEM_AND_ALL_ITS_DATA_IT_CANT_BE_BROUGHT_BACK,
            };
            let y2 = widgets::footnote(&mut c, t, f, y, ask);
            let (label, danger) = confirm_label(Confirm::RemoveSystem, s);
            let track = widgets::confirm(&mut c, t, f, y2 + t.m.dp(24.0), label, danger, s.confirm);
            out.push(Hit { rect: track, action: Action::Confirm(Confirm::RemoveSystem) });
            y = track.y + track.h;
        }
    }
    Frame::new(t, fit(c, t, y), out, title, top)
}

/// The phone's own fastbootd serving the computer: whether it runs, how far
/// the computer has the port, and where to go from here, chosen with the
/// keys and lit where they are.
pub fn fastboot(cx: &Ctx) -> Frame {
    let (t, f, s, i, _l) = (cx.t, cx.f, cx.state, cx.info, cx.state.lang);
    let mut c = page(cx);
    let top = t.m.content_top;
    let running = if s.fastboot.running { S_RUNNING } else { S_NOT_RUNNING };
    let link = match s.fastboot.link {
        UsbLink::Connected => S_COMPUTER_CONNECTED,
        UsbLink::Connecting => S_CONNECTING,
        UsbLink::Unplugged => S_NO_COMPUTER,
    };
    let slot = i.slot.map(|s| s.letter());
    let mut facts = vec![
        Row::new("fastbootd").trail(Trail::Text(running)),
        Row::new("USB").trail(Trail::Text(link)),
        Row::new(S_CODENAME).trail(Trail::Text(&i.codename)),
    ];
    if let Some(letter) = slot {
        facts.push(Row::new(CURRENT_SLOT).trail(Trail::Text(letter)));
    }
    let y = widgets::section_first(&mut c, t, f, top, S_STATUS);
    let state = widgets::list(&mut c, t, f, y, &facts);
    let note = S_FASTBOOT_COMMANDS_ON_THE_COMPUTER_CAN_FLASH_PARTITIONS_THE_LOGICAL_ONES_IN_SUPER_AMONG_THEM;
    let y = widgets::footnote(&mut c, t, f, state.bottom, note);
    let y = widgets::section(&mut c, t, f, y, S_LEAVE_FASTBOOT);
    let rows: Vec<Row> = FASTBOOT_CHOICES.iter().map(|&(title, _)| Row::new(title).trail(Trail::None)).collect();
    let choices = widgets::list(&mut c, t, f, y, &rows);
    let actions: Vec<Action> = FASTBOOT_CHOICES.iter().map(|&(_, action)| action).collect();
    let out = hits(&choices.rows, &actions);
    widgets::footnote(&mut c, t, f, choices.bottom, S_CHOOSE_WITH_THE_VOLUME_KEYS_AND_PRESS_THE_POWER_KEY_TO_CONFIRM);
    Frame { back: false, ..Frame::new(t, c, out, "Fastboot", top) }
}

/// Where to go once what the system asked for has stopped or cannot be
/// done, in the order the pages list them.
const AFTERWARDS: [(Text, Action); 3] = [
    (S_REBOOT_TO_SYSTEM, Action::RebootTo(Target::System)),
    (S_ENTER_RECOVERY, Action::Dismiss),
    (S_POWER_OFF, Action::RebootTo(Target::PowerOff)),
];

/// What work the system asked for says while it goes on of how the phone
/// leaves once it is done.
const POWERS_OFF_WHEN_DONE: Text = S_POWERS_OFF_ON_ITS_OWN_ONCE_DONE;
const REBOOTS_WHEN_DONE: Text = S_REBOOTS_ON_ITS_OWN_ONCE_DONE;

/// The list of [`AFTERWARDS`] at `y`, and where it ends. Work the user
/// chose in the recovery goes back to its home page, rather than into it.
fn afterwards(c: &mut Canvas, t: &Theme, f: &Fonts, _l: Lang, y: f32, asked: bool, out: &mut Vec<Hit>) -> f32 {
    let rows: Vec<Row> = AFTERWARDS
        .iter()
        .map(|&(title, action)| Row::new(if action == Action::Dismiss && !asked { S_BACK_TO_HOME } else { title }).trail(Trail::None))
        .collect();
    let choices = widgets::list(c, t, f, y, &rows);
    out.extend(hits(&choices.rows, &AFTERWARDS.map(|(_, action)| action)));
    choices.bottom
}

/// Erasing: each part and how far it has come, and once erasing has
/// stopped, why. What the system asked for leaves on its own once it is
/// done, and offers where to go only if it stops; what the user chose
/// offers where to go once it ends either way.
pub fn erase(cx: &Ctx) -> Frame {
    use std::cmp::Ordering;
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let e = &s.erase;
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let failed = matches!(e.outcome, Some(Err(_)));
    // Only a part that went wrong was under way; whatever else stopped
    // erasing stopped it before any part began.
    let begun = !failed || matches!(e.outcome, Some(Err(EraseStop::Failed(..))));
    let wiped = S_WIPED;
    let heading = match &e.outcome {
        None => WIPING,
        Some(Ok(())) => wiped,
        Some(Err(_)) => WIPE_INCOMPLETE,
    };
    let mut y = widgets::section_first(&mut c, t, f, top, heading);
    if !e.parts.is_empty() {
        let states: Vec<String> = (0..e.parts.len())
            .map(|k| match (k.cmp(&e.done), failed) {
                (Ordering::Less, _) => wiped.into(),
                (Ordering::Equal, false) => match e.progress {
                    Some((done, total)) if total > 0 => format!("{} {}%", WIPING, percent(share(done, total))),
                    _ => WIPING.into(),
                },
                (Ordering::Equal, true) if begun => S_UNFINISHED.into(),
                (Ordering::Greater, false) => WAITING_TURN.into(),
                (Ordering::Equal | Ordering::Greater, true) => S_NOT_WIPED.into(),
            })
            .collect();
        let rows: Vec<Row> = e.parts.iter().zip(&states).map(|(&part, state)| Row::new(fmt::part(l, part)).trail(Trail::Text(state))).collect();
        y = widgets::list(&mut c, t, f, y, &rows).bottom;
    }
    let notes: Vec<String> = match (&e.outcome, e.asked, e.reset) {
        (Some(Err(stop)), _, _) => {
            let (what, detail) = fmt::erase_stop(l, stop);
            std::iter::once(what).chain(detail).collect()
        }
        (_, true, true) => {
            let leaving = match s.asked.shutdown_after {
                true => S_ALL_OF_THIS_PHONES_DATA_IS_BEING_WIPED_AND_IT_POWERS_OFF_ON_ITS_OWN_ONCE_DONE,
                false => S_ALL_OF_THIS_PHONES_DATA_IS_BEING_WIPED_AND_IT_REBOOTS_ON_ITS_OWN_ONCE_DONE,
            };
            vec![leaving.into()]
        }
        (_, true, false) => vec![if s.asked.shutdown_after { POWERS_OFF_WHEN_DONE } else { REBOOTS_WHEN_DONE }.into()],
        (None, false, _) => vec![KEEP_POWER.into()],
        (Some(Ok(())), false, true) => {
            vec![S_THE_DATA_PARTITION_IS_MADE_ANEW_AT_THE_NEXT_START_WHICH_THEN_TAKES_LONGER.into()]
        }
        (Some(Ok(())), false, false) => Vec::new(),
    };
    for note in &notes {
        y = widgets::footnote(&mut c, t, f, y, note);
    }
    if let (true, Some(reason)) = (e.asked, &s.asked.reason) {
        y = widgets::info_card(&mut c, t, f, y + t.m.dp(12.0), &[(REASON, reason.clone())]);
    }
    if failed || (!e.asked && e.outcome.is_some()) {
        let below = widgets::section(&mut c, t, f, y, ACTIONS);
        y = afterwards(&mut c, t, f, l, below, e.asked, &mut out);
    }
    let title = match (e.reset, e.asked) {
        (true, _) => FACTORY_RESET,
        (false, true) => S_WIPE_CACHE,
        (false, false) => S_WIPE_DATA,
    };
    Frame { back: false, ..Frame::new(t, fit(c, t, y), out, title, top) }
}

const FACTORY_RESET: Text = S_FACTORY_RESET;

/// The system could not start Android and asks whether to erase all user
/// data: start it again, look around the recovery first, or erase.
pub fn cant_start(cx: &Ctx) -> Frame {
    let (t, f, s, _l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let what = S_THE_SYSTEM_COULDNT_START_AND_THIS_PHONES_DATA_MAY_BE_DAMAGED_IF_EVERY_START_ENDS_ON_THIS_PAGE_A_FACTORY_RESET_MAY_HELP_THOUGH_IT_WIPES_ALL_OF_THIS_PHONES_DATA;
    let mut y = widgets::paragraph(&mut c, t, f, top, what);
    if let Some(reason) = &s.asked.reason {
        y = widgets::info_card(&mut c, t, f, y + t.m.dp(12.0), &[(REASON, reason.clone())]);
    }
    y = widgets::section(&mut c, t, f, y, ACTIONS);
    let rows = [
        Row::new(S_TRY_AGAIN).subtitle(S_STARTS_THE_SYSTEM_AGAIN).trail(Trail::None),
        Row::new(S_ENTER_RECOVERY).subtitle(S_WIPES_NOTHING).trail(Trail::None),
    ];
    let choices = widgets::list(&mut c, t, f, y, &rows);
    out.extend(hits(&choices.rows, &[Action::RebootTo(Target::System), Action::Dismiss]));
    let y = widgets::footnote(&mut c, t, f, choices.bottom, S_DATA_A_FACTORY_RESET_WIPES_CANT_BE_BROUGHT_BACK);
    let (label, danger) = confirm_label(Confirm::FactoryReset, cx.state);
    let track = widgets::confirm(&mut c, t, f, y + t.m.dp(24.0), label, danger, s.confirm);
    out.push(Hit { rect: track, action: Action::Confirm(Confirm::FactoryReset) });
    Frame { back: false, ..Frame::new(t, c, out, S_CANT_START_THE_SYSTEM, top) }
}

/// Something the system asked of the recovery that it does not do: what
/// was asked, and where to go instead.
pub fn unhandled(cx: &Ctx) -> Frame {
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let mut items = vec![(S_REQUEST, s.asked.unhandled.clone().unwrap_or_default())];
    if let Some(reason) = &s.asked.reason {
        items.push((REASON, reason.clone()));
    }
    let y = widgets::info_card(&mut c, t, f, top, &items);
    let y = widgets::footnote(&mut c, t, f, y, S_SKYWARD_DOESNT_SUPPORT_THIS_REQUEST_AND_CHANGED_NOTHING_ON_THIS_PHONE);
    let y = widgets::section(&mut c, t, f, y, ACTIONS);
    afterwards(&mut c, t, f, l, y, true, &mut out);
    Frame { back: false, ..Frame::new(t, c, out, S_CANT_HANDLE_THE_REQUEST, top) }
}


#[cfg(feature = "roots")]
const ROOT: Text = "Root (beta)";
#[cfg(feature = "roots")]
const APATCH_SUBTITLE: Text = S_KERNEL_PATCH_AND_SUPERKEY;
#[cfg(feature = "roots")]
const BUNDLED: Text = S_BUNDLED;
#[cfg(feature = "roots")]
const FOLKPATCH_BUILD: Text = S_FOLKPATCH_BUILD;
#[cfg(feature = "roots")]
const APATCH: Text = "KernelPatch (FolkPatch)";
#[cfg(feature = "roots")]
const KSU: Text = S_KERNELSU_FAMILY;
#[cfg(feature = "roots")]
const KSU_SUBTITLE: Text = S_KERNELSU_NEXT_SUKISU_AND_BAKASU;
#[cfg(feature = "roots")]
const APATCH_SLOTS: Text = S_PARTITIONS;
#[cfg(feature = "roots")]
const APATCH_KEY: Text = S_KEY;
#[cfg(feature = "roots")]
const LOG: Text = S_LOG;
#[cfg(feature = "roots")]
const APATCH_KEY_TITLE: Text = S_SUPERKEY;
#[cfg(feature = "roots")]
const APATCH_TYPED: Text = S_SET;
#[cfg(feature = "roots")]
const APATCH_NOT_TYPED: Text = S_NOT_SET;
#[cfg(feature = "roots")]
const APATCH_NEW: Text = S_MADE_WHEN_IT_RUNS;
#[cfg(feature = "roots")]
const APATCH_LAST: Text = S_THE_LAST_RUNS;
#[cfg(feature = "roots")]
const KSU_TITLE: Text = S_KERNELSU_MANAGER;
#[cfg(feature = "roots")]
const KSU_MANAGER: Text = S_MANAGER;
#[cfg(feature = "roots")]
const KSU_RELEASE: Text = S_RELEASE;
#[cfg(feature = "roots")]
const KSU_SOURCE: Text = S_SOURCE;
#[cfg(feature = "roots")]
const KSU_DOWNLOAD: Text = S_DOWNLOAD_FROM_GITHUB;
#[cfg(feature = "roots")]
const KSU_INSTALLED: Text = S_INSTALLED_MANAGER;
#[cfg(feature = "roots")]
const KSU_INSTALLED_MANAGERS: Text = S_INSTALLED_MANAGERS;
#[cfg(feature = "roots")]
const KSU_INSTALLED_VALUE: Text = S_INSTALLED;
#[cfg(feature = "roots")]
const KSU_NOT_FOUND: Text = S_NOT_FOUND;
#[cfg(feature = "roots")]
const KSU_STABLE: Text = S_STABLE;
#[cfg(feature = "roots")]
const KSU_LATEST: Text = S_LATEST_PRERELEASES_TOO;
#[cfg(feature = "roots")]
const KSU_SLOTS: Text = S_PARTITIONS;
#[cfg(feature = "roots")]
const KERNEL_TITLE: Text = S_INSTALL_KERNEL;
#[cfg(feature = "roots")]
const KERNEL_KERNEL: Text = S_KERNEL;
#[cfg(feature = "roots")]
const KERNEL_RELEASE: Text = S_RELEASE;
#[cfg(feature = "roots")]
const KERNEL_GITHUB: Text = "GitHub";
#[cfg(feature = "roots")]
const KERNEL_VERSION: Text = S_KERNEL_VERSION;
#[cfg(feature = "roots")]
const KERNEL_CHECKING: Text = S_CHECKING;
#[cfg(feature = "roots")]
const KERNEL_UNAVAILABLE: Text = S_NOT_FOUND;
#[cfg(feature = "roots")]
const ROOT_PATCHING: Text = S_PATCHING;
#[cfg(feature = "roots")]
const ROOT_KERNELS: Text = S_PRE_BUILT_KERNELS;


/// Root: what this recovery can do to the boot image, one page to each way of
/// doing it. APatch, the KernelSU family and the phone's own kernels share
/// the page.
#[cfg(feature = "roots")]
pub fn root(cx: &Ctx) -> Frame {
    let (t, f, s, _l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    // What this page holds: the root methods this build offers, and the
    // kernels this phone's own configuration adds to them.
    let y = widgets::footnote(&mut c, t, f, top, S_THESE_ARE_THE_ROOT_METHODS_AND_KERNELS_THIS_RECOVERY_OFFERS);
    // Patching: the methods that write a patch into an image the phone
    // already has.
    let mut y = widgets::section(&mut c, t, f, y, ROOT_PATCHING);
    let ways = widgets::list(
        &mut c,
        t,
        f,
        y,
        &[
            Row::new(APATCH).subtitle(APATCH_SUBTITLE),
            Row::new(KSU).subtitle(KSU_SUBTITLE),
        ],
    );
    out.extend(hits(&ways.rows, &[Action::Open(Screen::Apatch), Action::Open(Screen::Ksu)]));

    // Pre-built kernels: complete images a device's configuration offers.
    y = ways.bottom;
    if !s.kernel_plugins.is_empty() {
        y = widgets::section(&mut c, t, f, y, ROOT_KERNELS);
        let rows: Vec<Row> = s
            .kernel_plugins
            .iter()
            .map(|plugin| match &plugin.subtitle {
                Some(subtitle) => Row::new(&plugin.name).subtitle(subtitle),
                None => Row::new(&plugin.name),
            })
            .collect();
        let kernels = widgets::list(&mut c, t, f, y, &rows);
        let acts: Vec<Action> = (0..s.kernel_plugins.len()).map(Action::KernelPlugin).collect();
        out.extend(hits(&kernels.rows, &acts));
    }

    Frame::new(t, c, out, ROOT, top)
}

/// APatch's own page: the slots, the key and the superkey card, what the page
/// found when it opened, and a swipe to start. The run itself is a page of its
/// own, so none of this stays under it.
#[cfg(feature = "roots")]
pub fn apatch(cx: &Ctx) -> Frame {
    use crate::app::{KeyMode, ApatchAction};
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let m = &t.m;
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = m.content_top;
    let root = s.apatch.clone().unwrap_or_default();

    // What this recovery patches with, first: the release as FolkPatch builds
    // it, with the hashes of both tools under it, so they can be checked.
    let short = |hash: &str| hash.chars().take(16).collect::<String>();
    let bundled = format!(
        "KernelPatch {} ({})\nkptools {}\nkpimg {}",
        skyward_root::folkpatch::KPIMG_VERSION,
        FOLKPATCH_BUILD,
        short(skyward_root::folkpatch::KPTOOLS_SHA256),
        short(skyward_root::folkpatch::KPIMG_SHA256)
    );
    let y = widgets::section_first(&mut c, t, f, top, BUNDLED);
    let y = widgets::info_card(&mut c, t, f, y, &[(S_VERSION, bundled)]);

    // The slots: the one the phone starts from, the other, or both.
    let mut y = widgets::section(&mut c, t, f, y, APATCH_SLOTS);
    let titles = Pick::ALL.map(|pick| pick_title(l, pick, cx.info.slot));
    let picks: Vec<Row> = Pick::ALL
        .iter()
        .zip(&titles)
        .map(|(pick, title)| Row::new(title).trail(Trail::Check(*pick == root.pick)))
        .collect();
    let slots = widgets::list(&mut c, t, f, y, &picks);
    out.extend(hits(&slots.rows, &Pick::ALL.map(|pick| Action::Apatch(ApatchAction::Pick(pick)))));

    // The key: one of three ways to come by it.
    y = widgets::section(&mut c, t, f, slots.bottom, APATCH_KEY);
    let last = root.last.is_some();
    let ways: [(&'static str, bool); 3] = [
        (S_GENERATE_A_KEY, true),
        (S_ENTER_A_KEY, true),
        (S_USE_THE_LAST_KEY, last),
    ];
    let modes: Vec<Row> = ways
        .iter()
        .zip([KeyMode::Generate, KeyMode::Enter, KeyMode::Last])
        .map(|((title, applies), mode)| {
            let row = Row::new(title).trail(Trail::Check(root.key_mode == mode));
            match applies {
                true => row,
                false => row.tint(t.pal.subtitle),
            }
        })
        .collect();
    let ways = widgets::list(&mut c, t, f, y, &modes);
    let mut acts = vec![
        Action::Apatch(ApatchAction::KeyMode(KeyMode::Generate)),
        Action::Apatch(ApatchAction::KeyMode(KeyMode::Enter)),
        Action::Apatch(ApatchAction::KeyMode(KeyMode::Last)),
    ];
    // A key of one's own is typed on the key page once it is chosen.
    if root.key_mode == KeyMode::Enter {
        acts[1] = Action::Apatch(ApatchAction::EditKey);
    }
    let applies = [true, true, last];
    let rows: Vec<Rect> = ways.rows.iter().zip(applies).filter(|(_, on)| *on).map(|(r, _)| *r).collect();
    let acts: Vec<Action> = acts.into_iter().zip(applies).filter(|(_, on)| *on).map(|(a, _)| a).collect();
    out.extend(hits(&rows, &acts));

    // The superkey card, on its own under the ways to come by one: the key as
    // it stands, dim while it is not one being typed, and it opens the page
    // that shows it and, with typing chosen, takes it.
    let held = match root.key_mode {
        KeyMode::Enter => if root.entered.is_empty() { APATCH_NOT_TYPED } else { APATCH_TYPED }.to_string(),
        KeyMode::Last => APATCH_LAST.to_string(),
        KeyMode::Generate => APATCH_NEW.to_string(),
    };
    let card = Row::new(APATCH_KEY_TITLE).trail(Trail::Value(&held));
    let card = match root.key_mode {
        KeyMode::Enter => card,
        _ => card.tint(t.pal.subtitle),
    };
    let key_card = widgets::list(&mut c, t, f, ways.bottom + m.card_gap, &[card]);
    out.extend(hits(&key_card.rows, &[Action::Apatch(ApatchAction::EditKey)]));

    // What the page found when it opened: what stands in the way, or that
    // nothing does, and whether the image carries the patch already.
    let mut y = key_card.bottom;
    for note in apatch_notes(l, cx, &root) {
        y = widgets::footnote(&mut c, t, f, y, &note);
    }

    if ready_to_apatch(cx) {
        let (label, danger) = confirm_label(Confirm::ApatchPatch, cx.state);
        let track = widgets::confirm(&mut c, t, f, y + m.card_gap, label, danger, s.confirm);
        out.push(Hit { rect: track, action: Action::Confirm(Confirm::ApatchPatch) });
        y = track.y + track.h;
    }

    Frame::new(t, fit(c, t, y), out, APATCH, top)
}

/// The run's own page: its log, the key it made once it has ended, and the
/// swipe that ends it. Nothing of the page it was started from, and no card
/// over the log.
#[cfg(feature = "roots")]
pub fn apatch_job(cx: &Ctx) -> Frame {
    use crate::app::ApatchJob;
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let m = &t.m;
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = m.content_top;
    let root = s.apatch.clone().unwrap_or_default();
    let finished = matches!(root.job, ApatchJob::Finished(_));

    // The log is what the run printed; once it has ended, how it ended and
    // what it left behind are its last lines.
    let (lines, key) = match &root.job {
        ApatchJob::Finished(outcome) => {
            let (summary, key, notes) = apatch_result(l, outcome);
            let mut lines = root.log.clone();
            lines.push(summary);
            lines.extend(notes);
            (lines, key)
        }
        _ => (root.log.clone(), None),
    };

    // The foot: a swipe, then the row under it, both there whether they show
    // or not, so nothing above them moves.
    let swipe_h = m.dp(widgets::CONFIRM_H_DP);
    let done_top = m.h as f32 - m.margin - swipe_h;
    let copy_top = done_top - m.card_gap - m.row_h;

    // The log, under its heading, and the key the run made in a card of its
    // own, under its own heading.
    let log_top = widgets::section_first(&mut c, t, f, top, LOG);
    let key_room = match &key {
        Some(_) => m.section_block + key_card_height(t, f) + m.card_gap,
        None => 0.0,
    };
    let room = copy_top - m.card_gap - log_top - key_room;
    let y = log_window(&mut c, t, f, log_top, room, &lines);
    if let Some(key) = &key {
        let y = widgets::section(&mut c, t, f, y + m.card_gap, APATCH_KEY);
        key_card(&mut c, t, f, y, key);
    }

    if let ApatchJob::Finished(outcome) = &root.job {
        // A run that made a key offers to keep a copy of it, once; one that
        // stopped at an image carrying the patch already offers to write it
        // again. Either way, the way out is the slider under them.
        let again = matches!(**outcome, skyward_root::folkpatch::Outcome::Refused(skyward_root::folkpatch::Refusal::AlreadyPatched(_)));
        if key.is_some() && root.copied.is_none() {
            let label = S_KEEP_A_COPY_OF_THE_KEY;
            let button = widgets::button(&mut c, t, f, copy_top, label);
            out.push(Hit { rect: button, action: Action::Apatch(crate::app::ApatchAction::KeepKey) });
        } else if again {
            let (label, danger) = confirm_label(Confirm::ApatchRepatch, cx.state);
            let track = widgets::confirm(&mut c, t, f, copy_top, label, danger, s.confirm);
            out.push(Hit { rect: track, action: Action::Confirm(Confirm::ApatchRepatch) });
        }
        // The way out, in the accent the rest of the pages give a plain step.
        let (label, danger) = confirm_label(Confirm::ApatchDone, cx.state);
        let track = widgets::confirm(&mut c, t, f, done_top, label, danger, s.confirm);
        out.push(Hit { rect: track, action: Action::Confirm(Confirm::ApatchDone) });
    }
    Frame { back: finished, ..Frame::new(t, c, out, APATCH, top) }
}

/// The KernelSU-family page: what a run would use, the slots it patches and
/// a swipe to start. The manager is chosen on a page of its own, and the run
/// on another.
#[cfg(feature = "roots")]
pub fn ksu(cx: &Ctx) -> Frame {
    use crate::app::KsuAction;
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let m = &t.m;
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = m.content_top;
    let ksu = s.ksu.clone().unwrap_or_default();

    // The manager, its release and where it comes from are chosen on their
    // own page; the card here says what a run would use.
    let y = widgets::section_first(&mut c, t, f, top, KSU_MANAGER);
    let summary = ksu_summary(l, &ksu);
    let managers =
        widgets::list(&mut c, t, f, y, &[Row::new(ksu.flavor.name()).trail(Trail::Value(&summary))]);
    out.extend(hits(&managers.rows, &[Action::Open(Screen::KsuManager)]));

    // The slots, as the APatch page has them.
    let y = widgets::section(&mut c, t, f, managers.bottom, KSU_SLOTS);
    let titles = Pick::ALL.map(|pick| pick_title(l, pick, cx.info.slot));
    let picks: Vec<Row> = Pick::ALL
        .iter()
        .zip(&titles)
        .map(|(pick, title)| Row::new(title).trail(Trail::Check(*pick == ksu.pick)))
        .collect();
    let slots = widgets::list(&mut c, t, f, y, &picks);
    out.extend(hits(&slots.rows, &Pick::ALL.map(|pick| Action::Ksu(KsuAction::Pick(pick)))));

    // What the page found when it opened, and what a run would do.
    let mut y = slots.bottom;
    for note in ksu_notes(l, cx, &ksu) {
        y = widgets::footnote(&mut c, t, f, y, &note);
    }

    if ready_to_ksu(cx) {
        let (label, danger) = confirm_label(Confirm::KsuPatch, cx.state);
        let track = widgets::confirm(&mut c, t, f, y + m.card_gap, label, danger, s.confirm);
        out.push(Hit { rect: track, action: Action::Confirm(Confirm::KsuPatch) });
        y = track.y + track.h;
    }

    Frame::new(t, fit(c, t, y), out, KSU_TITLE, top)
}

/// What the manager card on the family's page says a run would use.
#[cfg(feature = "roots")]
fn ksu_summary(_l: Lang, ksu: &crate::app::KsuState) -> String {
    use crate::app::KsuSource;
    use skyward_root::ksu::Tier;
    match ksu.source {
        KsuSource::Download => if ksu.tier == Tier::Stable { KSU_STABLE } else { KSU_LATEST }.to_string(),
        KsuSource::Installed => match ksu.installed_manager() {
            None => KSU_NOT_FOUND.to_string(),
            Some(manager) if manager.flavor == ksu.flavor => KSU_INSTALLED_VALUE.to_string(),
            Some(manager) => format!("{} · {}", KSU_INSTALLED_VALUE, manager.flavor.name()),
        },
    }
}

/// The manager, its release and where it comes from, chosen on a page of
/// their own.
#[cfg(feature = "roots")]
pub fn ksu_manager(cx: &Ctx) -> Frame {
    use crate::app::{KsuAction, KsuSource, KsuTier};
    use skyward_root::ksu::Flavor;
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let m = &t.m;
    let mut c = long_page(cx);
    let mut out = Vec::new();
    let top = m.content_top;
    let ksu = s.ksu.clone().unwrap_or_default();

    // The manager whose `ksud` patches the image.
    let y = widgets::section_first(&mut c, t, f, top, KSU_MANAGER);
    let rows: Vec<Row> = Flavor::ALL
        .iter()
        .map(|flavor| Row::new(flavor.name()).trail(Trail::Check(*flavor == ksu.flavor)))
        .collect();
    let managers = widgets::list(&mut c, t, f, y, &rows);
    out.extend(hits(&managers.rows, &Flavor::ALL.map(|flavor| Action::Ksu(KsuAction::Flavor(flavor)))));

    // Where the APK comes from: its own release, or one already in Android.
    let mut y = widgets::section(&mut c, t, f, managers.bottom, KSU_SOURCE);
    let no_manager = ksu.installed.is_empty();
    let sources = [
        Row::new(KSU_DOWNLOAD).trail(Trail::Check(ksu.source == KsuSource::Download)),
        Row::new(KSU_INSTALLED)
            .trail(Trail::Check(ksu.source == KsuSource::Installed))
            .tint(if no_manager { t.pal.subtitle } else { t.pal.text }),
    ];
    let from = widgets::list(&mut c, t, f, y, &sources);
    out.push(Hit { rect: from.rows[0], action: Action::Ksu(KsuAction::Source(KsuSource::Download)) });
    if !no_manager {
        out.push(Hit { rect: from.rows[1], action: Action::Ksu(KsuAction::Source(KsuSource::Installed)) });
    }

    // Which installed APK, where more than one of the family is installed.
    y = from.bottom;
    if ksu.source == KsuSource::Installed && !no_manager {
        y = widgets::section(&mut c, t, f, y, KSU_INSTALLED_MANAGERS);
        let current = ksu.installed_manager().map(|manager| &manager.path);
        let repeated = |flavor| ksu.installed.iter().filter(|manager| manager.flavor == flavor).count() > 1;
        let rows: Vec<Row> = ksu
            .installed
            .iter()
            .map(|manager| {
                let row = Row::new(manager.flavor.name()).trail(Trail::Check(current == Some(&manager.path)));
                if repeated(manager.flavor) { row.subtitle(&manager.package) } else { row }
            })
            .collect();
        let installed = widgets::list(&mut c, t, f, y, &rows);
        let acts: Vec<Action> = (0..ksu.installed.len())
            .map(|index| Action::Ksu(KsuAction::Installed(index)))
            .collect();
        out.extend(hits(&installed.rows, &acts));
        y = installed.bottom;
    }

    // The release, when the APK comes from it: the newest stable one, or the
    // newest one even while it is a prerelease.
    if ksu.source == KsuSource::Download {
        y = widgets::section(&mut c, t, f, y, KSU_RELEASE);
        let releases = [
            (KSU_STABLE, KsuTier::Stable),
            (KSU_LATEST, KsuTier::Latest),
        ];
        let rows: Vec<Row> = releases
            .into_iter()
            .map(|(title, tier)| Row::new(title).trail(Trail::Check(tier == ksu.tier)))
            .collect();
        let tiers = widgets::list(&mut c, t, f, y, &rows);
        out.extend(hits(
            &tiers.rows,
            &[Action::Ksu(KsuAction::Tier(KsuTier::Stable)), Action::Ksu(KsuAction::Tier(KsuTier::Latest))],
        ));
        y = tiers.bottom;
    }

    for note in ksu_manager_notes(l, cx, &ksu) {
        y = widgets::footnote(&mut c, t, f, y, &note);
    }

    Frame::new(t, fit(c, t, y), out, KSU_MANAGER, top)
}

/// What the manager page says about the choices it offers.
#[cfg(feature = "roots")]
fn ksu_manager_notes(_l: Lang, cx: &Ctx, ksu: &crate::app::KsuState) -> Vec<String> {
    use crate::app::KsuSource;
    use skyward_root::ksu::{Flavor, Tier};
    let mut notes = Vec::new();
    if ksu.flavor == Flavor::BakaSu && ksu.tier == Tier::Stable && ksu.source == KsuSource::Download {
        notes.push(S_BAKASU_PUBLISHES_ONLY_PRERELEASES_STABLE_HAS_NONE_SO_CHOOSE_LATEST.to_string());
    }
    if ksu.source == KsuSource::Download && !cx.state.online() {
        notes.push(S_JOIN_A_NETWORK_FIRST_THE_MANAGER_IS_DOWNLOADED_FROM_GITHUB.to_string());
    }
    if ksu.source == KsuSource::Installed && ksu.installed_manager().is_none() {
        notes.push(S_NO_MANAGER_OF_THE_FAMILY_IS_INSTALLED_IN_ANDROID.to_string());
    }
    if ksu.source == KsuSource::Installed {
        if let Some(manager) = ksu.installed_manager() {
            if manager.flavor != ksu.flavor {
                let selected = ksu.flavor.name();
                let installed = manager.flavor.name();
                notes.push(crate::t!(S_THE_PATCH_USES_THE_INSTALLED_APK_NOT_THE_SELECTED, installed = installed, selected = selected));
            }
        }
    }
    notes
}

/// The kernel a device's configuration offers: what it is, the slots it goes
/// to and a swipe to start. The run is a page of its own.
#[cfg(feature = "roots")]
pub fn kernel(cx: &Ctx) -> Frame {
    use crate::app::KernelAction;
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let m = &t.m;
    let top = m.content_top;
    let Some(kernel) = s.kernel.as_ref() else {
        return Frame::new(t, page(cx), Vec::new(), KERNEL_TITLE, top);
    };
    let mut c = long_page(cx);
    let mut out = Vec::new();

    // The kernel the release is of, as the device's configuration names it.
    let mut y = widgets::section_first(&mut c, t, f, top, KERNEL_KERNEL);
    let row = match &kernel.plugin.subtitle {
        Some(subtitle) => Row::new(&kernel.plugin.name).subtitle(subtitle),
        None => Row::new(&kernel.plugin.name),
    };
    let card = widgets::list(&mut c, t, f, y, &[row]);

    // The newest release, its kernel version and the manager it carries.
    y = widgets::section(&mut c, t, f, card.bottom, KERNEL_RELEASE);
    let rows: Vec<Row> = match &kernel.details {
        None if !cx.state.online() => {
            vec![Row::new(KERNEL_GITHUB).trail(Trail::Text(KERNEL_UNAVAILABLE))]
        }
        None => vec![Row::new(KERNEL_GITHUB).trail(Trail::Text(KERNEL_CHECKING))],
        Some(Err(why)) => vec![Row::new(KERNEL_GITHUB).subtitle(why).trail(Trail::Text(KERNEL_UNAVAILABLE))],
        Some(Ok(details)) => {
            let mut rows = vec![Row::new(KERNEL_GITHUB).trail(Trail::Text(&details.tag))];
            if let Some(version) = &details.kernel {
                rows.push(Row::new(KERNEL_VERSION).trail(Trail::Text(version)));
            }
            if let Some(manager) = &details.manager {
                rows.push(Row::new(KSU_MANAGER).trail(Trail::Text(manager)));
            }
            rows
        }
    };
    let release = widgets::list(&mut c, t, f, y, &rows);

    // The slots, as the root method pages have them.
    let y = widgets::section(&mut c, t, f, release.bottom, KSU_SLOTS);
    let titles = Pick::ALL.map(|pick| pick_title(l, pick, cx.info.slot));
    let picks: Vec<Row> = Pick::ALL
        .iter()
        .zip(&titles)
        .map(|(pick, title)| Row::new(title).trail(Trail::Check(*pick == kernel.pick)))
        .collect();
    let slots = widgets::list(&mut c, t, f, y, &picks);
    out.extend(hits(&slots.rows, &Pick::ALL.map(|pick| Action::Kernel(KernelAction::Pick(pick)))));

    // What the page found when it opened, and what a run would do.
    let mut y = slots.bottom;
    for note in kernel_notes(l, cx, kernel) {
        y = widgets::footnote(&mut c, t, f, y, &note);
    }

    if ready_to_kernel(cx) {
        let (label, danger) = confirm_label(Confirm::KernelInstall, cx.state);
        let track = widgets::confirm(&mut c, t, f, y + m.card_gap, label, danger, s.confirm);
        out.push(Hit { rect: track, action: Action::Confirm(Confirm::KernelInstall) });
        y = track.y + track.h;
    }

    Frame::new(t, fit(c, t, y), out, KERNEL_TITLE, top)
}

/// Whether nothing stands between the kernel page and a run: a slot to write,
/// the owner's storage, charge enough, and a network to fetch the release.
#[cfg(feature = "roots")]
fn ready_to_kernel(cx: &Ctx) -> bool {
    let enough_charge = cx.info.battery.as_ref().and_then(skyward_install::battery_refusal).is_none();
    cx.state.kernel.is_some() && cx.info.slot.is_some() && cx.state.data.storage_open() && enough_charge && cx.state.online()
}

/// What the kernel page says before a run: what stands in the way, or what it
/// will do.
#[cfg(feature = "roots")]
fn kernel_notes(_l: Lang, cx: &Ctx, kernel: &crate::app::KernelState) -> Vec<String> {
    let mut notes = Vec::new();
    if !cx.state.online() {
        notes.push(S_JOIN_A_NETWORK_FIRST_THE_KERNEL_IS_DOWNLOADED_FROM_GITHUB.to_string());
    }
    if cx.info.slot.is_none() {
        notes.push(S_THIS_PHONE_HAS_ONE_SLOT_SO_THERE_ARE_NO_BOOT_PARTITIONS_TO_WRITE.to_string());
    }
    if !cx.state.data.storage_open() {
        notes.push(S_OPEN_THE_DATA_PARTITION_IN_DECRYPT_FIRST_THE_BACKUPS_ARE_SAVED_TO_INTERNAL_STORAGE.to_string());
    }
    if let Some(skyward_install::Refusal::Battery { level, .. }) = cx.info.battery.as_ref().and_then(skyward_install::battery_refusal) {
        notes.push(crate::t!(S_ONLY_CHARGE_CHARGE_THE_PHONE_FIRST, level = level));
    }
    notes.push(S_THE_NEWEST_STABLE_RELEASE_IS_DOWNLOADED_AND_CHECKED_AGAINST_ITS_SHA_256_THE_BOOT_PARTITION_IS_BACKED_UP_FIRST.to_string());
    notes.push(S_THE_KERNEL_IS_UNSIGNED_A_BOOTLOADER_THAT_ENFORCES_AVB_MAY_REFUSE_TO_START_IT_RESTORE_THE_BACKUP_IF_IT_DOES_NOT.to_string());
    if let Some(note) = &kernel.plugin.note {
        notes.push(note.clone());
    }
    notes
}

/// The kernel install's page: what it has done, and how it ended.
#[cfg(feature = "roots")]
pub fn kernel_job(cx: &Ctx) -> Frame {
    use crate::app::KernelJob;
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let m = &t.m;
    let Some(kernel) = s.kernel.as_ref() else {
        return Frame::new(t, page(cx), Vec::new(), KERNEL_TITLE, m.content_top);
    };
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = m.content_top;
    let finished = matches!(kernel.job, KernelJob::Finished(_));

    let lines = match &kernel.job {
        KernelJob::Finished(outcome) => {
            let (summary, notes) = kernel_result(l, kernel, outcome);
            let mut lines = kernel.log.clone();
            lines.push(summary);
            lines.extend(notes);
            lines
        }
        _ => kernel.log.clone(),
    };

    let swipe_h = m.dp(widgets::CONFIRM_H_DP);
    let done_top = m.h as f32 - m.margin - swipe_h;
    let log_top = widgets::section_first(&mut c, t, f, top, LOG);
    let room = done_top - m.card_gap - log_top;
    log_window(&mut c, t, f, log_top, room, &lines);

    if finished {
        let (label, danger) = confirm_label(Confirm::KernelDone, cx.state);
        let track = widgets::confirm(&mut c, t, f, done_top, label, danger, s.confirm);
        out.push(Hit { rect: track, action: Action::Confirm(Confirm::KernelDone) });
    }
    Frame { back: finished, ..Frame::new(t, c, out, KERNEL_TITLE, top) }
}

/// The heading and the line under it a kernel run's stage says.
#[cfg(feature = "roots")]
pub(crate) fn kernel_stage(_l: Lang, stage: Option<&skyward_root::kernel::Stage>) -> (String, String) {
    use skyward_root::kernel::Stage;
    let bytes = |done: u64, total: u64| crate::t!(S_OF_BYTES, done = done, total = total);
    match stage {
        None => (S_GETTING_READY.into(), String::new()),
        Some(Stage::Finding) => (S_LOOKING_FOR_THE_KERNELS_RELEASE.into(), String::new()),
        Some(Stage::Downloading { done, total }) => {
            (S_DOWNLOADING_THE_KERNEL.into(), bytes(*done, *total))
        }
        Some(Stage::DownloadingManager { done, total }) => {
            (S_DOWNLOADING_THE_MANAGER.into(), bytes(*done, *total))
        }
        Some(Stage::Checking) => (S_CHECKING_THE_KERNEL_IMAGE.into(), String::new()),
        Some(Stage::BackingUp { slot, done, total }) => {
            let slot = slot.letter();
            (
                crate::t!(S_SAVING_SLOT_S_BOOT_PARTITION, slot = slot),
                bytes(*done, *total),
            )
        }
        Some(Stage::Flashing { slot, done, total }) => {
            let slot = slot.letter();
            (
                crate::t!(S_WRITING_SLOT_S_BOOT_PARTITION, slot = slot),
                bytes(*done, *total),
            )
        }
        Some(Stage::Saving) => (S_SAVING_THE_MANIFEST.into(), String::new()),
    }
}

/// How a kernel run ended: the line the window ends with, and what else the
/// page has to say.
#[cfg(feature = "roots")]
fn kernel_result(l: Lang, kernel: &crate::app::KernelState, outcome: &skyward_root::kernel::Outcome) -> (String, Vec<String>) {
    match outcome {
        skyward_root::kernel::Outcome::Installed(installed) => {
            let slots: Vec<&str> = installed.slots.iter().map(|slot| slot.letter()).collect();
            let slots = slots.join(", ");
            let folder = installed.backup_dir.parent().map_or_else(|| installed.backup_dir.display().to_string(), |path| path.display().to_string());
            let tag = &installed.tag;
            let image = &installed.image;
            let name = &kernel.plugin.name;
            let mut notes = vec![
                crate::t!(S_THE_BOOT_PARTITIONS_FROM_BEFORE_THE_WRITE_ARE_IN_IN_ITS_BACKUP_FOLDER, folder = folder),
                S_THE_KERNEL_IS_UNSIGNED_A_BOOTLOADER_THAT_ENFORCES_AVB_MAY_REFUSE_TO_START_IT_RESTORE_THE_BACKUP_IF_IT_DOES_NOT
                .to_string(),
            ];
            if let Some(note) = &kernel.plugin.note {
                notes.push(note.clone());
            }
            if let Some(manager) = &installed.manager {
                let manager = manager.display().to_string();
                notes.push(crate::t!(S_THE_MANAGER_IS_KEPT_AT_INSTALL_AND_OPEN_IT_IN_ANDROID_AFTER_THE_PHONE_STARTS, manager = manager));
            } else if let Some(why) = &installed.manager_note {
                notes.push(crate::t!(S_THE_MANAGER_WAS_NOT_DOWNLOADED, why = why));
            }
            let summary = crate::t!(S_INSTALLED_TO_SLOT, name = name, tag = tag, slots = slots, image = image);
            (summary, notes)
        }
        skyward_root::kernel::Outcome::Refused(refusal) => {
            let why = kernel_refusal(l, refusal);
            let summary = crate::t!(S_NOT_INSTALLED, why = why);
            (summary, vec![S_NO_BOOT_PARTITION_WAS_WRITTEN.to_string()])
        }
        skyward_root::kernel::Outcome::Failed(failure) => {
            let (heading, _) = kernel_stage(l, Some(&failure.stage));
            let why = &failure.why;
            let mut notes = Vec::new();
            if !failure.written.is_empty() {
                let slots: Vec<&str> = failure.written.iter().map(|slot| slot.letter()).collect();
                let slots = slots.join(", ");
                notes.push(crate::t!(S_SLOT_MAY_BE_PARTLY_WRITTEN_DONT_START_IT, slots = slots));
            }
            for copy in &failure.backups {
                let path = copy.path.display().to_string();
                let partition = format!("{}{}", copy.partition, copy.slot.suffix());
                notes.push(crate::t!(S_FROM_A_COMPUTER_FASTBOOT_FLASH, partition = partition, path = path));
            }
            let summary = crate::t!(S_NOT_INSTALLED_FAILED_WHILE, heading = heading, why = why);
            (summary, notes)
        }
    }
}

/// Why a kernel run did not start, or stopped before writing.
#[cfg(feature = "roots")]
fn kernel_refusal(_l: Lang, refusal: &skyward_root::kernel::Refusal) -> String {
    use skyward_root::kernel::Refusal;
    match refusal {
        Refusal::NoSlot => S_THIS_PHONE_HAS_NO_A_B_SLOT_TO_WRITE
            .into(),
        Refusal::Missing { slot, partition } => {
            let slot = slot.letter();
            crate::t!(S_SLOT_HAS_NO_PARTITION, slot = slot, partition = partition)
        }
        Refusal::Battery { level, charging } => {
            let how = if *charging {
                S_CHARGE_TOO_LOW_EVEN_WHILE_CHARGING
            } else {
                S_CHARGE_TOO_LOW_CHARGE_THE_PHONE_FIRST
            };
            how.replace("{level}", &level.to_string())
        }
        Refusal::NotBootImage { slot, partition } => {
            let slot = slot.letter();
            crate::t!(S_SLOT_S_IS_NOT_A_BOOT_IMAGE, slot = slot, partition = partition)
        }
        Refusal::RecoveryImage { slot, partition } => {
            let slot = slot.letter();
            crate::t!(S_THE_IMAGE_CARRIES_A_RECOVERY_NOT_THE_SYSTEMS_BOOT_AND_CANNOT_GO_TO_SLOT_S, slot = slot, partition = partition)
        }
        Refusal::TooLarge { slot, partition, image, partition_size } => {
            let slot = slot.letter();
            crate::t!(S_THE_KERNEL_IMAGE_IS_BYTES_MORE_THAN_SLOT_S_PARTITION_BYTES, image = image, slot = slot, partition = partition, partition_size = partition_size)
        }
        Refusal::NoRoom { need, have } => crate::t!(S_NOT_ENOUGH_INTERNAL_STORAGE_BYTES_NEEDED_FREE, need = need, have = have),
        Refusal::StorageClosed => S_INTERNAL_STORAGE_IS_NOT_OPEN
            .into(),
        Refusal::Damaged => S_THE_DOWNLOADED_IMAGE_DOES_NOT_MATCH_ITS_SHA_256
            .into(),
        Refusal::NoImage => S_THE_DOWNLOADED_IMAGE_IS_MISSING
            .into(),
        Refusal::NoRelease => S_THE_REPOSITORY_HAS_NO_STABLE_RELEASE_WITH_A_KERNEL_IMAGE
            .into(),
    }
}

/// The KernelSU-family run's page: what it has done, and how it ended.
#[cfg(feature = "roots")]
pub fn ksu_job(cx: &Ctx) -> Frame {
    use crate::app::KsuJob;
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let m = &t.m;
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = m.content_top;
    let ksu = s.ksu.clone().unwrap_or_default();
    let finished = matches!(ksu.job, KsuJob::Finished(_));

    let lines = match &ksu.job {
        KsuJob::Finished(outcome) => {
            let (summary, notes) = ksu_result(l, outcome);
            let mut lines = ksu.log.clone();
            lines.push(summary);
            lines.extend(notes);
            lines
        }
        _ => ksu.log.clone(),
    };

    let swipe_h = m.dp(widgets::CONFIRM_H_DP);
    let done_top = m.h as f32 - m.margin - swipe_h;
    let log_top = widgets::section_first(&mut c, t, f, top, LOG);
    let room = done_top - m.card_gap - log_top;
    log_window(&mut c, t, f, log_top, room, &lines);

    if finished {
        let (label, danger) = confirm_label(Confirm::KsuDone, cx.state);
        let track = widgets::confirm(&mut c, t, f, done_top, label, danger, s.confirm);
        out.push(Hit { rect: track, action: Action::Confirm(Confirm::KsuDone) });
    }
    Frame { back: finished, ..Frame::new(t, c, out, KSU_TITLE, top) }
}

/// Whether nothing stands between the page and a run: a slot to write, the
/// owner's storage, charge enough, and a manager to patch with.
#[cfg(feature = "roots")]
fn ready_to_ksu(cx: &Ctx) -> bool {
    use crate::app::KsuSource;
    let enough_charge = cx.info.battery.as_ref().and_then(skyward_install::battery_refusal).is_none();
    let ksu = cx.state.ksu.clone().unwrap_or_default();
    let manager = match ksu.source {
        KsuSource::Download => cx.state.online(),
        KsuSource::Installed => ksu.installed_manager().is_some(),
    };
    cx.info.slot.is_some() && cx.state.data.storage_open() && enough_charge && manager
}

/// What the page says before a run: what stands in the way, or what it will
/// do.
#[cfg(feature = "roots")]
fn ksu_notes(_l: Lang, cx: &Ctx, ksu: &crate::app::KsuState) -> Vec<String> {
    use crate::app::KsuSource;
    use skyward_root::ksu::{Flavor, Tier};
    let mut notes = Vec::new();
    if ksu.flavor == Flavor::BakaSu && ksu.tier == Tier::Stable && ksu.source == KsuSource::Download {
        notes.push(S_BAKASU_PUBLISHES_ONLY_PRERELEASES_STABLE_HAS_NONE_SO_CHOOSE_LATEST.to_string());
    }
    if ksu.source == KsuSource::Download && !cx.state.online() {
        notes.push(S_JOIN_A_NETWORK_FIRST_THE_MANAGER_IS_DOWNLOADED_FROM_GITHUB.to_string());
    }
    if ksu.source == KsuSource::Installed && ksu.installed_manager().is_none() {
        notes.push(S_NO_MANAGER_OF_THE_FAMILY_IS_INSTALLED_IN_ANDROID.to_string());
    }
    if cx.info.slot.is_none() {
        notes.push(S_THIS_PHONE_HAS_ONE_SLOT_SO_THERE_ARE_NO_BOOT_PARTITIONS_TO_WRITE.to_string());
    }
    if !cx.state.data.storage_open() {
        notes.push(S_OPEN_THE_DATA_PARTITION_IN_DECRYPT_FIRST_THE_BACKUPS_AND_MANAGER_ARE_SAVED_TO_INTERNAL_STORAGE.to_string());
    }
    if let Some(skyward_install::Refusal::Battery { level, .. }) = cx.info.battery.as_ref().and_then(skyward_install::battery_refusal) {
        notes.push(crate::t!(S_ONLY_CHARGE_CHARGE_THE_PHONE_FIRST, level = level));
    }
    if cx.info.snapshot != skyward_vab::SnapshotState::None {
        notes.push(S_AN_UPDATE_IS_STILL_IN_FLIGHT_THE_PATCH_MAY_BE_OVERWRITTEN_BEFORE_IT_FINISHES.to_string());
    }
    notes.push(S_THE_MANAGER_IS_ONLY_READ_FOR_THE_KSUD_INSIDE_IT_NONE_OF_ITS_SCRIPTS_ARE_RUN.to_string());
    notes.push(S_THE_PATCH_IS_UNSIGNED_A_BOOTLOADER_THAT_ENFORCES_AVB_MAY_REFUSE_TO_START_IT_RESTORE_THE_BACKUP_IF_IT_DOES_NOT.to_string());
    notes.push(S_AFTER_IT_STARTS_INSTALL_THE_MANAGER_APK_FROM_THE_RUNS_FOLDER_IN_ANDROID_IT_MUST_BE_THE_RELEASE_THE_PATCH_WAS_MADE_FROM.to_string());
    notes
}

/// The heading and the line under it a KernelSU-family run's stage says.
#[cfg(feature = "roots")]
pub(crate) fn ksu_stage(_l: Lang, stage: Option<&skyward_root::ksu::Stage>) -> (String, String) {
    use skyward_root::ksu::Stage;
    let bytes = |done: u64, total: u64| crate::t!(S_OF_BYTES, done = done, total = total);
    match stage {
        None => (S_GETTING_READY.into(), String::new()),
        Some(Stage::Checking) => (S_CHECKING.into(), String::new()),
        Some(Stage::Finding) => (S_LOOKING_FOR_THE_MANAGERS_RELEASE.into(), String::new()),
        Some(Stage::Downloading { done, total }) => {
            (S_DOWNLOADING_THE_MANAGER.into(), bytes(*done, *total))
        }
        Some(Stage::Extracting) => (S_TAKING_KSUD_OUT_OF_THE_MANAGER.into(), String::new()),
        Some(Stage::BackingUp { slot, partition, done, total }) => {
            let slot = slot.letter();
            (
                crate::t!(S_SAVING_SLOT_S_PARTITION, slot = slot, partition = partition),
                bytes(*done, *total),
            )
        }
        Some(Stage::Patching { slot, partition }) => {
            let slot = slot.letter();
            (
                crate::t!(S_PATCHING_SLOT_S, slot = slot, partition = partition),
                String::new(),
            )
        }
        Some(Stage::Verifying { slot, partition }) => {
            let slot = slot.letter();
            (
                crate::t!(S_CHECKING_SLOT_S_PATCH, slot = slot, partition = partition),
                String::new(),
            )
        }
        Some(Stage::Flashing { slot, partition, done, total }) => {
            let slot = slot.letter();
            (
                crate::t!(S_WRITING_SLOT_S, slot = slot, partition = partition),
                bytes(*done, *total),
            )
        }
        Some(Stage::SavingManager) => (S_KEEPING_A_COPY_OF_THE_MANAGER.into(), String::new()),
    }
}

/// How a KernelSU-family run ended: the line the window ends with, and what
/// else the page has to say.
#[cfg(feature = "roots")]
fn ksu_result(l: Lang, outcome: &skyward_root::ksu::Outcome) -> (String, Vec<String>) {
    match outcome {
        skyward_root::ksu::Outcome::Installed(installed) => {
            let places: Vec<String> = installed
                .slots
                .iter()
                .zip(&installed.partitions)
                .map(|(slot, partition)| format!("{} {partition}", slot.letter()))
                .collect();
            let places = places.join(", ");
            let name = installed.flavor.name();
            let manager = installed.apk.display().to_string();
            let folder = installed.backup_dir.parent().map_or_else(|| installed.backup_dir.display().to_string(), |p| p.display().to_string());
            let tag = &installed.tag;
            let notes = vec![
                crate::t!(S_THE_PARTITIONS_FROM_BEFORE_THE_WRITE_ARE_IN_IN_ITS_BACKUP_FOLDER, folder = folder),
                crate::t!(S_THE_MANAGER_IS_KEPT_AT_INSTALL_AND_OPEN_IT_IN_ANDROID_AFTER_THE_PHONE_STARTS_2, manager = manager, tag = tag),
                S_THE_PATCH_IS_UNSIGNED_A_BOOTLOADER_THAT_ENFORCES_AVB_MAY_REFUSE_TO_START_IT_RESTORE_THE_BACKUP_IF_IT_DOES_NOT
                .to_string(),
            ];
            let summary = crate::t!(S_INSTALLED_SLOT_IS_PATCHED, name = name, places = places);
            (summary, notes)
        }
        skyward_root::ksu::Outcome::Refused(refusal) => {
            let why = ksu_refusal(l, refusal);
            let summary = crate::t!(S_NOT_INSTALLED, why = why);
            let nothing = S_NO_BOOT_PARTITION_WAS_WRITTEN.to_string();
            (summary, vec![nothing])
        }
        skyward_root::ksu::Outcome::Failed(failure) => {
            let (heading, _) = ksu_stage(l, Some(&failure.stage));
            let why = &failure.why;
            let mut notes = Vec::new();
            if !failure.written.is_empty() {
                let slots: Vec<&str> = failure.written.iter().map(|s| s.letter()).collect();
                let slots = slots.join(", ");
                notes.push(crate::t!(S_SLOT_MAY_BE_PARTLY_WRITTEN_DONT_START_IT, slots = slots));
            }
            for copy in &failure.backups {
                let path = copy.path.display().to_string();
                let partition = format!("{}{}", copy.partition, copy.slot.suffix());
                notes.push(crate::t!(S_FROM_A_COMPUTER_FASTBOOT_FLASH, partition = partition, path = path));
            }
            let summary = crate::t!(S_NOT_INSTALLED_FAILED_WHILE, heading = heading, why = why);
            (summary, notes)
        }
    }
}

/// Why a KernelSU-family run did not start, or stopped before writing.
#[cfg(feature = "roots")]
fn ksu_refusal(_l: Lang, refusal: &skyward_root::ksu::Refusal) -> String {
    use skyward_root::ksu::{Refusal, Tier};
    match refusal {
        Refusal::NoSlot => S_THIS_PHONE_HAS_NO_A_B_SLOT_TO_WRITE
            .into(),
        Refusal::Missing { slot, partition } => {
            let slot = slot.letter();
            crate::t!(S_SLOT_HAS_NO_PARTITION_2, slot = slot, partition = partition)
        }
        Refusal::NotAarch64 => S_THIS_PHONES_ARCHITECTURE_ISNT_SUPPORTED
            .into(),
        Refusal::Battery { level, .. } => crate::t!(S_ONLY_CHARGE, level = level),
        Refusal::NotBootImage { slot, partition } => {
            let slot = slot.letter();
            crate::t!(S_SLOT_S_PARTITION_ISNT_A_BOOT_IMAGE_THAT_CAN_BE_PATCHED, slot = slot, partition = partition)
        }
        Refusal::RecoveryRamdisk { slot, partition } => {
            let slot = slot.letter();
            crate::t!(S_SLOT_S_PARTITION_HOLDS_A_RECOVERYS_RAMDISK, slot = slot, partition = partition)
        }
        Refusal::NoKsud(why) => crate::t!(S_THE_MANAGER_CARRIES_NO_KSUD_THIS_RECOVERY_CAN_RUN, why = why),
        Refusal::Damaged => S_THE_MANAGERS_APK_DOES_NOT_HAVE_THE_SHA_256_ITS_RELEASE_PUBLISHED
            .into(),
        Refusal::StorageClosed => S_INTERNAL_STORAGE_IS_CLOSED_SO_THE_MANAGER_HAS_NOWHERE_TO_GO
            .into(),
        Refusal::NoRoom { need, have } => {
            let need = fmt::bytes(*need);
            let have = fmt::bytes(*have);
            crate::t!(S_NOT_ENOUGH_ROOM_IT_NEEDS_AND_HAS, need = need, have = have)
        }
        Refusal::NoRelease { flavor, tier } => {
            let name = flavor.name();
            let tier = match tier {
                Tier::Stable => S_STABLE_RELEASE,
                Tier::Latest => S_LATEST_RELEASE,
            };
            crate::t!(S_HAS_NO_TO_TAKE_CHOOSE_ANOTHER_RELEASE, name = name, tier = tier)
        }
        Refusal::NoManager => S_NO_MANAGER_OF_THE_KERNELSU_FAMILY_IS_INSTALLED_IN_ANDROID
            .into(),
    }
}

/// How tall the card that holds a key is.
#[cfg(feature = "roots")]
fn key_card_height(t: &Theme, f: &Fonts) -> f32 {
    let (top, bottom) = f.mono.ink(t.m.dp(20.0));
    (bottom - top) + t.m.dp(40.0)
}

/// The key a run made, in a card of its own.
#[cfg(feature = "roots")]
fn key_card(c: &mut Canvas, t: &Theme, f: &Fonts, y: f32, key: &str) -> f32 {
    let m = &t.m;
    let h = key_card_height(t, f);
    c.fill_round_rect(m.margin, y, m.w as f32 - 2.0 * m.margin, h, m.card_radius, t.pal.card);
    key_line(c, t, f, y + m.dp(20.0), key);
    y + h
}

/// A key on a line of its own, in the face whose characters are all one width,
/// large enough to read off the screen and type back.
#[cfg(feature = "roots")]
fn key_line(c: &mut Canvas, t: &Theme, f: &Fonts, y: f32, key: &str) -> f32 {
    let m = &t.m;
    let px = m.dp(20.0);
    let (top, bottom) = f.mono.ink(px);
    c.text_top(&f.mono, key, m.margin + m.inner, y, px, t.pal.text, None);
    y + bottom - top
}

/// Whether nothing stands between the page and a run: a slot to write, the
/// owner's storage for the backups, and charge enough to write it.
#[cfg(feature = "roots")]
fn ready_to_apatch(cx: &Ctx) -> bool {
    let enough_charge = cx.info.battery.as_ref().and_then(skyward_install::battery_refusal).is_none();
    cx.info.slot.is_some() && cx.state.data.storage_open() && enough_charge
}

/// What the page says in its window before a run: what stands in the way, or
/// that nothing does.
#[cfg(feature = "roots")]
fn apatch_notes(_l: Lang, cx: &Ctx, root: &crate::app::ApatchState) -> Vec<String> {
    let mut notes = Vec::new();
    // An image that carries the patch already stops a run before it writes;
    // it is said here, for each slot the page would write, where the page
    // knows it, not asked about.
    if let Some(current) = cx.info.slot {
        for slot in root.pick.slots(Some(current)) {
            if root.patched_slots[slot.number() as usize] == Some(true) {
                let slot = slot.letter();
                notes.push(crate::t!(S_SLOT_S_IMAGE_CARRIES_THE_PATCH_ALREADY, slot = slot));
            }
        }
    }
    if cx.info.slot.is_none() {
        notes.push(S_THIS_PHONE_HAS_ONE_SLOT_SO_THERE_ARE_NO_BOOT_PARTITIONS_TO_WRITE.to_string());
    }
    if !cx.state.data.storage_open() {
        notes.push(S_OPEN_THE_DATA_PARTITION_IN_DECRYPT_FIRST_THE_BACKUPS_ARE_SAVED_TO_INTERNAL_STORAGE.to_string());
    }
    if let Some(skyward_install::Refusal::Battery { level, .. }) = cx.info.battery.as_ref().and_then(skyward_install::battery_refusal) {
        notes.push(crate::t!(S_ONLY_CHARGE_CHARGE_THE_PHONE_FIRST, level = level));
    } else if cx.info.slot.is_some() && cx.state.data.storage_open() {
        let slot = cx.info.slot.map(|s| s.letter()).unwrap_or_default();
        notes.push(crate::t!(S_READY_SLOT_S_BOOT_PARTITION_IS_BACKED_UP_BEFORE_IT_IS_WRITTEN, slot = slot));
    }
    if cx.info.snapshot != skyward_vab::SnapshotState::None {
        notes.push(S_AN_UPDATE_IS_STILL_IN_FLIGHT_THE_PATCH_MAY_BE_OVERWRITTEN_BEFORE_IT_FINISHES.to_string());
    }
    notes.push(S_THE_PATCH_BYPASSES_THE_SYSTEMS_OWN_CHECKS_A_FAILED_WRITE_CAN_LEAVE_THE_PHONE_UNABLE_TO_START.to_string());
    notes
}

/// The run's lines in a window that keeps the room it is given, so that it is
/// the same window from the first line to the last: the newest at its foot,
/// older ones falling off the top, wrapped rather than cut.
#[cfg(feature = "roots")]
fn log_window(c: &mut Canvas, t: &Theme, f: &Fonts, top: f32, room: f32, lines: &[String]) -> f32 {
    let m = &t.m;
    let (pitch, pad) = (m.dp(widgets::OUTPUT_PITCH_DP), m.dp(widgets::OUTPUT_TOP_DP));
    let x = m.margin;
    let width = m.w as f32 - m.margin * 2.0;
    let h = room.max(pad * 2.0 + pitch * 3.0);
    c.fill_round_rect(x, top, width, h, m.card_radius, t.pal.card);
    let text_width = width - 2.0 * m.inner;
    let mut wrapped: Vec<String> = lines.iter().flat_map(|line| widgets::wrap(c, &f.mono, line, m.sub_px, text_width)).collect();
    let fits = ((h - 2.0 * pad) / pitch).floor().max(1.0) as usize;
    if wrapped.len() > fits {
        wrapped.drain(..wrapped.len() - fits);
    }
    let mut foot = top + h - pad;
    for line in wrapped.iter().rev() {
        foot -= pitch;
        c.text_top(&f.mono, line, x + m.inner, foot, m.sub_px, t.pal.subtitle, None);
    }
    top + h
}

/// The superkey page: the key typed on the screen's keyboard once that is the
/// key chosen, and otherwise the key this run stands to use, to look at. The
/// key an earlier run made is here in both, under the field.
#[cfg(feature = "roots")]
pub fn apatch_key(cx: &Ctx) -> Frame {
    use crate::app::KeyMode;
    let (t, f, s, l) = (cx.t, cx.f, cx.state, cx.state.lang);
    let mut c = page(cx);
    let mut out = Vec::new();
    let top = t.m.content_top;
    let root = s.apatch.clone().unwrap_or_default();
    let title = APATCH_KEY_TITLE;
    let last = root.last.as_ref().map(|key| key.expose().to_string());
    if root.key_mode == KeyMode::Enter {
        let (y, caret) = widgets::text_field(&mut c, t, f, top, &root.entered);
        let y = widgets::footnote(&mut c, t, f, y, S_K_8_TO_63_LETTERS_AND_DIGITS_AT_LEAST_ONE_OF_EACH_TAP_DONE_TO_KEEP_IT);
        if let Some(key) = &last {
            let caption = S_THE_KEY_FROM_THE_LAST_RUN;
            let y = widgets::footnote(&mut c, t, f, y, caption);
            key_line(&mut c, t, f, y + t.m.dp(4.0), key);
        }
        keyboard_hits(&mut c, t, f, s, title_label(l), &mut out);
        return Frame { caret: Some(caret), ..Frame::new(t, c, out, title, top) };
    }
    // Not the key being typed: the key the run would use is here to read.
    // Which key that is, is chosen on the page this one comes from.
    let y = widgets::footnote(&mut c, t, f, top, S_THE_KEY_THIS_RUN_WOULD_USE);
    match (&root.key_mode, &last) {
        (KeyMode::Last, Some(key)) => {
            key_line(&mut c, t, f, y + t.m.dp(4.0), key);
        }
        _ => {
            widgets::footnote(&mut c, t, f, y, S_IT_IS_MADE_WHEN_THE_RUN_STARTS_SO_THERE_IS_NONE_YET);
        }
    }
    Frame::new(t, c, out, title, top)
}

/// What the key page's keyboard says on its last key.
#[cfg(feature = "roots")]
fn title_label(_l: Lang) -> &'static str {
    DONE
}

/// The heading and the line under it a run's stage says, on its card.
#[cfg(feature = "roots")]
pub(crate) fn apatch_stage(_l: Lang, stage: Option<&skyward_root::folkpatch::Stage>) -> (String, String) {
    use skyward_root::folkpatch::Stage;
    let bytes = |done: u64, total: u64| crate::t!(S_OF_BYTES, done = done, total = total);
    match stage {
        None => (S_GETTING_READY.into(), String::new()),
        Some(Stage::Checking) => (S_CHECKING.into(), String::new()),
        Some(Stage::BackingUp { slot, done, total }) => {
            let slot = slot.letter();
            (
                crate::t!(S_SAVING_SLOT_S_BOOT_PARTITION, slot = slot),
                bytes(*done, *total),
            )
        }
        Some(Stage::Patching { slot }) => {
            let slot = slot.letter();
            (crate::t!(S_PATCHING_SLOT, slot = slot), String::new())
        }
        Some(Stage::Verifying { slot }) => {
            let slot = slot.letter();
            (crate::t!(S_CHECKING_SLOT_S_PATCH_2, slot = slot), String::new())
        }
        Some(Stage::Flashing { slot, done, total }) => {
            let slot = slot.letter();
            (crate::t!(S_WRITING_SLOT, slot = slot), bytes(*done, *total))
        }
    }
}

/// How a run ended: the line the window ends with, the key it made where it
/// made one, and what else the page has to say.
#[cfg(feature = "roots")]
fn apatch_result(l: Lang, outcome: &skyward_root::folkpatch::Outcome) -> (String, Option<String>, Vec<String>) {
    match outcome {
        skyward_root::folkpatch::Outcome::Installed(installed) => {
            let slots: Vec<&str> = installed.slots.iter().map(|s| s.letter()).collect();
            let slots = slots.join(", ");
            let folder = installed.backup_dir.parent().map_or_else(|| installed.backup_dir.display().to_string(), |p| p.display().to_string());
            let mut notes = vec![crate::t!(S_THE_BOOT_IMAGE_FROM_BEFORE_THE_WRITE_IS_IN_IN_ITS_BACKUP_FOLDER, folder = folder)];
            if !installed.kallsyms_all {
                notes.push(
                    S_THE_KERNEL_HAS_NO_CONFIG_KALLSYMS_ALL_SOME_OF_WHAT_ROOT_RUNS_MAY_NOT_WORK_RESTORE_THE_BACKUP_IF_IT_DOES_NOT_START
                    .to_string(),
                );
            }
            let summary = crate::t!(S_KERNELPATCH_INSTALLED_SLOT_IS_PATCHED, slots = slots);
            (summary, Some(installed.key.expose().to_string()), notes)
        }
        skyward_root::folkpatch::Outcome::Refused(refusal) => {
            let why = apatch_refusal(l, refusal);
            let summary = crate::t!(S_NOT_INSTALLED, why = why);
            let nothing = S_NO_BOOT_PARTITION_WAS_WRITTEN.to_string();
            (summary, None, vec![nothing])
        }
        skyward_root::folkpatch::Outcome::Failed(failure) => {
            let (heading, _) = apatch_stage(l, Some(&failure.stage));
            let why = &failure.why;
            let mut notes = Vec::new();
            if !failure.written.is_empty() {
                let slots: Vec<&str> = failure.written.iter().map(|s| s.letter()).collect();
                let slots = slots.join(", ");
                notes.push(crate::t!(S_SLOT_MAY_BE_PARTLY_WRITTEN_DONT_START_IT, slots = slots));
            }
            for copy in &failure.backups {
                let path = copy.path.display().to_string();
                let boot = format!("{}{}", copy.partition, copy.slot.suffix());
                notes.push(crate::t!(S_FROM_A_COMPUTER_FASTBOOT_FLASH_2, boot = boot, path = path));
            }
            let summary = crate::t!(S_NOT_INSTALLED_FAILED_WHILE, heading = heading, why = why);
            (summary, None, notes)
        }
    }
}

/// Why a run did not start, or stopped before writing, in the user's words.
#[cfg(feature = "roots")]
fn apatch_refusal(_l: Lang, refusal: &skyward_root::folkpatch::Refusal) -> String {
    use skyward_root::folkpatch::Refusal;
    match refusal {
        Refusal::NoSlot => S_THIS_PHONE_HAS_NO_A_B_SLOT_TO_WRITE
            .into(),
        Refusal::Missing(s) => {
            let slot = s.letter();
            crate::t!(S_SLOT_HAS_NO_BOOT_PARTITION, slot = slot)
        }
        Refusal::NotAarch64 => S_THIS_PHONES_ARCHITECTURE_ISNT_SUPPORTED
            .into(),
        Refusal::Battery { level, .. } => crate::t!(S_ONLY_CHARGE, level = level),
        Refusal::NotBootImage(s) => {
            let slot = s.letter();
            crate::t!(S_SLOT_S_BOOT_PARTITION_ISNT_A_BOOT_IMAGE_THAT_CAN_BE_PATCHED, slot = slot)
        }
        Refusal::RecoveryRamdisk(s) => {
            let slot = s.letter();
            crate::t!(S_SLOT_S_BOOT_PARTITION_HOLDS_A_RECOVERYS_RAMDISK, slot = slot)
        }
        Refusal::AlreadyPatched(s) => {
            let slot = s.letter();
            crate::t!(S_SLOT_S_IMAGE_IS_PATCHED_ALREADY_CHOOSE_ANOTHER_SLOT_OR_RESTORE_THAT_SLOT_FROM_THE_BACKUP_FIRST, slot = slot)
        }
        Refusal::NoKallsyms => S_THE_KERNEL_HAS_NO_CONFIG_KALLSYMS_WHICH_FOLKPATCH_NEEDS
            .into(),
        Refusal::NoTools(what) => {
            let what = what.clone();
            crate::t!(S_THE_RECOVERY_HAS_NO, what = what)
        }
        Refusal::StorageClosed => S_INTERNAL_STORAGE_IS_CLOSED_SO_THE_MANAGER_AND_THE_KEY_HAVE_NOWHERE_TO_GO
            .into(),
        Refusal::NoRoom { need, have } => {
            let need = fmt::bytes(*need);
            let have = fmt::bytes(*have);
            crate::t!(S_NOT_ENOUGH_ROOM_IT_NEEDS_AND_HAS, need = need, have = have)
        }
        Refusal::Key(fault) => fault.why().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(restore: bool, checks: bool) -> BackupJob {
        let plan = vec![("vendor_boot".into(), 200), ("system".into(), 800)];
        BackupJob { restore, place: Place::Storage, checks, plan, at: None, outcome: None, stopping: false }
    }

    fn at(item: usize, done: u64, of: u64, checking: bool) -> BackupAt {
        BackupAt { item, done, of, checking }
    }


    #[test]
    fn a_backup_checks_its_parts_last_and_a_restore_first() {
        // Halfway through writing system, 600 of the 1000 bytes: the first
        // of two passes, then the second.
        assert_eq!(job_progress(&job(false, true), &at(1, 400, 800, false)), (0.3, 600));
        assert_eq!(job_progress(&job(false, true), &at(1, 400, 800, true)), (0.8, 600));
        assert_eq!(job_progress(&job(false, false), &at(1, 400, 800, false)), (0.6, 600), "without a check, writing is all");
        assert_eq!(job_progress(&job(true, true), &at(0, 100, 200, true)), (0.05, 100));
        assert_eq!(job_progress(&job(true, true), &at(0, 100, 200, false)), (0.55, 100));
    }



    #[test]
    fn a_phrase_that_opens_a_line_starts_with_a_capital_in_english() {
        assert_eq!(capitalized(Lang::En, "empties the data partition"), "Empties the data partition");
        assert_eq!(capitalized(Lang::En, "Wipe data"), "Wipe data");
    }

    #[cfg(feature = "roots")]
    #[test]
    fn a_kernelsu_run_says_where_the_manager_and_the_copies_went() {
        let outcome = skyward_root::ksu::Outcome::Installed(skyward_root::ksu::Installed {
            slots: vec![skyward_vab::Slot::A],
            partitions: vec!["init_boot"],
            flavor: skyward_root::ksu::Flavor::BakaSu,
            tag: "v4.2.0-rc3".into(),
            apk: "/sdcard/Download/Skyward/KSU/1120/manager.apk".into(),
            apk_sha256: "00".into(),
            ksud_sha256: "11".into(),
            backup_dir: "/sdcard/Download/Skyward/KSU/1120/backup".into(),
            run_dir: "/sdcard/Download/Skyward/KSU/1120".into(),
        });
        let (summary, notes) = ksu_result(Lang::En, &outcome);
        let text = notes.join("\n");
        assert_eq!(summary, "BakaSU installed: slot A init_boot is patched.");
        assert!(text.contains("manager.apk"), "{text}");
        assert!(text.contains("/sdcard/Download/Skyward/KSU/1120"), "{text}");
        assert!(text.contains("unsigned"), "{text}");
    }

    #[cfg(feature = "roots")]
    #[test]
    fn a_root_run_that_installed_says_the_key_and_where_the_manager_went() {
        let key = skyward_root::folkpatch::SuperKey::validate("abcd1234").unwrap();
        let outcome = skyward_root::folkpatch::Outcome::Installed(skyward_root::folkpatch::Installed {
            slots: vec![skyward_vab::Slot::A],
            key: key.clone(),
            key_path: "/data/local/tmp/skyward-root/1120/superkey.txt".into(),
            backup_dir: "/sdcard/Download/Skyward/FolkPatch/1120/backup".into(),
            kallsyms_all: true,
        });
        let (summary, key_line, notes) = apatch_result(Lang::En, &outcome);
        let text = notes.join("\n");
        assert_eq!(summary, "KernelPatch installed: slot A is patched.");
        assert_eq!(key_line.as_deref(), Some(key.expose()), "the key has a line of its own");
        assert!(text.contains("/sdcard/Download/Skyward/FolkPatch/1120"), "{text}");
        assert!(!text.contains("KALLSYMS_ALL"), "a kernel with every symbol has nothing to warn of");
    }

    #[cfg(feature = "roots")]
    #[test]
    fn a_root_run_that_stopped_says_what_may_be_written_and_how_to_put_it_back() {
        let outcome = skyward_root::folkpatch::Outcome::Failed(skyward_root::folkpatch::Failure {
            stage: skyward_root::folkpatch::Stage::Flashing { slot: skyward_vab::Slot::A, done: 0, total: 0 },
            slot: Some(skyward_vab::Slot::A),
            why: "no space left on device".into(),
            backups: vec![skyward_root::backup::Backup {
                slot: skyward_vab::Slot::A,
                partition: "boot".into(),
                path: "/sdcard/Download/Skyward/FolkPatch/1120/backup/boot_a.img".into(),
                size: 96 << 20,
                sha256: "00".into(),
            }],
            written: vec![skyward_vab::Slot::A],
            key_path: None,
        });
        let (summary, key_line, notes) = apatch_result(Lang::En, &outcome);
        let text = notes.join("\n");
        assert!(summary.starts_with("Not installed:"), "{summary}");
        assert!(summary.contains("no space left on device"), "{summary}");
        assert!(key_line.is_none(), "a run that stopped made no key to show");
        assert!(text.contains("don't start it"), "{text}");
        assert!(text.contains("fastboot flash boot_a /sdcard/Download/Skyward/FolkPatch/1120/backup/boot_a.img"), "{text}");
    }
}
