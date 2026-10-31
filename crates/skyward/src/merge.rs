//! Merging an update into the slot the phone runs from, as libsnapshot
//! follows a merge: started where it has not begun, then looked at every
//! two seconds until it ends, the bootloader told where it stands through
//! the phone's own boot control. On a thread of its own for the screen.

use anyhow::{Context, Result};
use skyward_binder::driver::Binder;
use skyward_sys::worker;
use skyward_vab::boot::BootControl;
use skyward_vab::{Merge, Slot};
use std::thread;
use std::time::Duration;

/// How often a merge under way is looked at, as libsnapshot looks.
const LOOK_EVERY: Duration = Duration::from_secs(2);

/// Merge the update into `slot`, starting the merge first where `start`,
/// and follow it to its end, `each` told how every look at it went.
/// /metadata is writable while it runs, and read-only again after.
pub fn run(slot: Slot, start: bool, each: &mut dyn FnMut(&Merge)) -> Result<Merge> {
    let (binder, hardware) = (Binder::open()?, Binder::hardware()?);
    let control = BootControl::open(&binder, &hardware)?;
    let tell = |status| control.set_merge_status(status);
    skyward_vab::metadata_writable(true).context("make /metadata writable to merge the update")?;
    let merged = (|| -> Result<Merge> {
        if start {
            skyward_vab::start_merge(slot, &tell)?;
        }
        loop {
            let merge = skyward_vab::check_merge(slot, &tell)?;
            each(&merge);
            if !matches!(merge, Merge::Going(_)) {
                return Ok(merge);
            }
            thread::sleep(LOOK_EVERY);
        }
    })();
    let back = skyward_vab::metadata_writable(false).context("make /metadata read-only again");
    let merge = merged?;
    back?;
    Ok(merge)
}

pub enum Event {
    /// How a look at the merge went.
    Looked(Merge),
    /// The merge could not be started or followed, and why.
    Failed(String),
}

/// A merge followed on a thread of its own.
pub struct Merger {
    pub worker: worker::Worker<Event>,
}

impl Merger {
    /// Follow the merge into `slot` as [`run`] does, starting it first where
    /// `start`.
    pub fn start(slot: Slot, start: bool) -> Result<Merger> {
        let worker = worker::Worker::start("merge", "the merger", move |tell| {
            if let Err(e) = run(slot, start, &mut |merge| tell.send(Event::Looked(*merge))) {
                tell.send(Event::Failed(format!("{e:#}")));
            }
        })?;
        Ok(Merger { worker })
    }
}
