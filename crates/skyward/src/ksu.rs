//! A KernelSU-family manager's root, run off the page's thread: its release is
//! read, its APK downloaded and checked, and its `ksud` patches each slot,
//! which is long enough that the page would stop moving under the finger if it
//! were done there.
//!
//! The manager's APK is kept for Android to install; nothing of it is run as a
//! script. The thread says how far the run has come as it goes, and how it
//! ended.

use anyhow::{Context, Result};
use skyward_root::ksu::{self, Failure, Flavor, Outcome, Refusal, Source, Stage, Tier};
use skyward_info::Battery;
use skyward_net::download::{self, Progress};
use skyward_net::http::{self, Servers};
use skyward_sys::worker;
use skyward_vab::Slot;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// Where only root reads what the run keeps while it works.
const PRIVATE: &str = "/data/local/tmp/skyward-ksu";

/// The most often how far a download has come is told.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

/// What a run is asked to do besides the phone it is on.
pub struct Task {
    pub device: String,
    pub current: Slot,
    pub slots: Vec<Slot>,
    pub battery: Battery,
    pub flavor: Flavor,
    pub tier: Tier,
    pub source: Source,
    /// The manager already installed in Android, when the source is its APK.
    pub installed: Option<ksu::InstalledManager>,
    /// The KMI the phone's kernel loads, where its version names another.
    pub kmi: Option<String>,
    /// When the run was started, as `20261101-123456`.
    pub stamp: String,
}

/// What a run tells the page as it goes.
pub enum Told {
    Stage(Stage),
    Ended(Outcome),
}

pub struct Worker {
    worker: worker::Worker<Told>,
}

impl Worker {
    /// Start a run of `task` on a thread of its own.
    pub fn start(task: Task, servers: Servers) -> Result<Worker> {
        let worker = worker::Worker::start("ksu", "the KernelSU worker", move |tell| {
            let mut told = |stage: Stage| tell.send(Told::Stage(stage));
            let outcome = run(task, servers, &mut told);
            let _ = skyward_sys::kmsg(match &outcome {
                Outcome::Installed(_) => "ksu: the boot partitions are patched",
                Outcome::Refused(_) => "ksu: nothing was written",
                Outcome::Failed(_) => "ksu: the run did not finish",
            });
            tell.send(Told::Ended(outcome));
        })?;
        Ok(Worker { worker })
    }

    /// What it has told since the last look.
    pub fn events(&self) -> Vec<Told> {
        self.worker.events()
    }
}

/// What the manager's APK was, and which release it came from.
struct Got {
    apk: PathBuf,
    apk_sha256: Option<String>,
    flavor: Flavor,
    tag: String,
}

fn run(task: Task, servers: Servers, tell: &mut dyn FnMut(Stage)) -> Outcome {
    let private = Path::new(PRIVATE).join(&task.stamp);
    let outcome = attempt(&task, servers, tell, &private);
    // The patched image is in the run's folder on storage now; nothing under
    // /data is needed once the run has ended.
    let _ = fs::remove_dir_all(&private);
    outcome
}

fn attempt(task: &Task, servers: Servers, tell: &mut dyn FnMut(Stage), private: &Path) -> Outcome {
    let got = match acquire(task, servers, tell, private) {
        Ok(got) => got,
        Err(outcome) => return *outcome,
    };
    let options = ksu::Options {
        device: task.device.clone(),
        current: task.current,
        slots: task.slots.clone(),
        battery: task.battery.clone(),
        flavor: got.flavor,
        tag: got.tag,
        kmi: task.kmi.clone(),
        apk: got.apk,
        apk_sha256: got.apk_sha256,
        partition_dir: PathBuf::from("/dev/block/by-name"),
        storage: PathBuf::from(skyward_crypto::data::SDCARD),
        private: PathBuf::from(PRIVATE),
        stamp: task.stamp.clone(),
    };
    ksu::install(&options, tell)
}

/// Take the manager from where the page said: its release, downloaded and
/// checked, or the one Android already has.
fn acquire(task: &Task, servers: Servers, tell: &mut dyn FnMut(Stage), private: &Path) -> Result<Got, Box<Outcome>> {
    if task.source == Source::Installed {
        let installed = task.installed.clone().ok_or_else(|| Box::new(Outcome::Refused(Refusal::NoManager)))?;
        return Ok(Got { apk: installed.path, apk_sha256: None, flavor: installed.flavor, tag: installed.package });
    }
    tell(Stage::Finding);
    let failed = |stage: Stage, why: String| {
        Box::new(Outcome::Failed(Failure {
            stage,
            slot: None,
            why,
            backups: Vec::new(),
            written: Vec::new(),
            run_dir: None,
        }))
    };
    let agent = http::agent(servers);
    let url = ksu::release::listing(task.flavor);
    let mut response = agent
        .get(&url)
        .call()
        .map_err(|e| failed(Stage::Finding, format!("ask {url}: {e:#}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(failed(Stage::Finding, format!("{url} answered {status}")));
    }
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| failed(Stage::Finding, format!("read {url}: {e:#}")))?;
    let release = ksu::release::pick(&body, task.tier)
        .map_err(|e| failed(Stage::Finding, format!("{e:#}")))?
        .ok_or_else(|| Box::new(Outcome::Refused(Refusal::NoRelease { flavor: task.flavor, tier: task.tier })))?;
    let sha256 = release
        .asset
        .sha256()
        .ok_or_else(|| failed(Stage::Finding, format!("the release {} publishes no SHA-256 for {}", release.tag, release.asset.name)))?;
    fs::create_dir_all(private)
        .with_context(|| format!("make {}", private.display()))
        .map_err(|e| failed(Stage::Finding, format!("{e:#}")))?;
    let apk = private.join("manager.apk");
    tell(Stage::Downloading { done: 0, total: release.asset.size });
    let mut last = Instant::now() - PROGRESS_EVERY;
    let mut progress = |progress: Progress| {
        let (done, total) = match progress {
            Progress::Checking { done, total } | Progress::Downloading { done, total } => (done, total),
        };
        if done == total || last.elapsed() >= PROGRESS_EVERY {
            last = Instant::now();
            tell(Stage::Downloading { done, total });
        }
    };
    download::fetch(&agent, &release.asset.url, &apk, &sha256, &AtomicBool::new(false), &mut progress)
        .map_err(|e| failed(Stage::Downloading { done: 0, total: release.asset.size }, format!("{e:#}")))?;
    Ok(Got { apk, apk_sha256: Some(sha256), flavor: task.flavor, tag: release.tag })
}
