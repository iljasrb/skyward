//! The phone's WLAN inside the recovery, on a thread of its own: the driver
//! brought up, the phone's own wpa_supplicant joining networks, and an
//! address asked for over DHCP and kept. The screen is told how things
//! stand each time that changes.

use anyhow::{bail, ensure, Context, Result};
use skyward_config::{Firmware, Wlan as Config};
use skyward_net::dhcp::{self, Kept, Lease};
use skyward_net::http::Servers;
use skyward_net::iface;
use skyward_net::reach::{self, Reach};
use skyward_net::supplicant::{self, Bss, Control, Event as Told, Security};
use skyward_net::wlan::{self, Joined, Link, Next, Power, Stage, Traffic, Wlan};
use std::fs;
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The first folder the firmware loader looks in that a recovery can
/// write.
const FIRMWARE: &str = "/etc/firmware";

/// Where a partition a firmware file is read from is mounted meanwhile.
const PARTITIONS: &str = "/skyward/wlan";

/// The supplicant's settings, and the folder of its control socket.
const HOME: &str = "/tmp/skyward-wlan";

/// The user the supplicant takes on as it starts, who has to be able to
/// make its socket.
const WIFI: u32 = 1010;

/// How long the driver may take to make its interface, the chip's firmware
/// loading meanwhile.
const INTERFACE_WAIT: Duration = Duration::from_secs(20);

/// How long the supplicant may take to open its control socket.
const SOCKET_WAIT: Duration = Duration::from_secs(10);

/// How long the supplicant gets to end once asked to.
const END_WAIT: Duration = Duration::from_secs(3);

/// How often a network joined is read for its signal.
const POLL: Duration = Duration::from_secs(5);

/// How often what went over the interface is counted, for how fast it
/// goes.
const TRAFFIC: Duration = Duration::from_secs(1);

/// How long after the network gave no address it is asked again.
const RETRY: Duration = Duration::from_secs(10);

/// What the screen asks of the WLAN.
pub enum Command {
    On,
    Off,
    /// Read the networks the boots before saved, once the partition they are
    /// kept on is open.
    Adopt,
    Scan,
    Join { ssid: Vec<u8>, name: String, security: Security, password: String },
    Leave,
}

/// What comes to the thread: a command, how asking for an address went,
/// how far the network joined reaches, or that the screen has let the
/// thread go.
enum Inbox {
    Command(Command),
    Address(u64, Result<Asked>),
    Reach(u64, Reach),
    End,
}

enum Asked {
    Got(Lease),
    Kept(Kept),
}

/// What the thread tells the screen: how the WLAN stands, or how fast data
/// went over it in the last second.
enum Report {
    Wlan(Wlan),
    Traffic(Traffic),
}

pub struct Worker {
    inbox: Sender<Inbox>,
    /// Written to wake the thread when something is in its inbox.
    poke: Arc<OwnedFd>,
    told: Receiver<Report>,
    /// Readable while the screen has something to be told.
    wake: OwnedFd,
    thread: Option<JoinHandle<()>>,
}

fn pipe() -> Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(std::io::Error::last_os_error()).context("make a pipe for the WLAN thread");
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn signal(fd: &OwnedFd) {
    unsafe { libc::write(fd.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
}

fn drain(fd: &OwnedFd) {
    let mut buf = [0u8; 64];
    while unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
}

impl Worker {
    /// Start the thread, with the WLAN off. `modules` are the folders the
    /// driver's modules are found in, `libraries` the bundle of the vendor
    /// interface's libraries, and `servers` where the DNS servers the
    /// network gives are kept for whoever looks names up.
    pub fn start(cfg: Config, modules: Vec<PathBuf>, libraries: Option<PathBuf>, servers: Servers) -> Result<Worker> {
        let (inbox_tx, inbox) = mpsc::channel();
        let (poked, poke) = pipe()?;
        let (told_tx, told) = mpsc::channel();
        let (wake, rung) = pipe()?;
        let poke = Arc::new(poke);
        let back = Back { inbox: inbox_tx.clone(), poke: poke.clone() };
        let thread = thread::Builder::new()
            .name("wlan".into())
            .spawn(move || {
                let tell = move |r: Report| {
                    // The screen only stops listening once it has let the
                    // worker go.
                    let _ = told_tx.send(r);
                    signal(&rung);
                };
                let mut s = Station { cfg, modules, libraries, servers, state: Wlan::default(), running: None, back, tell: Box::new(tell) };
                s.run(&inbox, &poked);
            })
            .context("start the WLAN thread")?;
        Ok(Worker { inbox: inbox_tx, poke, told, wake, thread: Some(thread) })
    }

    pub fn send(&self, c: Command) {
        let _ = self.inbox.send(Inbox::Command(c));
        signal(&self.poke);
    }

    pub fn fd(&self) -> RawFd {
        self.wake.as_raw_fd()
    }

    /// How the WLAN stands now, and how fast data went over it in the last
    /// second, each if told since the last call.
    pub fn told(&self) -> (Option<Wlan>, Option<Traffic>) {
        drain(&self.wake);
        let (mut wlan, mut traffic) = (None, None);
        for report in self.told.try_iter() {
            match report {
                Report::Wlan(w) => wlan = Some(w),
                Report::Traffic(t) => traffic = Some(t),
            }
        }
        (wlan, traffic)
    }
}

impl Drop for Worker {
    /// The WLAN goes off with the thread.
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.inbox.send(Inbox::End);
            signal(&self.poke);
            thread.join().expect("the WLAN thread does not panic");
        }
    }
}

