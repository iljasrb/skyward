//! The recovery's own updates: asking the fork's releases on GitHub for a
//! newer image of this phone, downloading it, and writing it over the
//! recovery partitions -- each on a thread of its own that tells the screen
//! how far it has come.

use crate::recovery;
use anyhow::{Context, Result};
use skyward_net::download::{self, Damaged, Failure, NoRoom, Progress, Refused};
use skyward_net::http::{self, Servers};
use skyward_net::reach::{self, Reach};
use skyward_net::release::{self, Release, Unanswered};
use skyward_net::rom;
use skyward_sys::worker;
use skyward_vab::Slot;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The most often how far a download has come is told.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

/// How long the speed shown is taken over.
const RATE_OVER: Duration = Duration::from_secs(3);

/// How many tries may fail before the download gives up, and how long it
/// waits between them.
const TRIES: u32 = 5;
const BETWEEN: Duration = Duration::from_secs(3);

pub enum Task {
    /// Ask `repo` for its newest release that carries `asset`, and whether it
    /// is newer than the build running, `build`. Where that fails, `probe`,
    /// the page that tells whether the network joined reaches the internet,
    /// says whether the network is why.
    Check { repo: String, asset: String, build: String, probe: String },
    /// Download `release`'s image to `path`, checked against the SHA-256 the
    /// release publishes; `sha256` is that, where it is already known.
    Download { release: Release, sha256: Option<String>, path: PathBuf },
    /// Write the image at `path` over the recovery partitions: the one the
    /// phone runs `active` from, and the other when it holds this recovery.
    Install { path: PathBuf, active: Slot },
    /// Ask the phone's ROM for its newest system, and say whether it is newer
    /// than the installed one. `flavor` is the prefix the installed system's
    /// `ro.build.flavor` must have, where the update is one ROM's alone.
    RomCheck { feed: String, installed: String, flavor: Option<String>, probe: String },
    /// Download the ROM's package to `path`, checked against the SHA-256 the
    /// feed publishes.
    RomDownload { release: rom::Release, path: PathBuf },
}

pub enum Event {
    Checked(Result<Option<Release>, Unanswered>),
    /// How far the download has come, and how fast it goes, in bytes a
    /// second.
    Progress(Progress, u64),
    Ended(End),
    /// How many bytes of how many were written and read back.
    Writing(u64, u64),
    /// The slots written, or why they could not be.
    Installed(Result<Vec<Slot>, String>),
    /// What the ROM's feed says of its newest system: none while the system
    /// installed is as new.
    RomChecked(Result<rom::Answer, Unanswered>),
    RomEnded(End),
}

pub enum End {
    Downloaded,
    Stopped,
    Failed(Failure),
}

pub struct Worker {
    pub worker: worker::Worker<Event>,
    stop: Arc<AtomicBool>,
    /// The connection in use, which stopping cuts at once.
    agent: Arc<Mutex<Option<ureq::Agent>>>,
}

