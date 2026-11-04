//! Downloading a package whole: from where an earlier download of it
//! stopped, over connections that may drop, and checked against the SHA-256
//! its server gave before it takes its own name.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use skyward_files::hex;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// How much is read and written at a time.
const CHUNK: usize = 1 << 20;

/// Where a package is downloaded to until it is whole and checked.
pub fn partial(path: &Path) -> PathBuf {
    let mut name = path.file_name().expect("a package has a name").to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

/// How far a download has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// What an earlier download left is read for its SHA-256, before the rest
    /// is added to it.
    Checking { done: u64, total: u64 },
    Downloading { done: u64, total: u64 },
}

/// Why a download did not end with the package whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The folder has too little room for what is left of it.
    NoRoom { needed: u64, free: u64 },
    /// What was downloaded was not the package its server described, and
    /// is gone.
    Damaged,
    /// The server now offers another system than the one downloaded.
    Changed,
    /// The connection kept failing, or anything else: why.
    Other(String),
}

/// The server turned the download down: what a signed address that has
/// run out gets, or one it no longer serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused(pub u16);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the server answered {}", self.0)
    }
}

impl std::error::Error for Refused {}

/// The whole file does not have the SHA-256 its release gave; what was
/// downloaded of it is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Damaged;

impl std::fmt::Display for Damaged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the file downloaded does not have the SHA-256 it was published with")
    }
}

impl std::error::Error for Damaged {}

/// The folder has too little room for what is left to download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoRoom {
    pub needed: u64,
    pub free: u64,
}

impl std::fmt::Display for NoRoom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} bytes more are needed, and {} are free", self.needed, self.free)
    }
}

impl std::error::Error for NoRoom {}

/// The bytes free for anyone to write in the filesystem `dir` is on.
pub fn free(dir: &Path) -> Result<u64> {
    let c = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).context("a path with a NUL")?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| format!("read the free space of {}", dir.display()));
    }
    Ok(s.f_bavail as u64 * s.f_frsize as u64)
}

/// The SHA-256 of what `file` holds, read from its start.
fn hash_of(file: &mut File, total: u64, stop: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> Result<Sha256> {
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            bail!(crate::Stopped);
        }
        let n = file.read(&mut buf).context("read what was downloaded before")?;
        if n == 0 {
            return Ok(hash);
        }
        hash.update(&buf[..n]);
        done += n as u64;
        progress(Progress::Checking { done, total });
    }
}