/// What a thread asking for an address answers to.
#[derive(Clone)]
struct Back {
    inbox: Sender<Inbox>,
    poke: Arc<OwnedFd>,
}

/// The supplicant running, and the address the network gave.
struct Running {
    child: Child,
    control: Control,
    monitor: Control,
    mac: [u8; 6],
    found: Vec<Bss>,
    lease: Option<Held>,
    /// The thread asking for an address or to keep one, and its number;
    /// what an earlier one answers is let go.
    asking: Option<(u64, Arc<AtomicBool>)>,
    asked: u64,
    /// The number of the last look at how far the network reaches; what an
    /// earlier one finds is let go.
    probed: u64,
    /// When to ask for an address again after the network gave none.
    retry_at: Option<Instant>,
    polled_at: Instant,
    /// What had gone over the interface when last counted, while it has an
    /// address.
    counted: Option<Counted>,
}

/// The interface's totals in and out, in bytes, and when they were read.
#[derive(Clone, Copy)]
struct Counted {
    at: Instant,
    rx: u64,
    tx: u64,
}

/// A lease, when it was given, and how far keeping it has come: 0 asks its
/// server at the renewal time, 1 any server at the rebinding time, 2 lets
/// it go when it runs out.
struct Held {
    lease: Lease,
    at: Instant,
    phase: u8,
}

impl Held {
    fn due(&self) -> Instant {
        let after = match self.phase {
            0 => self.lease.renew,
            1 => self.lease.rebind,
            _ => self.lease.seconds,
        };
        self.at + Duration::from_secs(after.into())
    }
}

struct Station {
    cfg: Config,
    modules: Vec<PathBuf>,
    libraries: Option<PathBuf>,
    servers: Servers,
    state: Wlan,
    running: Option<Running>,
    back: Back,
    tell: Box<dyn Fn(Report) + Send>,
}

impl Station {
    fn run(&mut self, inbox: &Receiver<Inbox>, poked: &OwnedFd) {
        let mut told = self.state.clone();
        loop {
            let mut fds = vec![libc::pollfd { fd: poked.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
            if let Some(r) = &self.running {
                fds.push(libc::pollfd { fd: r.monitor.fd(), events: libc::POLLIN, revents: 0 });
            }
            let timeout = self.next().map_or(-1, |at| at.saturating_duration_since(Instant::now()).as_millis().min(60_000) as i32);
            if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) } < 0 {
                let e = std::io::Error::last_os_error();
                assert_eq!(e.kind(), std::io::ErrorKind::Interrupted, "poll the WLAN thread's inputs: {e}");
            }
            drain(poked);
            for message in inbox.try_iter() {
                match message {
                    Inbox::Command(c) => self.command(c),
                    Inbox::Address(n, answer) => self.addressed(n, answer),
                    Inbox::Reach(n, reach) => self.reached(n, reach),
                    Inbox::End => return self.stop(),
                }
            }
            self.listen();
            self.timers();
            if self.state != told {
                told = self.state.clone();
                (self.tell)(Report::Wlan(told.clone()));
            }
        }
    }

