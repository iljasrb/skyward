//! Taking an image out of an update package on a thread of its own, telling
//! the screen how far it has come.

use anyhow::{bail, ensure, Context, Result};
use skyward_block::BY_NAME;
use skyward_config::{ramdisk, DeviceConfig};
use skyward_install::package::{self, Package, Phone};
use skyward_install::payload::{self, Cancelled};
use skyward_install::Refusal;
use skyward_sys::worker;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The most often how far the work has come is told.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

pub struct Task {
    pub package: PathBuf,
    pub partition: String,
    pub out: PathBuf,
}

pub enum Event {
    Progress { done: u64, total: u64 },
    Ended(End),
}

#[derive(Debug)]
pub enum End {
    Done { path: PathBuf, size: u64 },
    Stopped,
    Failed(String),
}

pub struct Worker {
    pub worker: worker::Worker<Event>,
    cancel: Arc<AtomicBool>,
}

impl Worker {
    pub fn start(task: Task) -> Result<Worker> {
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let worker = worker::Worker::start("extract", "the package extractor", move |tell| {
            let say = |event: Event| tell.send(event);
            let ended = extract(&task, &say, &stop);
            let end = match ended {
                Ok((path, size)) => End::Done { path, size },
                Err(e) if e.is::<Cancelled>() => End::Stopped,
                Err(e) => End::Failed(format!("{e:#}")),
            };
            let _ = skyward_sys::kmsg(&format!("extract: {end:?}"));
            say(Event::Ended(end));
        })?;
        Ok(Worker { worker, cancel })
    }

    /// Ask the work to stop; it does between two chunks, and takes away what
    /// it wrote.
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn extract(task: &Task, say: &impl Fn(Event), stop: &Arc<AtomicBool>) -> Result<(PathBuf, u64)> {
    let mut file = std::fs::File::open(&task.package).with_context(|| format!("open {}", task.package.display()))?;
    let update = match package::identify(&mut file)? {
        Package::Update(update) => update,
        _ => bail!("{} holds no update payload", task.package.display()),
    };
    // A package for another phone may hold images with the same names; they
    // are not this phone's firmware, so nothing is taken out of it.
    if let Err(refusal) = package::check_device(&update, &phone()?) {
        match refusal {
            Refusal::OtherDevice(devices) if devices.is_empty() => bail!("{} names no phone", task.package.display()),
            Refusal::OtherDevice(devices) => bail!("{} is for {devices}, not this phone", task.package.display()),
            Refusal::OtherSerial => bail!("{} is for another phone's serial number", task.package.display()),
            other => bail!("{} cannot be used: {other:?}", task.package.display()),
        }
    }
    let manifest = payload::read(&mut file, update.payload_offset, update.payload_size)?;
    let partition = manifest
        .partitions
        .iter()
        .find(|partition| partition.name == task.partition)
        .with_context(|| format!("{} holds no {} partition", task.package.display(), task.partition))?;
    if !partition.carried() {
        bail!("{} is written by steps this code does not carry out ({})", partition.name, partition.uncarried().join("- Done"));
    }
    // An image larger than the partition it goes to cannot be written, so
    // do not spend the time and the room taking it out.
    if let Some(slot) = skyward_vab::current_slot()? {
        let target = Path::new(BY_NAME).join(format!("{}{}", partition.name, slot.suffix()));
        if let Ok(size) = skyward_block::size_bytes(&target) {
            ensure!(
                partition.size <= size,
                "{} is {} bytes, more than the {} its partition holds",
                partition.name,
                partition.size,
                size
            );
        }
    }
    let mut last = Instant::now() - PROGRESS_EVERY;
    let mut progress = |done, total| {
        if last.elapsed() >= PROGRESS_EVERY {
            say(Event::Progress { done, total });
            last = Instant::now();
        }
    };
    payload::extract_while(
        &mut file,
        manifest.data_at,
        manifest.block(),
        partition,
        &task.out,
        &mut progress,
        &mut || stop.load(Ordering::Relaxed),
    )?;
    Ok((task.out.clone(), partition.size))
}

/// The phone this recovery is for, as the package's metadata names it.
fn phone() -> Result<Phone> {
    let cfg = DeviceConfig::load(Path::new("/").join(ramdisk::CONFIG))?;
    Ok(Phone { device: cfg.device.codename, serial: skyward_sys::prop("ro.serialno").unwrap_or_default(), installed: None })
}
