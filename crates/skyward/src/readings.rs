//! The phone's readings taken on a thread of their own, so the screen keeps
//! up: the battery's driver takes milliseconds to answer each read.

use anyhow::{Context, Result};
use skyward_config::DeviceConfig;
use skyward_info::{Battery, DeviceInfo};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// The battery and the port are read this often, so plugging the cable in or
/// pulling it shows at once; all the readings every so many times instead.
const BATTERY_EVERY: Duration = Duration::from_secs(1);
const ALL_EVERY: u32 = 30;

pub enum Reading {
    /// The battery and whether a computer has the USB port.
    Status(Result<Option<Battery>>, bool),
    /// All the readings, and when taking them started.
    All(Box<Result<DeviceInfo>>, Instant),
}

pub struct Readings {
    taken: Receiver<Reading>,
    /// Readable while readings wait.
    wake: OwnedFd,
}

impl Readings {
    /// Start taking the readings of the phone `cfg` describes, the first a
    /// second from now. The thread ends once the screen lets them go.
    pub fn start(cfg: &DeviceConfig) -> Result<Readings> {
        let mut fds = [0; 2];
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return Err(std::io::Error::last_os_error()).context("make the readings' wake pipe");
        }
        let (wake, signal) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        let (tx, taken) = mpsc::channel();
        let cfg = cfg.clone();
        thread::Builder::new()
            .name("readings".into())
            .spawn(move || {
                for k in 1u32.. {
                    thread::sleep(BATTERY_EVERY);
                    let reading = if k % ALL_EVERY == 0 {
                        let at = Instant::now();
                        Reading::All(Box::new(skyward_info::collect(&cfg)), at)
                    } else {
                        Reading::Status(skyward_info::battery_now(), skyward_info::usb_connected())
                    };
                    if tx.send(reading).is_err() {
                        return;
                    }
                    unsafe { libc::write(signal.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
                }
            })
            .context("start the readings thread")?;
        Ok(Readings { taken, wake })
    }

    pub fn fd(&self) -> RawFd {
        self.wake.as_raw_fd()
    }

    /// The readings taken since the last call.
    pub fn taken(&self) -> Vec<Reading> {
        let mut drain = [0u8; 64];
        while unsafe { libc::read(self.wake.as_raw_fd(), drain.as_mut_ptr().cast(), drain.len()) } > 0 {}
        self.taken.try_iter().collect()
    }
}