    /// The soonest a timer is due.
    fn next(&self) -> Option<Instant> {
        let r = self.running.as_ref()?;
        let poll = matches!(self.state.joined, Some(Joined { stage: Stage::Connected(_), .. })).then(|| r.polled_at + POLL);
        let count = r.lease.as_ref().map(|_| r.counted.map_or_else(Instant::now, |c| c.at + TRAFFIC));
        [r.lease.as_ref().filter(|_| r.asking.is_none()).map(Held::due), r.retry_at, poll, count].into_iter().flatten().min()
    }

    fn command(&mut self, c: Command) {
        match c {
            Command::On => self.on(),
            Command::Off => self.off(),
            Command::Adopt => self.adopt(),
            Command::Scan => {
                if let Some(r) = &self.running {
                    // A scan under way answers FAIL-BUSY, and its results
                    // come all the same.
                    if let Err(e) = r.control.ask("SCAN") {
                        self.lost(e);
                    }
                }
            }
            Command::Join { ssid, name, security, password } => {
                self.unaddress();
                let mut joined = Joined { ssid, name, security, stage: Stage::Joining };
                if let Err(e) = self.join(&joined, &password) {
                    let _ = skyward_sys::kmsg(&format!("joining {} failed: {e:#}", joined.name));
                    joined.stage = Stage::Failed;
                }
                self.state.joined = Some(joined);
            }
            Command::Leave => {
                self.unaddress();
                if let Some(r) = &self.running {
                    if let Err(e) = r.control.order("REMOVE_NETWORK all") {
                        self.lost(e);
                    }
                }
                self.state.joined = None;
            }
        }
    }

    fn on(&mut self) {
        // One already up is joined again rather than started again: started
        // against the driver a second time, this phone's supplicant ends with
        // SIGSEGV.
        if self.running.is_none() {
            self.state.power = Power::Starting;
            (self.tell)(Report::Wlan(self.state.clone()));
            match self.start() {
                Ok(r) => self.running = Some(r),
                Err(e) => {
                    let _ = skyward_sys::kmsg(&format!("the WLAN did not come up: {e:#}"));
                    self.state.power = Power::Failed(format!("{e:#}"));
                    return;
                }
            }
            // A boot where the data partition is open already holds the
            // networks saved on it; none of them was in memory to start from.
            self.adopt();
        }
        self.state.power = Power::On;
        self.resume();
        self.command(Command::Scan);
    }

    /// The network the session before joined and the supplicant kept: it is
    /// chosen again, so a recovery that comes back up is on the network it
    /// left without the user typing the password again. Choosing it makes the
    /// supplicant report the connection the way it reports one asked for
    /// here, which is what asks for an address and shows how it goes.
    fn resume(&mut self) {
        let mut joined = None;
        let mut lost = None;
        if let Some(r) = &self.running {
            match r.control.ask("LIST_NETWORKS") {
                Ok(list) => {
                    if let Some(kept) = supplicant::remembered(&list) {
                        let security = r
                            .control
                            .ask(&format!("GET_NETWORK {} key_mgmt", kept.id))
                            .map(|key| supplicant::security(&key))
                            .unwrap_or(supplicant::Security::Psk);
                        match r.control.order(&format!("SELECT_NETWORK {}", kept.id)) {
                            Ok(()) => {
                                let _ = skyward_sys::kmsg(&format!("joining {} again, as it was joined last", String::from_utf8_lossy(&kept.ssid)));
                                joined = Some(Joined {
                                    name: String::from_utf8_lossy(&kept.ssid).into_owned(),
                                    ssid: kept.ssid,
                                    security,
                                    stage: Stage::Joining,
                                });
                            }
                            Err(e) => lost = Some(e),
                        }
                    }
                }
                Err(e) => lost = Some(e),
            }
        }
        match lost {
            Some(e) => self.lost(e),
            None => self.state.joined = joined,
        }
    }

    fn start(&self) -> Result<Running> {
        bring_up(&self.cfg, &self.modules)?;
        let mut child = start_supplicant(&self.cfg, self.libraries.as_deref())?;
        let socket = Path::new(HOME).join("sockets").join(&self.cfg.interface);
        let connected = (|| -> Result<(Control, Control)> {
            let control = Control::open(&socket)?;
            ensure!(control.ask("PING")?.trim_end() == "PONG", "wpa_supplicant does not answer PING");
            let monitor = Control::open(&socket)?;
            monitor.attach()?;
            Ok((control, monitor))
        })();
        let (control, monitor) = match connected {
            Ok(both) => both,
            Err(e) => {
                end(&mut child, None);
                return Err(e);
            }
        };
        let mac = iface::hardware_address(&self.cfg.interface)?;
        Ok(Running {
            child,
            control,
            monitor,
            mac,
            found: Vec::new(),
            lease: None,
            asking: None,
            asked: 0,
            probed: 0,
            retry_at: None,
            polled_at: Instant::now(),
            counted: None,
        })
    }

