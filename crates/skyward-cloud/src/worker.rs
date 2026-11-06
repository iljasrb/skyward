//! The cloud storage on a thread of its own: testing a server, mounting it,
//! and letting it go, telling the screen how each went.

use crate::config::Config;
use crate::fuse::{mount, Mount};
use crate::webdav::{Info, WebDav};
use skyward_net::http::Servers;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

pub enum Command {
    Test(Config),
    Mount(Config),
    Unmount,
    Forget,
    Stop,
}

#[derive(Debug)]
pub enum Event {
    Tested(Result<Info, String>),
    Mounted(Result<Info, String>),
    Unmounted(Result<(), String>),
    Forgotten(Result<(), String>),
}

struct Tell {
    events: Sender<Event>,
    wake: Arc<OwnedFd>,
}

impl Tell {
    fn send(&self, event: Event) {
        if self.events.send(event).is_ok() {
            unsafe { libc::write(self.wake.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
        }
    }
}

pub struct Worker {
    inbox: Sender<Command>,
    told: Receiver<Event>,
    wake: OwnedFd,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn start(servers: Servers) -> std::io::Result<Worker> {
        let mut fds = [0; 2];
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let (wake, signal) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        let (tx, inbox) = mpsc::channel();
        let (events, told) = mpsc::channel();
        let tell = Tell { events, wake: Arc::new(signal) };
        let thread = thread::Builder::new().name("cloud".into()).spawn(move || run(inbox, tell, servers))?;
        Ok(Worker { inbox: tx, told, wake, thread: Some(thread) })
    }

    pub fn send(&self, command: Command) -> Result<(), String> {
        self.inbox.send(command).map_err(|_| "the cloud storage thread has ended".to_string())
    }

    pub fn fd(&self) -> RawFd {
        self.wake.as_raw_fd()
    }

    pub fn events(&self) -> Vec<Event> {
        let mut bytes = [0u8; 64];
        while unsafe { libc::read(self.wake.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) } > 0 {}
        self.told.try_iter().collect()
    }

    pub fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.inbox.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(inbox: Receiver<Command>, tell: Tell, servers: Servers) {
    let mut mounted: Option<Mount> = None;
    while let Ok(command) = inbox.recv() {
        match command {
            Command::Test(config) => {
                let result = WebDav::new(&config, servers.clone()).and_then(|server| server.test()).map_err(|e| e.to_string());
                tell.send(Event::Tested(result));
            }
            Command::Mount(config) => {
                if mounted.is_some() {
                    tell.send(Event::Mounted(Err("the cloud storage is mounted already".into())));
                    continue;
                }
                let result = (|| {
                    let server = WebDav::new(&config, servers.clone()).map_err(|e| e.to_string())?;
                    let info = server.test().map_err(|e| e.to_string())?;
                    let mount = mount(Arc::new(server)).map_err(|e| format!("{e:#}"))?;
                    Ok::<_, String>((info, mount))
                })();
                match result {
                    Ok((info, mount)) => {
                        mounted = Some(mount);
                        tell.send(Event::Mounted(Ok(info)));
                    }
                    Err(e) => tell.send(Event::Mounted(Err(e))),
                }
            }
            Command::Unmount => {
                let result = match mounted.take() {
                    Some(mount) => mount.unmount().map_err(|e| format!("{e:#}")),
                    None => Ok(()),
                };
                tell.send(Event::Unmounted(result));
            }
            Command::Forget => {
                let result = match mounted.take() {
                    Some(mount) => mount.unmount().map_err(|e| format!("{e:#}")),
                    None => Ok(()),
                };
                tell.send(Event::Forgotten(result));
            }
            Command::Stop => {
                if let Some(mount) = mounted.take() {
                    let _ = mount.unmount();
                }
                return;
            }
        }
    }
    if let Some(mount) = mounted.take() {
        let _ = mount.unmount();
    }
}
