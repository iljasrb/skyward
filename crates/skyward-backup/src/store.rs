//! The files a part of a backup is kept in: what the part holds, compressed
//! or not, cut into files of a size every filesystem a backup may go to
//! takes, with a digest of what it holds to check it by. The zeros a part
//! holds are left as holes in those files, so a partition's unused stretch
//! takes no room on the storage and still reads back as the zeros it is.

use anyhow::{ensure, Context, Result};
use lz4_flex::frame::{BlockSize, FrameDecoder, FrameEncoder, FrameInfo};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use skyward_files::{hex, Ownership};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The most one file holds: FAT, which USB drives often hold, takes no file
/// of 4 GiB or more.
pub const FILE_BYTES: u64 = 1 << 30;

/// A run of zeros this long is left as a hole in the file rather than
/// written: a partition's unused stretch takes no room then, and reads back
/// as the zeros it is. Shorter runs are written out, as a hole costs the
/// filesystem bookkeeping of its own.
const SPARSE_RUN: usize = 4096;

/// A part as its files keep it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    /// The files, in order, in the backup's folder.
    pub files: Vec<String>,
    /// The bytes the part holds, and the bytes its files take.
    pub size: u64,
    pub stored: u64,
    pub compressed: bool,
    /// The SHA-256 of what the part holds, in hex.
    pub sha256: String,
}

/// Files in `folder` named `<name>.000` on, each cut at `limit` bytes and
/// given what `ownership` says.
struct Files {
    folder: PathBuf,
    name: String,
    ownership: Ownership,
    limit: u64,
    open: Option<File>,
    in_open: u64,
    names: Vec<String>,
    written: u64,
    /// Whether the zeros a part holds are left as holes, which stops once the
    /// storage refuses one or the backup asks for none.
    sparse: bool,
}

impl Files {
    /// Write the file open to the storage, and make the next one.
    fn next(&mut self) -> io::Result<()> {
        if let Some(full) = self.open.take() {
            full.set_len(self.in_open)?;
            full.sync_all()?;
        }
        let name = format!("{}.{:03}", self.name, self.names.len());
        let file = skyward_files::make_file(&self.folder.join(&name), self.ownership).map_err(io::Error::other)?;
        self.names.push(name);
        self.open = Some(file);
        self.in_open = 0;
        Ok(())
    }

    /// Write what is left to the storage, and give the names of the files.
    fn finish(mut self) -> io::Result<(Vec<String>, u64)> {
        if self.open.is_none() {
            self.next()?;
        }
        if let Some(last) = self.open.take() {
            last.set_len(self.in_open)?;
            last.sync_all()?;
        }
        Ok((self.names, self.written))
    }

    /// Leave `len` bytes of the file open as zeros it does not hold, or write
    /// the zeros out where the storage keeps no holes.
    fn hole(&mut self, len: usize) -> io::Result<()> {
        if self.sparse {
            let file = self.open.as_mut().expect("a file is open");
            if file.seek(SeekFrom::Current(len as i64)).is_ok() {
                self.in_open += len as u64;
                return Ok(());
            }
            // The storage does not skip the zeros a seek leaves: every file
            // it is given takes the zeros of the part.
            self.sparse = false;
        }
        let zeros = [0u8; SPARSE_RUN];
        let mut left = len;
        while left > 0 {
            let take = left.min(SPARSE_RUN);
            let n = self.open.as_mut().expect("a file is open").write(&zeros[..take])?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            self.in_open += n as u64;
            self.written += n as u64;
            left -= n;
        }
        Ok(())
    }
}

impl Write for Files {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.open.is_none() || self.in_open == self.limit {
            self.next()?;
        }
        let room = (self.limit - self.in_open).min(buf.len() as u64) as usize;
        let (kept, zeros) = match self.sparse {
            true => split_zero_run(&buf[..room]),
            false => (room, 0),
        };
        if kept == 0 {
            self.hole(zeros)?;
            return Ok(zeros);
        }
        let n = self.open.as_mut().expect("a file is open").write(&buf[..kept])?;
        self.in_open += n as u64;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.open.as_mut().map_or(Ok(()), |f| f.flush())
    }
}

/// How much of `buf` is written, and how many zeros after it a hole takes
/// instead: all of it when it holds no run of [`SPARSE_RUN`], and none of it
/// when it starts with one.
fn split_zero_run(buf: &[u8]) -> (usize, usize) {
    let mut at = 0;
    while at < buf.len() {
        if buf[at] != 0 {
            at += 1;
            continue;
        }
        let start = at;
        while at < buf.len() && buf[at] == 0 {
            at += 1;
        }
        if at - start >= SPARSE_RUN {
            return (start, at - start);
        }
    }
    (buf.len(), 0)
}

