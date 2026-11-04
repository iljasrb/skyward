//! A device's own kernel, run off the page's thread: its newest release is
//! read, its complete boot image downloaded and checked, and the image written
//! to each boot partition. The release may take a moment over the network, and
//! the write is long enough that the page would stop moving under the finger.
//!
//! Nothing of the release is run as a script: its SHA-256 is checked and the
//! image is looked into before it is written. The thread says how far the run
//! has come as it goes, and how it ended.

use anyhow::{Context, Result};
use skyward_root::kernel::{self, Failure, Outcome, Plugin, Refusal, Stage};
use skyward_info::Battery;
use skyward_net::download::{self, Progress};
use skyward_net::http::{self, Servers};
use skyward_sys::worker;
use skyward_vab::Slot;
use std::fs::{self, File};
use std::io::copy;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// Where only root reads what the run keeps while it works.
const PRIVATE: &str = "/data/local/tmp/skyward-kernel";

/// The most often how far a download has come is told.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

/// What a run is asked to do besides the phone it is on.
pub struct Task {
    pub device: String,
    pub current: Slot,
    pub slots: Vec<Slot>,
    pub battery: Battery,
    pub plugin: Plugin,
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
        let worker = worker::Worker::start("kernel", "the kernel worker", move |tell| {
            let mut told = |stage: Stage| tell.send(Told::Stage(stage));
            let outcome = run(task, servers, &mut told);
            let _ = skyward_sys::kmsg(match &outcome {
                Outcome::Installed(_) => "kernel: the boot partitions are written",
                Outcome::Refused(_) => "kernel: nothing was written",
                Outcome::Failed(_) => "kernel: the run did not finish",
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

/// What a release check tells the page.
pub enum Checked {
    Done(Result<kernel::Details, String>),
}

/// Reads the newest release and its `INFO.md` off the page's thread, so the
/// kernel page can show the version before anything is written.
pub struct Checker {
    worker: worker::Worker<Checked>,
}

impl Checker {
    pub fn start(plugin: Plugin, servers: Servers) -> Result<Checker> {
        let worker = worker::Worker::start("kernel-release", "the kernel release checker", move |tell| {
            let checked = details(&plugin, servers);
            if let Err(e) = &checked {
                let _ = skyward_sys::kmsg(&format!("kernel release check failed: {e}"));
            }
            tell.send(Checked::Done(checked));
        })?;
        Ok(Checker { worker })
    }

    pub fn events(&self) -> Vec<Checked> {
        self.worker.events()
    }

    pub fn fd(&self) -> std::os::unix::io::RawFd {
        self.worker.fd()
    }
}

fn details(plugin: &Plugin, servers: Servers) -> Result<kernel::Details, String> {
    let agent = http::agent(servers);
    let sources = fetch_sources(&agent, &plugin.repo).map_err(|e| format!("{e:#}"))?;
    let tag = sources.tag().ok_or_else(|| "sources.env names no PROJECT_VERSION".to_string())?;
    let short = |revision: &str| revision.chars().take(12).collect::<String>();
    let kernel = match (&sources.line, &sources.kernel) {
        (Some(line), Some(revision)) => Some(format!("{line} @ {}", short(revision))),
        (_, Some(revision)) => Some(short(revision)),
        _ => None,
    };
    let manager = plugin.manager_name.clone().map(|name| match &sources.manager {
        Some(revision) => format!("{name} {}", short(revision)),
        None => name,
    });
    Ok(kernel::Details { tag, kernel, manager })
}

/// The kernel image and the release it came from.
struct Got {
    image: PathBuf,
    sha256: String,
    asset: String,
    tag: String,
    manager: Option<PathBuf>,
    manager_note: Option<String>,
}

fn run(task: Task, servers: Servers, tell: &mut dyn FnMut(Stage)) -> Outcome {
    let private = Path::new(PRIVATE).join(&task.stamp);
    let outcome = attempt(&task, servers, tell, &private);
    // The image is no longer needed once the run has ended; the copies and
    // the manifest are in the run's folder on the owner's storage.
    let _ = fs::remove_dir_all(&private);
    outcome
}

fn attempt(task: &Task, servers: Servers, tell: &mut dyn FnMut(Stage), private: &Path) -> Outcome {
    let got = match acquire(task, servers, tell, private) {
        Ok(got) => got,
        Err(outcome) => return outcome,
    };
    let options = kernel::Options {
        device: task.device.clone(),
        current: task.current,
        slots: task.slots.clone(),
        battery: task.battery.clone(),
        plugin: task.plugin.clone(),
        tag: got.tag,
        asset: got.asset,
        sha256: got.sha256,
        image: got.image,
        manager: got.manager,
        manager_note: got.manager_note,
        partition_dir: PathBuf::from("/dev/block/by-name"),
        storage: PathBuf::from(skyward_crypto::data::SDCARD),
        stamp: task.stamp.clone(),
    };
    kernel::install(&options, tell)
}

/// Take the newest release of the device's kernel, download its boot image
/// and check it against the sums file the release carries.
fn acquire(task: &Task, servers: Servers, tell: &mut dyn FnMut(Stage), private: &Path) -> Result<Got, Outcome> {
    tell(Stage::Finding);
    let failed = |stage: Stage, why: String| {
        Outcome::Failed(Failure {
            stage,
            slot: None,
            why,
            backups: Vec::new(),
            written: Vec::new(),
            run_dir: None,
        })
    };
    let agent = http::agent(servers);
    let sources = fetch_sources(&agent, &task.plugin.repo).map_err(|e| failed(Stage::Finding, format!("{e:#}")))?;
    let tag = sources.tag().ok_or(Outcome::Refused(Refusal::NoRelease))?;
    let image_name = sources
        .image(&task.plugin.asset)
        .ok_or(Outcome::Refused(Refusal::NoRelease))?;
    let image_url = sources
        .image_url(&task.plugin.repo, &task.plugin.asset)
        .ok_or(Outcome::Refused(Refusal::NoRelease))?;
    let sums_url = sources
        .sums_url(&task.plugin.repo, &task.plugin.asset)
        .ok_or(Outcome::Refused(Refusal::NoRelease))?;
    let mut response = agent
        .get(&sums_url)
        .header("User-Agent", "skyward-recovery")
        .call()
        .map_err(|e| failed(Stage::Finding, format!("ask {sums_url}: {e:#}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(failed(Stage::Finding, format!("{sums_url} answered {status}")));
    }
    let sums = response
        .body_mut()
        .read_to_string()
        .map_err(|e| failed(Stage::Finding, format!("read {sums_url}: {e:#}")))?;
    let sha256 = kernel::release::sums_sha256(&sums, &image_name)
        .ok_or_else(|| failed(Stage::Finding, format!("{sums_url} does not carry the SHA-256 of {image_name}")))?;
    fs::create_dir_all(private)
        .with_context(|| format!("make {}", private.display()))
        .map_err(|e| failed(Stage::Finding, format!("{e:#}")))?;
    let image = private.join("kernel.img");
    tell(Stage::Downloading { done: 0, total: 0 });
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
    download::fetch(&agent, &image_url, &image, &sha256, &AtomicBool::new(false), &mut progress)
        .map_err(|e| failed(Stage::Downloading { done: 0, total: 0 }, format!("{e:#}")))?;
    let (manager, manager_note) = if task.plugin.manager_name.is_some() {
        let urls = [sources.manager_url(), sources.manager_url_main()];
        let mut tried = Vec::new();
        let mut found = None;
        let mut note = None;
        for url in urls.into_iter().flatten() {
            if tried.contains(&url) {
                continue;
            }
            tried.push(url.clone());
            match fetch_manager(&url, &agent, private, tell) {
                Ok(path) => {
                    found = Some(path);
                    break;
                }
                Err(e) => note = Some(format!("{e:#}")),
            }
        }
        (found, note.or_else(|| Some(format!("{} pins no BakaSU manager", task.plugin.repo))))
    } else {
        (None, None)
    };
    Ok(Got { image, sha256, asset: image_name, tag, manager, manager_note })
}

fn fetch_sources(agent: &ureq::Agent, repo: &str) -> Result<kernel::release::Sources> {
    let url = format!("https://raw.githubusercontent.com/{repo}/main/sources.env");
    let mut response = agent.get(&url).header("User-Agent", "skyward-recovery").call().with_context(|| format!("ask {url}"))?;
    anyhow::ensure!(response.status().is_success(), "{url} answered {}", response.status());
    let text = response.body_mut().read_to_string().with_context(|| format!("read {url}"))?;
    Ok(kernel::release::sources(&text))
}

/// Download the zip of manager APKs the kernel's device names, and take its
/// arm64 APK. The zip comes from the latest CI run and is not hash-pinned;
/// the kernel image itself is.
fn fetch_manager(url: &str, agent: &ureq::Agent, private: &Path, tell: &mut dyn FnMut(Stage)) -> Result<PathBuf> {
    tell(Stage::DownloadingManager { done: 0, total: 0 });
    let mut response = agent.get(url).header("User-Agent", "skyward-recovery").call()?;
    let status = response.status();
    anyhow::ensure!(status.is_success(), "{url} answered {status}");
    let zip_path = private.join("manager.zip");
    let mut file = File::create(&zip_path).with_context(|| format!("make {}", zip_path.display()))?;
    copy(&mut response.body_mut().with_config().limit(u64::MAX).reader(), &mut file)
        .with_context(|| format!("download {url}"))?;
    file.sync_all().with_context(|| format!("flush {}", zip_path.display()))?;
    let size = fs::metadata(&zip_path)?.len();
    tell(Stage::DownloadingManager { done: size, total: size });

    let mut archive = zip::ZipArchive::new(File::open(&zip_path)?).with_context(|| format!("read {}", zip_path.display()))?;
    let names: Vec<String> = archive.file_names().map(str::to_string).collect();
    let index = names
        .iter()
        .position(|name| name.ends_with(".apk") && name.contains("arm64-v8a") && !name.contains("debug"))
        .or_else(|| names.iter().position(|name| name.ends_with(".apk") && name.contains("universal")))
        .or_else(|| names.iter().position(|name| name.ends_with(".apk")))
        .with_context(|| format!("{url} carries no APK"))?;
    let apk = private.join("manager.apk");
    let mut entry = archive.by_index(index)?;
    let mut out = File::create(&apk).with_context(|| format!("make {}", apk.display()))?;
    copy(&mut entry, &mut out).with_context(|| format!("take {} out of {}", names[index], zip_path.display()))?;
    out.sync_all().with_context(|| format!("flush {}", apk.display()))?;
    let mut apk_zip = zip::ZipArchive::new(File::open(&apk)?).with_context(|| format!("read {}", apk.display()))?;
    anyhow::ensure!(apk_zip.by_name("AndroidManifest.xml").is_ok(), "{} is not an APK", names[index]);
    Ok(apk)
}
