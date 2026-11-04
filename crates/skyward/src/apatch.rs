//! FolkPatch's APatch, run off the page's thread: a run backs up whole
//! partitions, patches them and reads them back, which is long enough that the
//! page would stop moving under the finger if it were done there.
//!
//! The thread says how far the run has come as it goes, and how it ended;
//! nothing waits on it, so a page that ends without it is left to finish.

use anyhow::{Context, Result};
use skyward_root::folkpatch::{Outcome, Stage};
use std::sync::mpsc::{self, Receiver, TryIter};

/// What a run tells the page as it goes.
pub enum Told {
    Stage(Stage),
    Ended(Outcome),
}

pub struct Worker {
    rx: Receiver<Told>,
}

impl Worker {
    /// Start a run of `options` on a thread of its own.
    pub fn start(options: skyward_root::folkpatch::Options) -> Result<Worker> {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("folkpatch".into())
            .spawn(move || {
                let mut told = |stage: Stage| {
                    let _ = tx.send(Told::Stage(stage));
                };
                let outcome = skyward_root::folkpatch::install(&options, &mut told);
                let _ = tx.send(Told::Ended(outcome));
            })
            .context("start the apatch thread")?;
        Ok(Worker { rx })
    }

    /// What it has told since the last look.
    pub fn events(&self) -> TryIter<'_, Told> {
        self.rx.try_iter()
    }
}