impl Worker {
    pub fn start(task: Task, servers: Servers) -> Result<Worker> {
        let stop = Arc::new(AtomicBool::new(false));
        let agent: Arc<Mutex<Option<ureq::Agent>>> = Arc::new(Mutex::new(None));
        let (stopping, using) = (stop.clone(), agent.clone());
        let worker = worker::Worker::start("updates", "the update worker", move |tell| {
            let say = |event: Event| tell.send(event);
            match task {
                Task::Check { repo, asset, build, probe } => {
                    let a = http::agent(servers.clone());
                    *using.lock().expect("the agent is not held by a thread that panicked") = Some(a.clone());
                    let answer = release::newest(&a, &repo, &asset)
                        .map_err(|e| unanswered(&e, &repo, &probe, &servers))
                        .and_then(|found| match found {
                            // A release nothing can be checked against is
                            // no release to offer.
                            Some(r) if r.is_newer_than(&build) && (r.image.digest.is_some() || r.sums.is_some()) => Ok(Some(r)),
                            Some(r) if r.is_newer_than(&build) => Err(Unanswered::Failed(format!(
                                "the release {} of {repo} publishes no SHA-256 to check its image against",
                                r.tag
                            ))),
                            _ => Ok(None),
                        });
                    match &answer {
                        Ok(Some(r)) => {
                            let _ = skyward_sys::kmsg(&format!("a newer recovery is published: {} ({})", r.tag, r.published));
                        }
                        Ok(None) => {
                            let _ = skyward_sys::kmsg(&format!("no recovery is published after this build, {build}"));
                        }
                        Err(why) => {
                            let _ = skyward_sys::kmsg(&format!("the releases of {repo} were not read: {why:?}"));
                        }
                    }
                    say(Event::Checked(answer));
                }
                Task::Download { release, sha256, path } => {
                    let mut remote = Remote::of_release(&release);
                    if sha256.is_some() {
                        remote.sha256 = sha256;
                    }
                    let end = fetch(&servers, remote, &path, &stopping, &using, &say);
                    say(Event::Ended(end));
                }
                Task::Install { path, active } => {
                    let mut told = Instant::now() - PROGRESS_EVERY;
                    let written = recovery::slots(active).and_then(|slots| {
                        recovery::install(&path, &slots, &mut |done, total| {
                            if done == total || told.elapsed() >= PROGRESS_EVERY * 5 {
                                told = Instant::now();
                                say(Event::Writing(done, total));
                            }
                        })
                        .map(|()| slots)
                    });
                    let _ = skyward_sys::kmsg(&format!("writing {}: {written:?}", path.display()));
                    say(Event::Installed(written.map_err(|e| format!("{e:#}"))));
                }
                Task::RomCheck { feed, installed, flavor, probe } => {
                    let on_this_rom = rom::is_this_rom(
                        skyward_info::current_prop("ro.build.flavor").as_deref(),
                        flavor.as_deref(),
                    );
                    let answer = if on_this_rom {
                        let a = http::agent(servers.clone());
                        *using.lock().expect("the agent is not held by a thread that panicked") = Some(a.clone());
                        rom::newest(&a, &feed)
                            .map_err(|e| unanswered(&e, &feed, &probe, &servers))
                            .map(|found| match found.is_newer_than(&installed) {
                                true => rom::Answer::Newer(found),
                                false => rom::Answer::Current,
                            })
                    } else {
                        Ok(rom::Answer::Other)
                    };
                    match &answer {
                        Ok(rom::Answer::Newer(found)) => {
                            let _ = skyward_sys::kmsg(&format!("a newer system is published: {}", found.build));
                        }
                        Ok(rom::Answer::Current) => {
                            let _ = skyward_sys::kmsg(&format!("no system is published after {installed}"));
                        }
                        Ok(rom::Answer::Other) => {
                            let _ = skyward_sys::kmsg("the installed system is not the ROM the feed belongs to");
                        }
                        Err(why) => {
                            let _ = skyward_sys::kmsg(&format!("the system's feed {feed} was not read: {why:?}"));
                        }
                    }
                    say(Event::RomChecked(answer));
                }
                Task::RomDownload { release, path } => {
                    let end = fetch(&servers, Remote::of_rom(&release), &path, &stopping, &using, &say);
                    say(Event::RomEnded(end));
                }
            }
        })?;
        Ok(Worker { worker, stop, agent })
    }

    /// Stop at once, cutting the connection in use.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(a) = self.agent.lock().expect("the agent is not held by a thread that panicked").take() {
            a.shutdown();
        }
    }
}

/// Why `repo` gave no answer: that the network joined wants a login first, or
/// reaches nothing, where `probe` shows it, rather than how asking failed
/// through such a network.
fn unanswered(e: &anyhow::Error, repo: &str, probe: &str, servers: &Servers) -> Unanswered {
    let dns = servers.lock().expect("the DNS servers are not held by a thread that panicked").clone();
    let reach = reach::probe(probe, &dns);
    let _ = skyward_sys::kmsg(&format!("asking {repo}: {e:#}; the network joined {reach}"));
    match reach {
        Reach::Online => Unanswered::Failed(format!("{e:#}")),
        Reach::Login(_) => Unanswered::Login,
        Reach::Offline(why) => Unanswered::Offline(why),
    }
}

/// A package to download, with where its SHA-256 is if it is not already
/// known: a recovery's GitHub release, or the ROM's system update.
struct Remote {
    name: String,
    url: String,
    size: u64,
    /// The SHA-256 already known, from the release's digest or the feed.
    sha256: Option<String>,
    /// Where the file of sums that names the package is, where the digest is
    /// not known.
    sums: Option<String>,
}

impl Remote {
    fn of_release(release: &Release) -> Remote {
        Remote {
            name: release.image.name.clone(),
            url: release.image.url.clone(),
            size: release.image.size,
            sha256: release.sha256(None),
            sums: release.sums.as_ref().map(|sums| sums.url.clone()),
        }
    }