    /// Leave the network, and keep the supplicant for the next time the WLAN
    /// is asked for: started against the driver a second time, it ends with
    /// SIGSEGV. One that does not answer is noticed by the events read next.
    fn off(&mut self) {
        self.unaddress();
        if let Some(r) = &self.running {
            let _ = r.control.order("DISCONNECT");
        }
        self.state = Wlan::default();
    }

    /// End the supplicant, and the interface with it.
    fn stop(&mut self) {
        self.unaddress();
        if let Some(mut r) = self.running.take() {
            end(&mut r.child, Some(&r.control));
        }
        if let Err(e) = iface::set_up(&self.cfg.interface, false) {
            let _ = skyward_sys::kmsg(&format!("{e:#}"));
        }
        self.state = Wlan::default();
    }

    /// The supplicant no longer answers: the WLAN is off, and why is said.
    fn lost(&mut self, e: anyhow::Error) {
        let _ = skyward_sys::kmsg(&format!("the WLAN went away: {e:#}"));
        self.stop();
        self.state.power = Power::Failed(format!("{e:#}"));
    }

    fn join(&self, joined: &Joined, password: &str) -> Result<()> {
        let r = self.running.as_ref().context("the WLAN is off")?;
        r.control.order("REMOVE_NETWORK all")?;
        let id: u32 = r.control.ask("ADD_NETWORK")?.trim().parse().context("wpa_supplicant numbered the network with no number")?;
        // One the last scan did not see may hide its name, as one added by
        // hand may.
        let hidden = !r.found.iter().any(|b| b.ssid == joined.ssid);
        for c in supplicant::network_commands(id, &joined.ssid, joined.security, password, hidden)? {
            r.control.order(&c)?;
        }
        r.control.order(&format!("SELECT_NETWORK {id}"))?;
        // What was joined is kept for the boots to come, where the
        // configuration can be written back.
        r.control.order("SAVE_CONFIG")?;
        self.keep_saved();
        Ok(())
    }

    /// Keep what the supplicant wrote where the boots to come read it, once
    /// that partition is open: its own copy lives in memory until then.
    fn keep_saved(&self) {
        let Some(dir) = kept_dir() else { return };
        let saved = dir.join(KEPT.1);
        let keep = fs::read(Path::new(HOME).join(KEPT.1))
            .and_then(|text| fs::write(&saved, text))
            .and_then(|()| std::os::unix::fs::chown(&saved, Some(WIFI), Some(WIFI)))
            .and_then(|()| fs::set_permissions(&saved, fs::Permissions::from_mode(0o600)));
        match keep {
            Ok(()) => {
                let _ = skyward_sys::kmsg(&format!("the WLAN's networks are kept in {}", saved.display()));
            }
            Err(e) => {
                let _ = skyward_sys::kmsg(&format!("the WLAN's networks are not kept: {e}"));
            }
        }
    }

    /// Take in the networks the boots before saved, which a supplicant
    /// started before that partition was open has not read: its own copy
    /// gains them, and it reads that copy again. Started again it ends with
    /// SIGSEGV, so reading again is what is asked of it instead.
    fn adopt(&mut self) {
        let Some(dir) = kept_dir() else { return };
        let Ok(theirs) = fs::read_to_string(dir.join(KEPT.1)) else { return };
        let Some(at) = theirs.find("network={") else {
            // Nothing saved on the partition yet: what was joined while it
            // was closed goes there now.
            self.keep_saved();
            return;
        };
        let ours = Path::new(HOME).join(KEPT.1);
        let Ok(mut text) = fs::read_to_string(&ours) else { return };
        if !text.contains(&theirs[at..]) {
            text.push_str(&theirs[at..]);
            if let Err(e) = fs::write(&ours, &text) {
                let _ = skyward_sys::kmsg(&format!("{} cannot hold the WLAN's saved networks: {e}", ours.display()));
                return;
            }
        }
        self.keep_saved();
        let reread = match &self.running {
            Some(r) => r.control.order("RECONFIGURE"),
            None => return,
        };
        match reread {
            Ok(()) => {
                let _ = skyward_sys::kmsg("the WLAN read the networks saved on the data partition");
                self.resume();
            }
            Err(e) => self.lost(e),
        }
    }

