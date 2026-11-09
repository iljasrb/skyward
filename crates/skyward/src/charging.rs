//! Keeping to the charge limit the phone's own system keeps, while the
//! recovery runs.
//!
//! The system enforces its limit from its own services, which a recovery does
//! not run: a phone booted into the recovery charges to the battery's full
//! whatever the system's limit says. The kernel takes a switch for it, which
//! the phone's configuration names, and the system keeps the limit itself in
//! a settings database, which is read here.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// How far below the limit the battery falls before charging starts again,
/// so that it does not stop and start at every percent.
const HYSTERESIS: u8 = 3;

/// The limit the phone's charging page shows and keeps until the slider on it
/// is moved, which is the only time the settings database gets a limit of its
/// own: 80, as the page shows it.
const DEFAULT_LIMIT: u8 = 80;

/// The kernel switch that stops and starts charging: 0 stops it, 1 starts it.
pub struct Switch {
    path: PathBuf,
}

impl Switch {
    pub fn new(path: impl Into<PathBuf>) -> Switch {
        Switch { path: path.into() }
    }

    /// Keep to `limit`: stop charging at it, and start again a few percent
    /// below. `None` starts charging and keeps it going.
    pub fn keep(&mut self, level: u8, limit: Option<u8>) -> Result<()> {
        match limit {
            Some(limit) if level >= limit => self.write(true),
            Some(limit) if level.saturating_add(HYSTERESIS) <= limit => self.write(false),
            Some(_) => Ok(()),
            None => self.write(false),
        }
    }

    /// Write the switch unless it already says so. What the node says now is
    /// what counts: it can be written by the phone's own programs too, and a
    /// switch remembered as set is not a battery that is not charging.
    fn write(&mut self, stop: bool) -> Result<()> {
        let wanted = if stop { "0" } else { "1" };
        if fs::read_to_string(&self.path).is_ok_and(|now| now.trim() == wanted) {
            return Ok(());
        }
        fs::write(&self.path, wanted).with_context(|| format!("write {}", self.path.display()))
    }
}

/// The limit the phone's own system keeps, where it has one: a LineageOS
/// settings database, whose `system` table holds `charging_control_enabled`
/// and `charging_control_charging_limit`.
///
/// The limit row is only written once the slider on the phone's charging
/// page is moved: until then the page shows [`DEFAULT_LIMIT`] and the phone
/// keeps to that, so a phone with charging control on and no limit written
/// down keeps the default rather than none.
pub fn system_limit(db: &Path) -> Option<u8> {
    let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let value = |name: &str| -> Option<String> {
        conn.query_row("select value from system where name = ?1", [name], |row| row.get(0)).ok()
    };
    if value("charging_control_enabled").as_deref() != Some("1") {
        return None;
    }
    match value("charging_control_charging_limit") {
        Some(text) => text.parse().ok(),
        None => Some(DEFAULT_LIMIT),
    }
}

/// The limit kept from the last time the data partition was open, where the
/// phone's configuration names a file for it: none while the file is missing,
/// and none while it says the system keeps none.
pub fn cached(path: &Path) -> Option<u8> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Keep `limit` for the next start, or that the system keeps none.
pub fn cache(path: &Path, limit: Option<u8>) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let text = limit.map_or_else(|| "off".to_string(), |limit| limit.to_string());
    fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_stops_at_the_limit_and_starts_below_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("charging_enabled");
        fs::write(&path, "1").unwrap();
        let mut switch = Switch::new(&path);
        let read = || fs::read_to_string(&path).unwrap();
        switch.keep(77, Some(78)).unwrap();
        assert_eq!(read(), "1", "below the limit");
        switch.keep(78, Some(78)).unwrap();
        assert_eq!(read(), "0", "at the limit");
        switch.keep(76, Some(78)).unwrap();
        assert_eq!(read(), "0", "within the hysteresis");
        switch.keep(75, Some(78)).unwrap();
        assert_eq!(read(), "1", "below the hysteresis");
        switch.keep(100, None).unwrap();
        assert_eq!(read(), "1", "no limit");
    }

    #[test]
    fn a_switch_changed_behind_its_back_is_set_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("charging_enabled");
        fs::write(&path, "1").unwrap();
        let mut switch = Switch::new(&path);
        switch.keep(90, Some(80)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "0", "at the limit");
        fs::write(&path, "1").unwrap();
        switch.keep(90, Some(80)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "0", "written back where the limit holds");
    }

    #[test]
    fn the_limit_kept_is_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("skyward/charge-limit");
        assert_eq!(cached(&path), None, "nothing kept yet");
        cache(&path, Some(78)).unwrap();
        assert_eq!(cached(&path), Some(78));
        cache(&path, None).unwrap();
        assert_eq!(cached(&path), None, "the system keeps none");
    }

    #[test]
    fn a_limit_never_moved_keeps_the_pages_default() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("lineagesettings.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "create table system (name text, value text);
             insert into system values ('charging_control_enabled', '1');
             insert into system values ('charging_control_mode', '3');",
        )
        .unwrap();
        assert_eq!(system_limit(&db), Some(DEFAULT_LIMIT), "as the page shows it");
        conn.execute("update system set value = '0' where name = 'charging_control_enabled'", []).unwrap();
        assert_eq!(system_limit(&db), None, "the limit is off");
    }

    #[test]
    fn a_database_without_a_limit_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("lineagesettings.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "create table system (name text, value text);
             insert into system values ('charging_control_enabled', '1');
             insert into system values ('charging_control_charging_limit', '78');",
        )
        .unwrap();
        assert_eq!(system_limit(&db), Some(78));
        conn.execute("update system set value = '0' where name = 'charging_control_enabled'", []).unwrap();
        assert_eq!(system_limit(&db), None, "the limit is off");
        assert_eq!(system_limit(&dir.path().join("missing.db")), None);
    }
}