    fn of_rom(release: &rom::Release) -> Remote {
        Remote {
            name: release.image.name.clone(),
            url: release.image.url.clone(),
            size: release.image.size,
            sha256: release.image.digest.clone(),
            // The OTA feed names only its package's MD5; the mirror keeps the
            // SHA-256 sums beside the package.
            sums: match release.image.digest {
                Some(_) => None,
                None => release.image.url.rsplit_once('/').map(|(dir, _)| format!("{dir}/SHA256SUMS")),
            },
        }
    }
}

/// Read the SHA-256 a package is to be checked against: the one already
/// known, or else the line for it in the file of sums.
fn published_sha(agent: &ureq::Agent, remote: &Remote) -> Result<String> {
    if let Some(known) = &remote.sha256 {
        return Ok(known.clone());
    }
    let url = remote.sums.as_ref().context("the update publishes no SHA-256 for its package")?;
    let mut response = agent.get(url).call().with_context(|| format!("ask {url}"))?;
    let status = response.status();
    anyhow::ensure!(status.is_success(), "{url} answered {status}");
    let body = response.body_mut().read_to_string().with_context(|| format!("read {url}"))?;
    release::sha256_in(&body, &remote.name).with_context(|| format!("{url} says nothing of {}", remote.name))
}

/// Download the package, trying again while the connection fails and
/// something more comes each time.
fn fetch(
    servers: &Servers,
    remote: Remote,
    path: &std::path::Path,
    stop: &AtomicBool,
    using: &Mutex<Option<ureq::Agent>>,
    tell: &dyn Fn(Event),
) -> End {
    let mut failed = 0;
    let mut samples: Vec<(Instant, u64)> = Vec::new();
    let mut told = Instant::now() - PROGRESS_EVERY;
    loop {
        if stop.load(Ordering::Relaxed) {
            return End::Stopped;
        }
        let a = http::agent(servers.clone());
        *using.lock().expect("the agent is not held by a thread that panicked") = Some(a.clone());
        let sha256 = match published_sha(&a, &remote) {
            Ok(sha) => sha,
            Err(e) => {
                let _ = skyward_sys::kmsg(&format!("reading the sums of {}: {e:#}", remote.name));
                failed += 1;
                if failed >= TRIES {
                    return End::Failed(Failure::Other(format!("{e:#}")));
                }
                if !pause(stop) {
                    return End::Stopped;
                }
                continue;
            }
        };
        let mut progress = |p: Progress| {
            if let Progress::Downloading { done, .. } = p {
                let now = Instant::now();
                samples.push((now, done));
                samples.retain(|(at, _)| now.duration_since(*at) <= RATE_OVER);
            }
            let rate = match (samples.first(), samples.last()) {
                (Some((t0, d0)), Some((t1, d1))) if t1 > t0 => ((d1 - d0) as f64 / t1.duration_since(*t0).as_secs_f64()) as u64,
                _ => 0,
            };
            if told.elapsed() >= PROGRESS_EVERY {
                told = Instant::now();
                tell(Event::Progress(p, rate));
            }
        };
        let result = download::fetch(&a, &remote.url, path, &sha256, stop, &mut progress);
        // How far it came shows in the end, whatever stopped it.
        let done = samples.last().map_or(0, |&(_, done)| done);
        tell(Event::Progress(Progress::Downloading { done, total: remote.size }, 0));
        let e = match result {
            Ok(()) => return End::Downloaded,
            Err(e) => e,
        };
        if e.is::<skyward_net::Stopped>() || stop.load(Ordering::Relaxed) {
            return End::Stopped;
        }
        if let Some(room) = e.downcast_ref::<NoRoom>() {
            return End::Failed(Failure::NoRoom { needed: room.needed, free: room.free });
        }
        if e.is::<Damaged>() {
            return End::Failed(Failure::Damaged);
        }
        if let Some(refused) = e.downcast_ref::<Refused>() {
            // A server that is busy, or asks to try later, is tried again; one
            // that will not serve the file at all is not.
            let status = refused.0;
            if !(status == 408 || status == 416 || status == 429 || (500..=599).contains(&status)) {
                return End::Failed(Failure::Other(format!("{e:#}")));
            }
        }
        let _ = skyward_sys::kmsg(&format!("downloading {} from {}: {e:#}", remote.name, remote.url));
        failed += 1;
        if failed >= TRIES {
            return End::Failed(Failure::Other(format!("{e:#}")));
        }
        if !pause(stop) {
            return End::Stopped;
        }
    }
}

/// Wait between two tries, unless the user stopped the download.
fn pause(stop: &AtomicBool) -> bool {
    let until = Instant::now() + BETWEEN;
    while Instant::now() < until {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
    true
}