/// Download `url` to `path`, carrying on from what an earlier download
/// left, and give it its name once its SHA-256 is `sha256`. A part as long as the
/// package is only checked; a server that will not send from the middle
/// sends it all again.
pub fn fetch(agent: &ureq::Agent, url: &str, path: &Path, sha256: &str, stop: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> Result<()> {
    let part = partial(path);
    let dir = path.parent().context("a package's path is in a folder")?;
    fs::create_dir_all(dir).with_context(|| format!("make {}", dir.display()))?;
    let head = agent.head(url).call().context("ask the server about the package")?;
    match head.status().as_u16() {
        200 => {}
        status => bail!(Refused(status)),
    }
    let total = content_length(head.headers()).context("the server does not say how long the package is")?;
    let mut file = OpenOptions::new().create(true).read(true).append(true).open(&part).with_context(|| format!("open {}", part.display()))?;
    let mut have = file.metadata().with_context(|| format!("read {}", part.display()))?.len();
    if have > total {
        file.set_len(0).with_context(|| format!("empty {}", part.display()))?;
        have = 0;
    }
    let room = free(dir)?;
    if total - have > room {
        bail!(NoRoom { needed: total - have, free: room });
    }
    let mut hash = hash_of(&mut file, have, stop, progress)?;
    if have < total {
        let mut asked = agent.get(url);
        if have > 0 {
            asked = asked.header("Range", format!("bytes={have}-"));
        }
        let mut response = asked.call().context("start the download")?;
        match response.status().as_u16() {
            206 if have > 0 => {
                // A part is only appended to when the server sends the bytes
                // that come next; anything else is started over.
                let Some((start, whole)) = content_range(response.headers()) else {
                    file.set_len(0).with_context(|| format!("empty {}", part.display()))?;
                    bail!(Refused(416));
                };
                if start != have || whole != total {
                    file.set_len(0).with_context(|| format!("empty {}", part.display()))?;
                    bail!(Refused(416));
                }
            }
            200 => {
                if have > 0 {
                    file.set_len(0).with_context(|| format!("empty {}", part.display()))?;
                    have = 0;
                    hash = Sha256::new();
                }
            }
            // The part is longer than what the server now has: start over.
            416 => {
                file.set_len(0).with_context(|| format!("empty {}", part.display()))?;
                bail!(Refused(416));
            }
            status => bail!(Refused(status)),
        }
        let mut body = response.body_mut().with_config().limit(u64::MAX).reader();
        let mut buf = vec![0u8; CHUNK];
        let mut done = have;
        progress(Progress::Downloading { done, total });
        while done < total {
            if stop.load(Ordering::Relaxed) {
                bail!(crate::Stopped);
            }
            let n = body.read(&mut buf).context("download the package")?;
            if n == 0 {
                bail!("the server stopped sending at {done} of {total} bytes");
            }
            let n = n.min((total - done) as usize);
            file.write_all(&buf[..n]).with_context(|| format!("write {}", part.display()))?;
            hash.update(&buf[..n]);
            done += n as u64;
            progress(Progress::Downloading { done, total });
        }
        file.sync_all().with_context(|| format!("write {} through", part.display()))?;
    }
    if hex(&hash.finalize()) != sha256.to_lowercase() {
        drop(file);
        fs::remove_file(&part).with_context(|| format!("delete {}", part.display()))?;
        bail!(Damaged);
    }
    fs::rename(&part, path).with_context(|| format!("name {} {}", part.display(), path.display()))
}

/// The length a response says its body has.
pub(crate) fn content_length(headers: &ureq::http::HeaderMap) -> Option<u64> {
    headers.get(ureq::http::header::CONTENT_LENGTH)?.to_str().ok()?.parse().ok()
}

/// The first byte and the whole length a `Content-Range: bytes a-b/n` names.
fn content_range(headers: &ureq::http::HeaderMap) -> Option<(u64, u64)> {
    let value = headers.get(ureq::http::header::CONTENT_RANGE)?.to_str().ok()?;
    let (range, whole) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, _) = range.split_once('-')?;
    Some((start.parse().ok()?, whole.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_part_is_named_after_its_package() {
        assert_eq!(partial(Path::new("/sdcard/downloaded_rom/a.zip")), Path::new("/sdcard/downloaded_rom/a.zip.part"));
    }

    #[test]
    fn what_is_there_is_read_for_its_sha256() {
        let dir = std::env::temp_dir().join(format!("skyward-net-hash-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.part");
        fs::write(&path, vec![7u8; CHUNK + 5]).unwrap();
        let mut seen = Vec::new();
        let hash = hash_of(&mut File::open(&path).unwrap(), (CHUNK + 5) as u64, &AtomicBool::new(false), &mut |p| seen.push(p)).unwrap();
        assert_eq!(hex(&hash.finalize()), hex(&Sha256::digest(vec![7u8; CHUNK + 5])));
        assert_eq!(seen.last(), Some(&Progress::Checking { done: (CHUNK + 5) as u64, total: (CHUNK + 5) as u64 }));
        let stopped = hash_of(&mut File::open(&path).unwrap(), 1, &AtomicBool::new(true), &mut |_| {});
        assert!(stopped.unwrap_err().is::<crate::Stopped>());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_range_a_part_is_sent_from_is_read() {
        let mut headers = ureq::http::HeaderMap::new();
        headers.insert(ureq::http::header::CONTENT_RANGE, "bytes 10-19/100".parse().unwrap());
        assert_eq!(content_range(&headers), Some((10, 100)));
        headers.insert(ureq::http::header::CONTENT_RANGE, "bytes 10-19/*".parse().unwrap());
        assert_eq!(content_range(&headers), None, "a length it does not say");
    }

    #[test]
    fn free_space_is_read_from_the_filesystem() {
        assert!(free(&std::env::temp_dir()).unwrap() > 0);
        assert!(free(Path::new("/no/such/folder")).is_err());
    }
}
