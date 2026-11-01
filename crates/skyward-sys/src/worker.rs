//! Work on a thread of its own, telling the screen as it goes: what it makes
//! is queued and the wake pipe written, so a poll on `fd()` says when to call
//! `events()`.

use anyhow::{Context, Result};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

/// The work's end of a [`Worker`]: it queues what the screen is to be told
/// and wakes it.
pub struct Tell<T> {
    events: Sender<T>,
    /// The write end of the wake pipe.
    wake: Arc<OwnedFd>,
}

impl<T> Clone for Tell<T> {
    fn clone(&self) -> Self {
        Tell { events: self.events.clone(), wake: self.wake.clone() }
    }
}

impl<T> Tell<T> {
    /// Tell the screen `event`, waking it.
    pub fn send(&self, event: T) {
        if self.events.send(event).is_ok() {
            // A full pipe already wakes the screen.
            unsafe { libc::write(self.wake.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
        }
    }
}

/// Work on a thread of its own, joined once let go.
pub struct Worker<T> {
    events: Receiver<T>,
    /// Readable while events wait.
    wake: OwnedFd,
    thread: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> Worker<T> {
    /// Run `work` on a thread named `name`, telling the screen through the
    /// `Tell` it is given; `who` names it in what could not be started.
    pub fn start(name: &str, who: &str, work: impl FnOnce(Tell<T>) + Send + 'static) -> Result<Worker<T>> {
        let mut fds = [0; 2];
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| format!("make {who}'s wake pipe"));
        }
        let (wake, signal) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        let (tx, events) = mpsc::channel();
        let tell = Tell { events: tx, wake: Arc::new(signal) };
        let thread = thread::Builder::new().name(name.into()).spawn(move || work(tell)).with_context(|| format!("start {who}"))?;
        Ok(Worker { events, wake, thread: Some(thread) })
    }

    pub fn fd(&self) -> RawFd {
        self.wake.as_raw_fd()
    }

    /// The events since the last call.
    pub fn events(&self) -> Vec<T> {
        self.drain();
        self.events.try_iter().collect()
    }

    /// The one event the work ends with, once it has.
    pub fn recv(&self) -> Option<T> {
        self.drain();
        self.events.try_recv().ok()
    }

    /// Whether the work is no longer running: it ended, or failed.
    pub fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    fn drain(&self) {
        let mut bytes = [0u8; 64];
        while unsafe { libc::read(self.wake.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) } > 0 {}
    }
}

impl<T> Drop for Worker<T> {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            thread.join().expect("the worker does not panic");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(worker: &Worker<u8>) {
        while !worker.finished() {
            thread::yield_now();
        }
    }

    #[test]
    fn what_the_work_tells_comes_back_through_the_worker() {
        let worker = Worker::start("test", "the test worker", |tell| {
            tell.send(1);
            tell.send(2);
        })
        .unwrap();
        settle(&worker);
        assert_eq!(worker.events(), [1, 2], "each event once, in the order it was told");
        assert_eq!(worker.events(), [], "and none twice");
        assert!(worker.fd() > 0, "the wake pipe the screen waits on");
    }

    #[test]
    fn one_event_is_taken_once() {
        let worker = Worker::start("test", "the test worker", |tell| tell.send(7)).unwrap();
        settle(&worker);
        assert_eq!(worker.recv(), Some(7));
        assert_eq!(worker.recv(), None);
    }
}
