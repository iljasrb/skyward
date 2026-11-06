use crate::app::Screen;
use crate::lang::{Lang, Text};
use crate::strings::*;

/// A feature or partition the search finds, and the page it opens.
pub struct Entry {
    pub title: Text,
    /// Where it lives, shown under the title.
    pub place: Text,
    pub screen: Screen,
    /// What it answers to besides its title: pinyin in full and by
    /// initials, and English, since the keyboard types Latin letters.
    pub keys: &'static [&'static str],
}

const fn entry(title: Text, place: Text, screen: Screen, keys: &'static [&'static str]) -> Entry {
    Entry { title, place, screen, keys }
}

/// A partition, named the same in every language.
const fn partition(name: &'static str, place: Text, screen: Screen) -> Entry {
    Entry { title: name, place, screen, keys: &[] }
}

const HOME: Text = S_HOME;
const INSTALL: Text = S_INSTALL_2;
const ADVANCED: Text = S_ADVANCED;
const SETTINGS: Text = S_SETTINGS;
const MOUNT: Text = S_MOUNT;

pub const ENTRIES: &[Entry] = &[
    entry("Wi-Fi", HOME, Screen::Wlan, &["wifi", "wlan"]),
    entry(INSTALL, HOME, Screen::Install, &["anzhuang", "az", "install", "shuaji", "sj"]),
    entry(
        S_SYSTEM_UPDATE_2,
        INSTALL,
        Screen::Install,
        &["xitonggengxin", "xtgx", "hebing", "hb", "merge", "user", "ota"],
    ),
    entry(S_INSTALL_FROM_INTERNAL_STORAGE, INSTALL, Screen::Install, &["congneibucunchuanzhuang", "cnbccaz", "neicun", "sdcard", "zip"]),
    entry(S_INSTALL_FROM_A_USB_DRIVE, INSTALL, Screen::Install, &["congupananzhuang", "cupaz", "upan", "usb", "otg"]),
    entry(S_INSTALL_FROM_A_COMPUTER, INSTALL, Screen::Install, &["tongguodiannaoanzhuang", "tgdnaz", "sideload", "adb"]),
    entry(S_BACKUP, HOME, Screen::Backup, &["beifen", "bf", "backup"]),
    entry(S_RESTORE, HOME, Screen::Restore, &["huifu", "hf", "restore"]),
    entry(MOUNT, HOME, Screen::Mount, &["guazai", "gz", "mount", "umount"]),
    entry(S_WIPE_DATA, HOME, Screen::Wipe, &["qingchushuju", "qcsj", "wipe", "format", "geshihua", "gsh"]),
    entry(ADVANCED, HOME, Screen::Advanced, &["gaoji", "gj", "advanced"]),
    entry(S_FILE_MANAGER, ADVANCED, Screen::Files, &["wenjianguanli", "wjgl", "files", "file"]),
    entry(S_TERMINAL, ADVANCED, Screen::Terminal, &["zhongduan", "zd", "terminal", "shell"]),
    entry(
        S_FIX_SELINUX_CONTEXTS,
        ADVANCED,
        Screen::Relabel,
        &["xiufuselinuxshangxiawen", "xfsxw", "xiufu", "xf", "shangxiawen", "context", "restorecon", "label"],
    ),
    entry(
        S_FILESYSTEMS,
        ADVANCED,
        Screen::Filesystems,
        &["wenjianxitong", "wjxt", "genghuan", "gh", "fsck", "filesystem", "f2fs", "ext4"],
    ),
    entry(S_EXPORT_LOGS, ADVANCED, Screen::Logs, &["daochurizhi", "dcrz", "rizhi", "rz", "logs", "dmesg"]),
    entry(
        S_UPDATE,
        ADVANCED,
        Screen::Update,
        &["recoverygengxin", "rcgx", "gengxin", "gx", "skyward", "recovery", "update", "xiazai", "xz", "download"],
    ),
    #[cfg(feature = "roots")]
    entry(
        "KernelPatch (FolkPatch)",
        HOME,
        Screen::Apatch,
        &["root", "folkpatch", "apatch", "kpatch", "kernelpatch", "tiquan", "superkey"],
    ),
    #[cfg(feature = "roots")]
    entry(
        S_KERNELSU_FAMILY,
        HOME,
        Screen::Ksu,
        &["root", "kernelsu", "ksu", "sukisu", "bakasu", "resukisu", "kernelsunext", "manager", "tiquan"],
    ),
    entry(SETTINGS, HOME, Screen::Settings, &["shezhi", "sz", "settings"]),
    entry(S_DARK_MODE, SETTINGS, Screen::Settings, &["shensemoshi", "ssms", "dark", "theme"]),
    entry(S_SCREEN_BRIGHTNESS, SETTINGS, Screen::Settings, &["pingmuliangdu", "pmld", "liangdu", "ld", "brightness"]),
    entry(S_SCREEN_TIMEOUT, SETTINGS, Screen::ScreenOff, &["xipingshijian", "xpsj", "timeout"]),
    entry(S_VIBRATION_INTENSITY, SETTINGS, Screen::Settings, &["zhendongqiangdu", "zdqd", "qiangdu", "qd", "intensity", "strength"]),
    entry(S_TOUCH_VIBRATION, SETTINGS, Screen::Settings, &["chumozhendong", "cmzd", "zhendong", "vibrate", "haptic"]),
    entry(S_FILE_TRANSFER, SETTINGS, Screen::Settings, &["wenjianchuanshu", "wjcs", "chuanshu", "mtp", "usb"]),
    entry(S_TIME_ZONE, SETTINGS, Screen::TimeZone, &["shiqu", "sq", "timezone"]),
    entry(S_REBOOT, HOME, Screen::Reboot, &["chongqi", "cq", "reboot", "fastboot", "bootloader", "edl", "guanji"]),
    entry(S_BOOT_SLOT, S_REBOOT, Screen::Slot, &["qidongcaowei", "qdcw", "caowei", "cw", "slot"]),
    entry(S_ABOUT_PHONE, SETTINGS, Screen::About, &["guanyubenji", "gybj", "about", "banben", "version"]),
    entry(
        S_ALL_SPECS,
        S_SETTINGS_ABOUT_PHONE,
        Screen::Specs,
        &["quanbucanshuyuxinxi", "qbcsyxx", "canshu", "specs"],
    ),
    entry(
        S_OPEN_SOURCE_LICENSES,
        S_SETTINGS_ABOUT_PHONE_ALL_SPECS,
        Screen::License,
        &["kaiyuanxuke", "kyxk", "license", "gpl"],
    ),
    entry(
        S_UI_FONTS,
        S_SETTINGS_ABOUT_PHONE_ALL_SPECS,
        Screen::FontLicense,
        &["jiemianziti", "jmzt", "ziti", "font", "misans", "kirkmono", "dengkuan"],
    ),
    partition("vendor_boot", INSTALL, Screen::Install),
    partition("vendor_boot", INSTALL, Screen::Install),
    partition("vendor_boot", INSTALL, Screen::Install),
    partition("recovery", INSTALL, Screen::Install),
    partition("dtbo", INSTALL, Screen::Install),
    partition("vbmeta", INSTALL, Screen::Install),
    partition("system", MOUNT, Screen::Mount),
    partition("system_ext", MOUNT, Screen::Mount),
    partition("product", MOUNT, Screen::Mount),
    partition("vendor", MOUNT, Screen::Mount),
    partition("odm", MOUNT, Screen::Mount),
    partition("vendor_dlkm", MOUNT, Screen::Mount),
    partition("system_dlkm", MOUNT, Screen::Mount),
    partition("odm_dlkm", MOUNT, Screen::Mount),
];