    /// Read what the supplicant has told.
    fn listen(&mut self) {
        let Some(r) = &self.running else { return };
        let messages = match r.monitor.messages() {
            Ok(m) => m,
            Err(e) => return self.lost(e),
        };
        for m in messages {
            let Some(told) = supplicant::event(&m) else { continue };
            match told {
                Told::ScanResults => {
                    if let Err(e) = self.read_scan() {
                        return self.lost(e);
                    }
                }
                Told::ScanFailed => {}
                Told::Terminating => return self.lost(anyhow::anyhow!("wpa_supplicant ended")),
                other => self.joining(&other),
            }
        }
    }

    fn read_scan(&mut self) -> Result<()> {
        let r = self.running.as_mut().expect("scan results come from a supplicant running");
        let mut found = Vec::new();
        let mut first = 0;
        loop {
            let page = supplicant::bss_list(&r.control.ask(&supplicant::bss_command(first))?)?;
            let Some(last) = page.last() else { break };
            first = last.id + 1;
            found.extend(page);
        }
        self.state.networks = wlan::networks(&found);
        r.found = found;
        Ok(())
    }

    fn joining(&mut self, told: &Told) {
        let Some(j) = &mut self.state.joined else { return };
        match j.on(told) {
            Next::Show => {}
            Next::Address => self.ask_address(),
            Next::Unaddress => self.unaddress(),
            Next::Forget => {
                if let Some(r) = &self.running {
                    if let Err(e) = r.control.order("REMOVE_NETWORK all") {
                        self.lost(e);
                    }
                }
            }
        }
    }

    /// Ask the network for an address, on a thread of its own.
    fn ask_address(&mut self) {
        let interface = self.cfg.interface.clone();
        let back = self.back.clone();
        let Some(r) = &mut self.running else { return };
        r.retry_at = None;
        if let Some((_, stop)) = r.asking.take() {
            stop.store(true, Ordering::Relaxed);
        }
        r.asked += 1;
        let n = r.asked;
        let stop = Arc::new(AtomicBool::new(false));
        r.asking = Some((n, stop.clone()));
        let mac = r.mac;
        let keeping = r.lease.as_ref().map(|h| (h.lease.clone(), h.phase));
        thread::spawn(move || {
            let answer = match keeping {
                None => dhcp::obtain(&interface, mac, &stop).map(Asked::Got),
                Some((lease, phase)) => dhcp::keep(&interface, mac, &lease, phase > 0, &stop).map(Asked::Kept),
            };
            let _ = back.inbox.send(Inbox::Address(n, answer));
            signal(&back.poke);
        });
    }

    fn addressed(&mut self, n: u64, answer: Result<Asked>) {
        let Some(r) = &mut self.running else { return };
        if r.asking.as_ref().map(|(m, _)| *m) != Some(n) {
            return;
        }
        r.asking = None;
        let lease = match answer {
            Ok(Asked::Got(lease)) | Ok(Asked::Kept(Kept::Kept(lease))) => lease,
            Ok(Asked::Kept(Kept::Unanswered)) => {
                if let Some(h) = &mut r.lease {
                    h.phase += 1;
                }
                return;
            }
            Ok(Asked::Kept(Kept::Refused)) => {
                self.unaddress();
                return self.ask_address();
            }
            Err(e) if e.is::<skyward_net::Stopped>() => return,
            Err(e) => {
                let _ = skyward_sys::kmsg(&format!("no address: {e:#}"));
                r.retry_at = Some(Instant::now() + RETRY);
                if let Some(j) = &mut self.state.joined {
                    j.stage = Stage::NoAddress(format!("{e:#}"));
                }
                return;
            }
        };
        let same = r.lease.as_ref().is_some_and(|h| (h.lease.address, h.lease.mask, h.lease.router) == (lease.address, lease.mask, lease.router));
        if !same {
            if let Err(e) = iface::configure(&self.cfg.interface, lease.address, lease.mask, lease.router) {
                let _ = skyward_sys::kmsg(&format!("{e:#}"));
                r.retry_at = Some(Instant::now() + RETRY);
                if let Some(j) = &mut self.state.joined {
                    j.stage = Stage::NoAddress(format!("{e:#}"));
                }
                return;
            }
        }
        let _ = skyward_sys::kmsg(&format!(
            "{} has {} ({}) through {:?} for {} s, kept again after {} s",
            self.cfg.interface, lease.address, lease.mask, lease.router, lease.seconds, lease.renew
        ));
        *self.servers.lock().expect("the DNS servers are not held by a thread that panicked") = lease.dns.clone();
        let mut link = Link::new(&lease, r.mac);
        // An address kept keeps how far its network was found to reach; a
        // new one has it looked at.
        link.reach = match &self.state.joined {
            Some(Joined { stage: Stage::Connected(had), .. }) if same => had.reach.clone(),
            _ => None,
        };
        let look = link.reach.is_none().then(|| lease.dns.clone());
        r.lease = Some(Held { lease, at: Instant::now(), phase: 0 });
        if let Some(j) = &mut self.state.joined {
            j.stage = Stage::Connected(link);
        }
        if let Some(dns) = look {
            self.probe(dns);
        }
        self.poll_link();
    }