enum Sink {
    Plain(Files),
    Lz4(FrameEncoder<Files>),
}

/// A part being written to its files.
pub struct Writer {
    sink: Sink,
    digest: Sha256,
    size: u64,
}

impl Writer {
    /// Begin the part called `name` in the backup's `folder`, its files
    /// given what `ownership` says, and the zeros it holds left as holes
    /// unless `sparse` says not to.
    pub fn create(folder: &Path, name: &str, compress: bool, sparse: bool, ownership: Ownership) -> Writer {
        Writer::cut_at(folder, name, compress, sparse, ownership, FILE_BYTES)
    }

    fn cut_at(folder: &Path, name: &str, compress: bool, sparse: bool, ownership: Ownership, limit: u64) -> Writer {
        let files =
            Files { folder: folder.to_path_buf(), name: name.into(), ownership, limit, open: None, in_open: 0, names: Vec::new(), written: 0, sparse };
        let sink = match compress {
            true => {
                let info = FrameInfo::new().block_size(BlockSize::Max4MB).content_checksum(true);
                Sink::Lz4(FrameEncoder::with_frame_info(info, files))
            }
            false => Sink::Plain(files),
        };
        Writer { sink, digest: Sha256::new(), size: 0 }
    }

    /// Write the rest to the storage, and give what the part's files keep.
    pub fn finish(self) -> Result<Stored> {
        let (files, compressed) = match self.sink {
            Sink::Plain(files) => (files, false),
            Sink::Lz4(encoder) => (encoder.finish().context("finish compressing")?, true),
        };
        let (files, stored) = files.finish().context("write the backup's files")?;
        Ok(Stored { files, size: self.size, stored, compressed, sha256: hex(&self.digest.finalize()) })
    }
}

impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = match &mut self.sink {
            Sink::Plain(files) => files.write(buf)?,
            Sink::Lz4(encoder) => encoder.write(buf)?,
        };
        self.digest.update(&buf[..n]);
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.sink {
            Sink::Plain(files) => files.flush(),
            Sink::Lz4(encoder) => encoder.flush(),
        }
    }
}

/// A part's files read one after another, as one.
struct Joined {
    folder: PathBuf,
    names: Vec<String>,
    next: usize,
    open: Option<BufReader<File>>,
}

impl Read for Joined {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if let Some(file) = &mut self.open {
                let n = file.read(buf)?;
                if n > 0 || buf.is_empty() {
                    return Ok(n);
                }
            }
            let Some(name) = self.names.get(self.next) else { return Ok(0) };
            let path = self.folder.join(name);
            let file = File::open(&path).map_err(|e| io::Error::new(e.kind(), format!("open {}: {e}", path.display())))?;
            self.open = Some(BufReader::with_capacity(1 << 20, file));
            self.next += 1;
        }
    }
}

/// A part read back from its files, what it holds counted and digested as
/// it goes.
pub struct Reader {
    source: Box<dyn Read + Send>,
    digest: Sha256,
    size: u64,
}

impl Reader {
    pub fn open(folder: &Path, stored: &Stored) -> Reader {
        let joined = Joined { folder: folder.to_path_buf(), names: stored.files.clone(), next: 0, open: None };
        let source: Box<dyn Read + Send> = match stored.compressed {
            true => Box::new(FrameDecoder::new(joined)),
            false => Box::new(joined),
        };
        Reader { source, digest: Sha256::new(), size: 0 }
    }

    /// Once all of it is read: whether it is what was stored.
    pub fn check(self, stored: &Stored) -> Result<()> {
        ensure!(self.size == stored.size, "the backup holds {} bytes where {} were stored", self.size, stored.size);
        let digest = hex(&self.digest.finalize());
        ensure!(digest == stored.sha256, "the backup does not hold what was stored: its digest is {digest}, {} was stored", stored.sha256);
        Ok(())
    }
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.source.read(buf)?;
        self.digest.update(&buf[..n]);
        self.size += n as u64;
        Ok(n)
    }
}