/// The entries `query` finds, best first: a title in `l` holding it, then a
/// key starting with it, then a key holding it.
pub fn find(query: &str, _l: Lang) -> Vec<&'static Entry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let rank = |e: &Entry| {
        if e.title.to_lowercase().contains(&q) {
            Some(0)
        } else if e.keys.iter().any(|k| k.starts_with(&q)) {
            Some(1)
        } else if e.keys.iter().any(|k| k.contains(&q)) {
            Some(2)
        } else {
            None
        }
    };
    let mut found: Vec<(u8, usize, &Entry)> = ENTRIES.iter().enumerate().filter_map(|(k, e)| rank(e).map(|r| (r, k, e))).collect();
    found.sort_by_key(|&(r, k, _)| (r, k));
    found.into_iter().map(|(_, _, e)| e).collect()
}

/// What [`find`] finds that the phone has: a phone whose WLAN the recovery
/// does not bring up has no WLAN page.
pub fn find_on(query: &str, l: Lang, wlan: bool) -> Vec<&'static Entry> {
    find(query, l).into_iter().filter(|e| wlan || e.screen != Screen::Wlan).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(query: &str) -> Vec<&'static str> {
        find(query, Lang::En).iter().map(|e| e.title).collect()
    }

    #[test]
    fn english_finds_a_feature() {
        assert_eq!(titles("backup"), ["Backup"]);
        assert_eq!(titles("Backup"), ["Backup"]);
        assert_eq!(titles("timeout"), ["Screen timeout"]);
    }


    #[test]
    fn the_filesystem_page_is_found_by_what_it_does() {
        assert_eq!(titles("fsck"), ["Filesystems"]);
        assert_eq!(titles("filesystem"), ["Filesystems"]);
    }

    #[test]
    fn the_wlan_is_found_where_the_phone_has_it() {
        assert_eq!(titles("wifi"), ["Wi-Fi"]);
        assert_eq!(find_on("wlan", Lang::En, true).iter().map(|e| e.title).collect::<Vec<_>>(), ["Wi-Fi"]);
        assert!(find_on("wlan", Lang::En, false).is_empty());
        assert_eq!(titles("download"), ["Update"]);
    }

    #[test]
    fn nothing_typed_finds_nothing() {
        assert!(find("", Lang::En).is_empty());
        assert!(find("   ", Lang::En).is_empty());
        assert!(find("zzzz", Lang::En).is_empty());
    }
}