    /// Look how far the network joined reaches, on a thread of its own, the
    /// page's name looked up in the network's own DNS servers, `dns`.
    fn probe(&mut self, dns: Vec<Ipv4Addr>) {
        let (page, back) = (self.cfg.probe.clone(), self.back.clone());
        let Some(r) = &mut self.running else { return };
        r.probed += 1;
        let n = r.probed;
        thread::spawn(move || {
            let reach = reach::probe(&page, &dns);
            let _ = back.inbox.send(Inbox::Reach(n, reach));
            signal(&back.poke);
        });
    }

    fn reached(&mut self, n: u64, reach: Reach) {
        if self.running.as_ref().is_none_or(|r| r.probed != n) {
            return;
        }
        let Some(Joined { stage: Stage::Connected(link), .. }) = &mut self.state.joined else { return };
        let _ = skyward_sys::kmsg(&format!("the network on {} {reach}", self.cfg.interface));
        link.reach = Some(reach);
    }

    /// Give the address back, and stop asking for one or keeping it.
    fn unaddress(&mut self) {
        let Some(r) = &mut self.running else { return };
        r.retry_at = None;
        if let Some((_, stop)) = r.asking.take() {
            stop.store(true, Ordering::Relaxed);
        }
        if r.lease.take().is_some() {
            if let Err(e) = iface::unconfigure(&self.cfg.interface) {
                let _ = skyward_sys::kmsg(&format!("{e:#}"));
            }
        }
        // Nothing goes over a network left, and counting starts afresh on
        // the next one.
        if r.counted.take().is_some() {
            (self.tell)(Report::Traffic(Traffic::default()));
        }
        self.servers.lock().expect("the DNS servers are not held by a thread that panicked").clear();
    }

    fn timers(&mut self) {
        let now = Instant::now();
        let Some(r) = &mut self.running else { return };
        if r.retry_at.is_some_and(|at| now >= at) {
            r.retry_at = None;
            return self.ask_address();
        }
        if r.asking.is_none() {
            if let Some(h) = &r.lease {
                if now >= h.due() {
                    if h.phase >= 2 {
                        // The lease ran out with no server to keep it.
                        self.unaddress();
                        if let Some(j) = &mut self.state.joined {
                            j.stage = Stage::Addressing;
                        }
                    }
                    return self.ask_address();
                }
            }
        }
        if now >= r.polled_at + POLL {
            self.poll_link();
        }
        if let Some(r) = &self.running {
            if r.lease.is_some() && r.counted.is_none_or(|c| now >= c.at + TRAFFIC) {
                self.count(now);
            }
        }
    }

    /// Read what has gone over the interface, and tell how fast it went
    /// since the last reading.
    fn count(&mut self, now: Instant) {
        let stats = Path::new("/sys/class/net").join(&self.cfg.interface).join("statistics");
        let read = |name: &str| -> Result<u64> {
            let path = stats.join(name);
            let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
            text.trim().parse().with_context(|| format!("{} holds no count", path.display()))
        };
        let (rx, tx) = match (read("rx_bytes"), read("tx_bytes")) {
            (Ok(rx), Ok(tx)) => (rx, tx),
            (Err(e), _) | (_, Err(e)) => return self.lost(e),
        };
        let Some(r) = &mut self.running else { return };
        if let Some(c) = r.counted {
            let secs = now.duration_since(c.at).as_secs_f64();
            let rate = |total: u64, before: u64| (total.saturating_sub(before) as f64 / secs).round() as u64;
            (self.tell)(Report::Traffic(Traffic { rx: rate(rx, c.rx), tx: rate(tx, c.tx) }));
        }
        r.counted = Some(Counted { at: now, rx, tx });
    }