/// The SHA-256 of the first `len` bytes `from` gives, in hex as a part's is
/// kept: a partition read back once a part is written to it, to see that
/// it holds the part. Tells how many bytes are read so far.
pub fn digest(mut from: impl Read, len: u64, progress: &mut dyn FnMut(u64)) -> io::Result<String> {
    let mut digest = Sha256::new();
    let mut buf = vec![0u8; 4 << 20];
    let mut done = 0u64;
    while done < len {
        let want = (len - done).min(buf.len() as u64) as usize;
        from.read_exact(&mut buf[..want])?;
        digest.update(&buf[..want]);
        done += want as u64;
        progress(done);
    }
    Ok(hex(&digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(test: &str) -> PathBuf {
        let at = std::env::temp_dir().join(format!("skyward-store-{test}-{}", std::process::id()));
        if at.exists() {
            fs::remove_dir_all(&at).unwrap();
        }
        fs::create_dir_all(&at).unwrap();
        at
    }

    /// Bytes that compress some way but not all the way.
    fn sample(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8 ^ (i / 997) as u8).collect()
    }

    #[test]
    fn a_part_is_cut_into_files_and_read_back_whole() {
        let at = scratch("cut");
        for compress in [false, true] {
            let name = if compress { "packed" } else { "plain" };
            let held = sample(2_500);
            let mut w = Writer::cut_at(&at, name, compress, true, Ownership::Kept, 1_000);
            w.write_all(&held).unwrap();
            let stored = w.finish().unwrap();
            assert_eq!((stored.size, stored.compressed), (2_500, compress));
            if !compress {
                assert_eq!(stored.files, ["plain.000", "plain.001", "plain.002"], "cut at the limit");
                assert_eq!(stored.stored, 2_500);
            }
            let mut r = Reader::open(&at, &stored);
            let mut back = Vec::new();
            r.read_to_end(&mut back).unwrap();
            assert_eq!(back, held);
            r.check(&stored).unwrap();
        }
        fs::remove_dir_all(&at).unwrap();
    }

    #[test]
    fn a_part_that_changed_after_it_was_stored_fails_its_check() {
        let at = scratch("changed");
        let mut w = Writer::cut_at(&at, "boot", false, true, Ownership::Kept, 1_000);
        w.write_all(&sample(1_500)).unwrap();
        let stored = w.finish().unwrap();
        let second = at.join("boot.001");
        let mut bytes = fs::read(&second).unwrap();
        bytes[10] ^= 1;
        fs::write(&second, bytes).unwrap();
        let mut r = Reader::open(&at, &stored);
        io::copy(&mut r, &mut io::sink()).unwrap();
        assert!(r.check(&stored).is_err());
        fs::remove_file(&second).unwrap();
        let mut r = Reader::open(&at, &stored);
        assert!(io::copy(&mut r, &mut io::sink()).is_err(), "a file gone is an error");
        fs::remove_dir_all(&at).unwrap();
    }

    #[test]
    fn what_is_read_back_digests_as_the_part_was_stored() {
        let at = scratch("digest");
        let held = sample(3_000);
        let mut w = Writer::cut_at(&at, "dtbo", true, true, Ownership::Kept, 1_000);
        w.write_all(&held).unwrap();
        let stored = w.finish().unwrap();
        // A partition is larger than what went into it; only the part's
        // bytes count.
        let mut partition = held.clone();
        partition.extend([0xff; 512]);
        let mut told = 0;
        assert_eq!(digest(partition.as_slice(), stored.size, &mut |n| told = n).unwrap(), stored.sha256);
        assert_eq!(told, 3_000);
        partition[2_999] ^= 1;
        assert_ne!(digest(partition.as_slice(), stored.size, &mut |_| {}).unwrap(), stored.sha256);
        assert!(digest(&held[..10], stored.size, &mut |_| {}).is_err(), "a partition too small to hold the part");
        fs::remove_dir_all(&at).unwrap();
    }

    #[test]
    fn the_zeros_a_part_holds_take_no_room_in_the_backup() {
        let at = scratch("sparse");
        let mut held = sample(1_000);
        held.extend(vec![0u8; 8 << 20]);
        held.extend(vec![0xab; 1_500]);
        let mut w = Writer::cut_at(&at, "super", false, true, Ownership::Kept, 1 << 30);
        w.write_all(&held).unwrap();
        let stored = w.finish().unwrap();
        assert_eq!(stored.size, held.len() as u64, "the part is as long as it was");
        assert_eq!(stored.stored, 2_500, "the zeros took no room on the disk");
        let taken = fs::metadata(at.join("super.000")).unwrap();
        assert_eq!(taken.len(), held.len() as u64, "the file is as long as the part");
        use std::os::unix::fs::MetadataExt;
        assert!(taken.blocks() * 512 < 1 << 20, "and holds only what is not zeros: {} bytes", taken.blocks() * 512);
        let mut r = Reader::open(&at, &stored);
        let mut back = Vec::new();
        r.read_to_end(&mut back).unwrap();
        assert_eq!(back, held, "the zeros read back as the zeros they are");
        r.check(&stored).unwrap();
        fs::remove_dir_all(&at).unwrap();
    }

    #[test]
    fn zeros_at_the_end_of_a_part_keep_its_length() {
        let at = scratch("trailing");
        let mut held = sample(500);
        held.extend(vec![0u8; 20 << 20]);
        let mut w = Writer::create(&at, "boot", false, true, Ownership::Kept);
        w.write_all(&held).unwrap();
        let stored = w.finish().unwrap();
        assert_eq!(stored.size, held.len() as u64);
        assert_eq!(stored.stored, 500, "only what is not zeros takes room");
        let last = fs::metadata(at.join(&stored.files[0])).unwrap();
        assert_eq!(last.len(), held.len() as u64, "the file ends where the part does, in a hole no byte was written for");
        let mut r = Reader::open(&at, &stored);
        let mut back = Vec::new();
        r.read_to_end(&mut back).unwrap();
        assert_eq!(back, held, "the part is read back whole, its last files all zeros");
        r.check(&stored).unwrap();
        fs::remove_dir_all(&at).unwrap();
    }

    #[test]
    fn a_part_that_is_all_zeros_is_a_file_of_its_length_with_nothing_in_it() {
        use std::os::unix::fs::MetadataExt;
        let at = scratch("zeros");
        let held = vec![0u8; 3 << 20];
        let mut w = Writer::cut_at(&at, "misc", false, true, Ownership::Kept, 1 << 20);
        w.write_all(&held).unwrap();
        let stored = w.finish().unwrap();
        assert_eq!((stored.size, stored.stored), (held.len() as u64, 0), "a part of zeros takes no room at all");
        assert_eq!(stored.files, ["misc.000", "misc.001", "misc.002"], "and is cut where it was asked to be");
        for name in &stored.files {
            let file = fs::metadata(at.join(name)).unwrap();
            assert_eq!((file.len(), file.blocks()), (1 << 20, 0), "{name} is a megabyte long and holds no block");
        }
        let mut r = Reader::open(&at, &stored);
        let mut back = Vec::new();
        r.read_to_end(&mut back).unwrap();
        assert_eq!(back, held, "every byte of it reads back as the zero it is");
        r.check(&stored).unwrap();
        fs::remove_dir_all(&at).unwrap();
    }

    #[test]
    fn the_zeros_are_cut_into_files_like_the_rest_of_the_part() {
        let at = scratch("straddle");
        let mut held = vec![0x11; 5_000];
        held.extend(vec![0u8; 40_000]);
        held.extend(vec![0x22; 5_000]);
        let mut w = Writer::cut_at(&at, "boot", false, true, Ownership::Kept, 8 << 10);
        w.write_all(&held).unwrap();
        let stored = w.finish().unwrap();
        assert_eq!(stored.size, held.len() as u64, "the part is as long as it was");
        assert!(stored.stored < held.len() as u64 / 2, "the zeros take little of it: {} of {}", stored.stored, held.len());
        for name in &stored.files {
            assert!(fs::metadata(at.join(name)).unwrap().len() <= 8 << 10, "{name} is cut where it was asked to be");
        }
        let mut r = Reader::open(&at, &stored);
        let mut back = Vec::new();
        r.read_to_end(&mut back).unwrap();
        assert_eq!(back, held, "the part is read back whole across the files it is cut into");
        r.check(&stored).unwrap();
        fs::remove_dir_all(&at).unwrap();
    }

    #[test]
    fn the_zeros_are_written_out_when_no_holes_are_asked_for() {
        let at = scratch("dense");
        let mut held = vec![0x11; 1_000];
        held.extend(vec![0u8; 8 << 20]);
        held.extend(vec![0x22; 1_000]);
        let mut w = Writer::create(&at, "super", false, false, Ownership::Kept);
        w.write_all(&held).unwrap();
        let stored = w.finish().unwrap();
        assert_eq!((stored.size, stored.stored), (held.len() as u64, held.len() as u64), "every byte of the part takes room");
        let mut r = Reader::open(&at, &stored);
        let mut back = Vec::new();
        r.read_to_end(&mut back).unwrap();
        assert_eq!(back, held);
        r.check(&stored).unwrap();
        fs::remove_dir_all(&at).unwrap();
    }

    #[test]
    fn an_empty_part_still_has_a_file() {
        let at = scratch("empty");
        let stored = Writer::cut_at(&at, "fsc", true, true, Ownership::Kept, 1_000).finish().unwrap();
        assert_eq!(stored.files, ["fsc.000"]);
        let mut r = Reader::open(&at, &stored);
        assert_eq!(io::copy(&mut r, &mut io::sink()).unwrap(), 0);
        r.check(&stored).unwrap();
        fs::remove_dir_all(&at).unwrap();
    }
}