    /// Read the network joined for its signal and speed.
    fn poll_link(&mut self) {
        let Some(r) = &mut self.running else { return };
        r.polled_at = Instant::now();
        let Some(Joined { stage: Stage::Connected(link), .. }) = &mut self.state.joined else { return };
        // A plain STATUS has Android's supplicant tell every listener again
        // that the network is connected; this one leaves that out.
        let (signal, status) = match (r.control.ask("SIGNAL_POLL"), r.control.ask("STATUS-NO_EVENTS")) {
            (Ok(signal), Ok(status)) => (signal, status),
            (Err(e), _) | (_, Err(e)) => return self.lost(e),
        };
        let fields = supplicant::fields(&signal);
        if let Some(rssi) = fields.get("RSSI").and_then(|v| v.parse().ok()) {
            link.signal = rssi;
        }
        if let Some(f) = fields.get("FREQUENCY").and_then(|v| v.parse().ok()) {
            link.frequency = f;
        }
        link.speed = fields.get("LINKSPEED").and_then(|v| v.parse().ok());
        link.generation = supplicant::fields(&status).get("wifi_generation").and_then(|v| v.parse().ok());
    }
}

/// Load the driver where its interface is not there yet, and bring the
/// interface up.
fn bring_up(cfg: &Config, modules: &[PathBuf]) -> Result<()> {
    let net = Path::new("/sys/class/net").join(&cfg.interface);
    if !net.exists() {
        for file in &cfg.firmware {
            place(file)?;
        }
        for path in skyward_modules::load(modules, &cfg.modules)? {
            skyward_sys::kmsg(&format!("loaded {}", path.display()))?;
        }
        for w in &cfg.writes {
            fs::write(&w.path, &w.value).with_context(|| format!("write {} to {}", w.value, w.path))?;
        }
        let until = Instant::now() + INTERFACE_WAIT;
        while !net.exists() {
            ensure!(Instant::now() < until, "the driver made no {} in {} seconds", cfg.interface, INTERFACE_WAIT.as_secs());
            thread::sleep(Duration::from_millis(100));
        }
    }
    // The node is not the interface yet: the driver takes it down and up
    // again as the firmware it starts with comes up, and until it settles
    // even its address cannot be read.
    let until = Instant::now() + INTERFACE_WAIT;
    while let Err(e) = iface::hardware_address(&cfg.interface) {
        ensure!(Instant::now() < until, "{} is there but cannot be read in {} seconds: {e}", cfg.interface, INTERFACE_WAIT.as_secs());
        thread::sleep(Duration::from_millis(100));
    }
    iface::set_up(&cfg.interface, true)
}

/// Put a file the driver asks for where the firmware loader looks.
fn place(file: &Firmware) -> Result<()> {
    let to = Path::new(FIRMWARE).join(&file.name);
    let dir = to.parent().expect("a firmware file is in a folder");
    fs::create_dir_all(dir).with_context(|| format!("make {}", dir.display()))?;
    let bytes = match &file.partition {
        None => fs::read(&file.path).with_context(|| format!("read {}", file.path))?,
        Some(partition) => {
            let at = Path::new(PARTITIONS).join(partition);
            fs::create_dir_all(&at).with_context(|| format!("make {}", at.display()))?;
            skyward_block::mount::mount_ro(&Path::new(skyward_block::BY_NAME).join(partition), &at)?;
            let read = fs::read(at.join(&file.path)).with_context(|| format!("read {} on {partition}", file.path));
            skyward_block::mount::umount(&at)?;
            read?
        }
    };
    fs::write(&to, bytes).with_context(|| format!("write {}", to.display()))
}

/// The folder the supplicant's configuration is kept in, and the file
/// itself: the network joined last comes back with the next boot. It is on
/// the data partition, which is encrypted and holds the password.
const KEPT: (&str, &str) = ("wlan", "wpa_supplicant.conf");

/// Where the supplicant's configuration is kept on the data partition, made
/// ready for the supplicant to write in, if that partition is open.
fn kept_dir() -> Option<PathBuf> {
    if !skyward_sys::kept::mounted(skyward_sys::kept::DATA) {
        return None;
    }
    let kept = skyward_sys::kept::folder(skyward_sys::kept::DATA, KEPT.0);
    // The supplicant runs as the wifi user, so it has to be able to write
    // its configuration back here; this phone's own build of it dies rather
    // than saying it cannot.
    let ready = fs::create_dir_all(&kept)
        .and_then(|()| std::os::unix::fs::chown(&kept, Some(WIFI), Some(WIFI)))
        .and_then(|()| fs::set_permissions(&kept, fs::Permissions::from_mode(0o770)));
    match ready {
        Ok(()) => Some(kept),
        Err(e) => {
            let _ = skyward_sys::kmsg(&format!("{} cannot hold the WLAN's networks: {e}", kept.display()));
            None
        }
    }
}

/// Where the supplicant keeps its configuration: the data partition once it
/// is open, and a home in memory until then, which forgets on a restart.
/// Start the supplicant on the interface, and wait for its control socket.
fn start_supplicant(cfg: &Config, libraries: Option<&Path>) -> Result<Child> {
    let home = Path::new(HOME);
    let sockets = home.join("sockets");
    fs::create_dir_all(&sockets).with_context(|| format!("make {}", sockets.display()))?;
    // The supplicant writes its configuration back beside its socket as the
    // wifi user, so both folders are its own: a save it cannot make answers
    // FAIL and the network is lost with the boot.
    std::os::unix::fs::chown(home, Some(WIFI), Some(WIFI)).with_context(|| format!("give {} to the supplicant", home.display()))?;
    fs::set_permissions(home, fs::Permissions::from_mode(0o770)).with_context(|| format!("open {} to the supplicant", home.display()))?;
    std::os::unix::fs::chown(&sockets, Some(WIFI), Some(WIFI)).with_context(|| format!("give {} to the supplicant", sockets.display()))?;
    fs::set_permissions(&sockets, fs::Permissions::from_mode(0o770)).with_context(|| format!("keep {} to the supplicant", sockets.display()))?;
    // The supplicant is never given the copy on the data partition: this
    // phone's own build of it ends with SIGSEGV on a configuration read from
    // there. The networks saved on the partition are read by the recovery's
    // own code once it is open, and taken in with `Adopt`.
    match kept_dir() {
        Some(dir) => {
            let _ = skyward_sys::kmsg(&format!("the WLAN's networks are kept in {}", dir.display()));
        }
        None => {
            let _ = skyward_sys::kmsg("the WLAN's networks wait for the data partition to be decrypted");
        }
    }
    let settings = home.join(KEPT.1);
    if !settings.exists() {
        fs::write(&settings, format!("ctrl_interface={}\nupdate_config=1\nap_scan=1\np2p_disabled=1\npmf=1\nsae_pwe=2\n", sockets.display()))
            .with_context(|| format!("write {}", settings.display()))?;
    }
    // The supplicant writes the file back when the user joins a network, as
    // it runs as the wifi user.
    let _ = std::os::unix::fs::chown(&settings, Some(WIFI), Some(WIFI));
    fs::set_permissions(&settings, fs::Permissions::from_mode(0o600)).with_context(|| format!("open {} to the supplicant", settings.display()))?;
    let socket = sockets.join(&cfg.interface);
    // One an earlier run left would answer nothing.
    if socket.exists() {
        fs::remove_file(&socket).with_context(|| format!("take away {}", socket.display()))?;
    }
    let mut path: Vec<String> = cfg.libraries.clone();
    if let Some(dir) = libraries {
        path.push(dir.to_string_lossy().into_owned());
    }
    let mut child = Process::new(&cfg.supplicant)
        .args(["-i", &cfg.interface, "-D", "nl80211", "-c"])
        .arg(&settings)
        .env("LD_LIBRARY_PATH", path.join(":"))
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("start {}", cfg.supplicant))?;
    let until = Instant::now() + SOCKET_WAIT;
    while !socket.exists() {
        if let Some(status) = child.try_wait().context("look at wpa_supplicant")? {
            bail!("wpa_supplicant ended as it started: {status}");
        }
        if Instant::now() >= until {
            end(&mut child, None);
            bail!("wpa_supplicant opened no control socket in {} seconds", SOCKET_WAIT.as_secs());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(child)
}

/// End the supplicant: asked to, where it can be, and killed after a while.
fn end(child: &mut Child, control: Option<&Control>) {
    if let Some(c) = control {
        let _ = c.ask("TERMINATE");
        let until = Instant::now() + END_WAIT;
        while Instant::now() < until {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}
