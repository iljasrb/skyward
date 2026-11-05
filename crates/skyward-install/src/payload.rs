//! The manifest inside an A/B update package's `payload.bin`: the partitions
//! it holds, how large each is, and the steps that produce it. Reading the
//! manifest is what lets a package's images be taken out without the
//! snapshots `update_engine` would keep.

use anyhow::{bail, ensure, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The four bytes every payload begins with.
pub const MAGIC: &[u8; 4] = b"CrAU";

/// The blocks a payload counts in, when its manifest names none.
pub const BLOCK: u64 = 4096;

/// The largest manifest read, and the largest image taken out. A boot image
/// is far smaller than either; the caps keep a damaged package from taking
/// the recovery's memory or storage.
const MAX_MANIFEST: u64 = 16 << 20;
const MAX_IMAGE: u64 = 512 << 20;
const MAX_PACKED: u64 = 512 << 20;

/// How much is read or written at a time.
const CHUNK: usize = 1 << 20;

/// The work was stopped as asked.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stopped")
    }
}

impl std::error::Error for Cancelled {}

/// A run of blocks within a partition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub start_block: u64,
    pub num_blocks: u64,
}

impl Extent {
    /// Where the run starts among the partition's bytes.
    pub fn at(&self, block: u64) -> u64 {
        self.start_block * block
    }

    /// How many bytes the run holds.
    pub fn len(&self, block: u64) -> u64 {
        self.num_blocks * block
    }
}

/// What the payload's own header says: the format it is in, how long its
/// manifest is, and how long the signature over that manifest is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: u64,
    pub manifest_size: u64,
    pub metadata_signature_size: u32,
}

impl Header {
    /// Where the manifest starts among the payload's bytes.
    pub fn manifest_at(&self) -> usize {
        match self.version {
            2.. => 4 + 8 + 8 + 4,
            _ => 4 + 8 + 8,
        }
    }
}

/// The header that opens `bytes`.
pub fn header(bytes: &[u8]) -> Result<Header> {
    ensure!(bytes.len() >= 20, "the payload's header is cut short");
    ensure!(bytes.starts_with(MAGIC), "the payload does not begin with CrAU");
    let version = u64::from_be_bytes(bytes[4..12].try_into().expect("eight bytes"));
    let manifest_size = u64::from_be_bytes(bytes[12..20].try_into().expect("eight bytes"));
    let signature = match version {
        2.. => {
            ensure!(bytes.len() >= 24, "the payload's header is cut short");
            u32::from_be_bytes(bytes[20..24].try_into().expect("four bytes"))
        }
        _ => 0,
    };
    ensure!(manifest_size <= MAX_MANIFEST, "the manifest is {manifest_size} bytes, larger than this recovery reads");
    Ok(Header { version, manifest_size, metadata_signature_size: signature })
}

/// How one of a partition's steps produces blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Replace,
    ReplaceBz,
    ReplaceXz,
    Zero,
    SourceCopy,
    SourceBsdiff,
    BrotliBsdiff,
    Puffdiff,
    /// A step this code does not carry out.
    Unknown(i32),
}

impl Kind {
    fn of(value: i32) -> Kind {
        // The numbers update_metadata.proto gives each step.
        match value {
            0 => Kind::Replace,
            1 => Kind::ReplaceBz,
            4 => Kind::SourceCopy,
            5 => Kind::SourceBsdiff,
            6 => Kind::Zero,
            8 => Kind::ReplaceXz,
            9 => Kind::Puffdiff,
            10 => Kind::BrotliBsdiff,
            // 11 and up are the delta steps newer packages use: zucchini,
            // lz4diff and their kin. None is a whole-image replace.
            other => Kind::Unknown(other),
        }
    }

    /// Whether the step reads the payload's data at an offset, rather than
    /// making the blocks from what it finds.
    pub fn reads_data(&self) -> bool {
        matches!(self, Kind::Replace | Kind::ReplaceBz | Kind::ReplaceXz)
    }

    /// Whether this code carries the step out.
    pub fn carried(&self) -> bool {
        matches!(self, Kind::Replace | Kind::Zero | Kind::ReplaceBz | Kind::ReplaceXz)
    }

    /// What a manifest calls the step.
    pub fn name(&self) -> String {
        match self {
            Kind::Replace => "REPLACE",
            Kind::ReplaceBz => "REPLACE_BZ",
            Kind::ReplaceXz => "REPLACE_XZ",
            Kind::Zero => "ZERO",
            Kind::SourceCopy => "SOURCE_COPY",
            Kind::SourceBsdiff => "SOURCE_BSDIFF",
            Kind::BrotliBsdiff => "BROTLI_BSDIFF",
            Kind::Puffdiff => "PUFFDIFF",
            Kind::Unknown(value) => return format!("type {value}"),
        }
        .to_string()
    }
}

/// One step of a partition, as the manifest lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Operation {
    pub kind: Kind,
    /// Where its data sits in the payload, for the steps that read any.
    pub data_offset: u64,
    pub data_length: u64,
    /// The runs of the partition it fills.
    pub dst: Vec<Extent>,
    /// The SHA-256 the package gives the operation's data, where it does.
    pub data_sha256: Option<Vec<u8>>,
}

impl Operation {
    /// How many bytes the step writes.
    pub fn len(&self, block: u64) -> u64 {
        self.dst.iter().map(|extent| extent.len(block)).sum()
    }
}

/// A partition the payload writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partition {
    pub name: String,
    /// How large the partition is once written, in bytes.
    pub size: u64,
    pub operations: Vec<Operation>,
    /// The SHA-256 the package gives the whole partition, where it does.
    pub hash: Option<Vec<u8>>,
}

impl Partition {
    /// Whether every step of the partition is one this code carries out.
    pub fn carried(&self) -> bool {
        self.operations.iter().all(|operation| operation.kind.carried())
    }

    /// The steps this code does not carry out, each said once.
    pub fn uncarried(&self) -> Vec<String> {
        let mut said: Vec<String> = Vec::new();
        for operation in &self.operations {
            let name = operation.kind.name();
            if !operation.kind.carried() && !said.contains(&name) {
                said.push(name);
            }
        }
        said
    }
}

/// What the payload says to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub block_size: u32,
    pub partitions: Vec<Partition>,
    /// Where the steps' data begins among the package's bytes: the payload's
    /// header, manifest and the signature over it come first.
    pub data_at: u64,
}

impl Manifest {
    /// The block size the steps count in.
    pub fn block(&self) -> u64 {
        match self.block_size {
            0 => BLOCK,
            size => u64::from(size),
        }
    }
}

/// The manifest of the payload that sits at `at` among `source`'s bytes and
/// runs for `size` of them, as a package's `payload.bin` does.
pub fn read(source: &mut (impl Read + Seek), at: u64, size: u64) -> Result<Manifest> {
    ensure!(size >= 20, "the payload is {size} bytes, too short for its header");
    source.seek(SeekFrom::Start(at))?;
    let mut opening = [0u8; 24];
    source.read_exact(&mut opening[..20])?;
    let version = u64::from_be_bytes(opening[4..12].try_into().expect("eight bytes"));
    let held = match version {
        2.. => 24,
        _ => 20,
    };
    if held == 24 {
        source.read_exact(&mut opening[20..24])?;
    }
    let head = header(&opening[..held])?;
    ensure!(head.manifest_size <= size, "the manifest is longer than the payload");
    let manifest_at = at.checked_add(head.manifest_at() as u64).ok_or_else(|| anyhow::anyhow!("the payload's manifest starts past its end"))?;
    let manifest_end = manifest_at.checked_add(head.manifest_size).ok_or_else(|| anyhow::anyhow!("the payload's manifest runs past its end"))?;
    ensure!(manifest_end <= at.saturating_add(size), "the manifest runs past the payload's end");
    let mut manifest_bytes = vec![0u8; head.manifest_size as usize];
    source.seek(SeekFrom::Start(manifest_at))?;
    source.read_exact(&mut manifest_bytes)?;
    let mut read = manifest(&manifest_bytes)?;
    read.data_at = manifest_end
        .checked_add(u64::from(head.metadata_signature_size))
        .ok_or_else(|| anyhow::anyhow!("the payload's data starts past its end"))?;
    ensure!(read.data_at <= at.saturating_add(size), "the payload's data starts past its end");
    Ok(read)
}

/// What a step's data comes to once it is unpacked, `want` bytes of it.
fn unpack(kind: Kind, data: &[u8], want: usize) -> Result<Vec<u8>> {
    ensure!(want as u64 <= MAX_IMAGE, "a step unpacks to {want} bytes, larger than this recovery takes out");
    let mut out = Vec::with_capacity(want.min(CHUNK));
    match kind {
        Kind::Replace => out.extend_from_slice(data),
        Kind::ReplaceBz => {
            let mut held = bzip2_rs::DecoderReader::new(data);
            held.read_to_end(&mut out)?;
        }
        Kind::ReplaceXz => {
            lzma_rs::xz_decompress(&mut std::io::Cursor::new(data), &mut out)?;
        }
        other => bail!("a step of {} is not carried out here", other.name()),
    }
    ensure!(out.len() == want, "a step of {} unpacks to {} bytes, not {want}", kind.name(), out.len());
    Ok(out)
}

/// Whether the data a step read is the data the package says it is.
fn check_hash(operation: &Operation, got: &[u8]) -> Result<()> {
    if let Some(want) = &operation.data_sha256 {
        ensure!(want.len() == 32, "a step's hash is {} bytes, not 32", want.len());
        ensure!(want.as_slice() == got, "a step of {} does not match the hash the package gives it", operation.kind.name());
    }
    Ok(())
}

/// The extents a step writes, checked against the partition's size.
fn check_extents(partition: &Partition, operation: &Operation, block: u64) -> Result<()> {
    let blocks = partition.size.div_ceil(block);
    for run in &operation.dst {
        let end = run.start_block.checked_add(run.num_blocks).ok_or_else(|| anyhow::anyhow!("{} has a run that runs past its end", partition.name))?;
        ensure!(end <= blocks, "{} writes past its end", partition.name);
    }
    Ok(())
}

/// Writes what `partition` holds into `out`, reading the steps' data from
/// `source` — the package, whose `payload.bin` starts at `at` — a chunk at a
/// time, so a package of ten gigabytes never sits in memory. `progress` is
/// told the bytes written and how many there are in all; `stop` is asked
/// between chunks whether to give up, which it does with [`Cancelled`].
pub fn apply_while(
    source: &mut (impl Read + Seek),
    at: u64,
    block: u64,
    partition: &Partition,
    out: &mut (impl Write + Seek),
    progress: &mut dyn FnMut(u64, u64),
    stop: &mut dyn FnMut() -> bool,
) -> Result<()> {
    ensure!(block > 0, "the payload counts in blocks of zero");
    ensure!(partition.size > 0, "{} has no size in the package", partition.name);
    ensure!(partition.size <= MAX_IMAGE, "{} is {} bytes, larger than this recovery takes out", partition.name, partition.size);
    ensure!(
        partition.carried(),
        "{} is written by steps this code does not carry out ({})",
        partition.name,
        partition.uncarried().join(" ")
    );
    let all = partition.size;
    let mut written = 0u64;
    for operation in &partition.operations {
        let wants = operation.len(block);
        ensure!(wants <= MAX_IMAGE, "{} writes {wants} bytes in one step, larger than this recovery takes out", partition.name);
        check_extents(partition, operation, block)?;
        match operation.kind {
            Kind::Replace => {
                ensure!(
                    wants == operation.data_length,
                    "{} reads {} bytes and writes {wants}",
                    partition.name,
                    operation.data_length
                );
                source.seek(SeekFrom::Start(
                    at.checked_add(operation.data_offset).ok_or_else(|| anyhow::anyhow!("{}'s data starts past the payload's end", partition.name))?,
                ))?;
                let mut left = operation.data_length;
                let mut hasher = Sha256::new();
                for run in &operation.dst {
                    out.seek(SeekFrom::Start(run.at(block)))?;
                    let mut len = run.len(block);
                    while len > 0 {
                        if stop() {
                            bail!(Cancelled);
                        }
                        let step = len.min(CHUNK as u64);
                        let mut held = vec![0u8; step as usize];
                        source.read_exact(&mut held)?;
                        hasher.update(&held);
                        out.write_all(&held)?;
                        len -= step;
                        left -= step;
                        written += step;
                        progress(written, all);
                    }
                }
                ensure!(left == 0, "{} reads more than its runs hold", partition.name);
                check_hash(operation, &hasher.finalize())?;
            }
            Kind::Zero => {
                let zero = vec![0u8; CHUNK];
                for run in &operation.dst {
                    out.seek(SeekFrom::Start(run.at(block)))?;
                    let mut len = run.len(block);
                    while len > 0 {
                        if stop() {
                            bail!(Cancelled);
                        }
                        let step = len.min(CHUNK as u64);
                        out.write_all(&zero[..step as usize])?;
                        len -= step;
                        written += step;
                        progress(written, all);
                    }
                }
            }
            // A run packed by one of the ways the manifest names is
            // unpacked whole, then written across the runs it fills.
            Kind::ReplaceBz | Kind::ReplaceXz => {
                ensure!(operation.data_length <= MAX_PACKED, "{}'s data is larger than this recovery reads", partition.name);
                source.seek(SeekFrom::Start(
                    at.checked_add(operation.data_offset).ok_or_else(|| anyhow::anyhow!("{}'s data starts past the payload's end", partition.name))?,
                ))?;
                let mut packed = vec![0u8; operation.data_length as usize];
                source.read_exact(&mut packed)?;
                check_hash(operation, &Sha256::digest(&packed))?;
                let data = unpack(operation.kind, &packed, wants as usize)?;
                let mut from = 0usize;
                for run in &operation.dst {
                    let len = run.len(block) as usize;
                    ensure!(from + len <= data.len(), "{} writes more than its step unpacks to", partition.name);
                    out.seek(SeekFrom::Start(run.at(block)))?;
                    out.write_all(&data[from..from + len])?;
                    from += len;
                    written += len as u64;
                    progress(written, all);
                }
                ensure!(from == data.len(), "{} writes less than its step unpacks to", partition.name);
            }
            _ => bail!("{} is written by steps this code does not carry out ({})", partition.name, partition.uncarried().join(" ")),
        }
    }
    Ok(())
}

/// Writes the image `partition` holds to the file at `out`, reading it from
/// `source` where the payload sits at `at`. The partition's own size is what
/// the file comes to, and a file that did not come out whole is taken away.
pub fn extract_while(
    source: &mut (impl Read + Seek),
    at: u64,
    block: u64,
    partition: &Partition,
    out: &Path,
    progress: &mut dyn FnMut(u64, u64),
    stop: &mut dyn FnMut() -> bool,
) -> Result<()> {
    ensure!(partition.size > 0, "{} has no size in the package", partition.name);
    ensure!(partition.size <= MAX_IMAGE, "{} is {} bytes, larger than this recovery takes out", partition.name, partition.size);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = tmp_path(out);
    let mut file = std::fs::File::create(&tmp).map_err(|e| anyhow::anyhow!("create {}: {e}", tmp.display()))?;
    let written = apply_while(source, at, block, partition, &mut file, progress, stop).and_then(|()| {
        file.set_len(partition.size)?;
        file.sync_all()?;
        Ok(())
    });
    drop(file);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Some(want) = &partition.hash {
        ensure!(want.len() == 32, "{}'s hash is {} bytes, not 32", partition.name, want.len());
        let got = sha256_file(&tmp)?;
        if want.as_slice() != got.as_slice() {
            let _ = std::fs::remove_file(&tmp);
            bail!("{} does not match the hash the package gives it", partition.name);
        }
    }
    std::fs::rename(&tmp, out).map_err(|e| anyhow::anyhow!("rename {} to {}: {e}", tmp.display(), out.display()))?;
    Ok(())
}

/// The same, without a way to stop it.
pub fn extract(
    source: &mut (impl Read + Seek),
    at: u64,
    block: u64,
    partition: &Partition,
    out: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    extract_while(source, at, block, partition, out, progress, &mut || false)
}

fn tmp_path(out: &Path) -> PathBuf {
    let mut name = out.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    out.with_file_name(name)
}

fn sha256_file(path: &Path) -> Result<Vec<u8>> {
    let mut file = std::fs::File::open(path).map_err(|e| anyhow::anyhow!("open {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_vec())
}

/// The manifest `manifest` holds.
pub fn manifest(manifest: &[u8]) -> Result<Manifest> {
    let mut block_size = 0;
    let mut partitions = Vec::new();
    for (field, wire, value) in fields(manifest)? {
        match (field, wire) {
            (3, 0) => block_size = u32::try_from(number(value)?).unwrap_or(0),
            (13, 2) => partitions.push(partition(value)?),
            _ => {}
        }
    }
    Ok(Manifest { block_size, partitions, data_at: 0 })
}

fn partition(bytes: &[u8]) -> Result<Partition> {
    let mut name = String::new();
    let mut size = 0;
    let mut operations = Vec::new();
    let mut hash = None;
    for (field, wire, value) in fields(bytes)? {
        match (field, wire) {
            (1, 2) => name = String::from_utf8_lossy(value).into_owned(),
            // The partition as it is written, a message of its own: its
            // size and the hash it is checked against.
            (7, 2) => {
                for (inner, inner_wire, held) in fields(value)? {
                    match (inner, inner_wire) {
                        (1, 0) => size = number(held)?,
                        (2, 2) => hash = Some(held.to_vec()),
                        _ => {}
                    }
                }
            }
            (8, 2) => operations.push(operation(value)?),
            _ => {}
        }
    }
    ensure!(!name.is_empty(), "a partition of the manifest has no name");
    Ok(Partition { name, size, operations, hash })
}

fn operation(bytes: &[u8]) -> Result<Operation> {
    let mut kind = Kind::Replace;
    let mut data_offset = 0;
    let mut data_length = 0;
    let mut dst = Vec::new();
    let mut data_sha256 = None;
    for (field, wire, value) in fields(bytes)? {
        match (field, wire) {
            (1, 0) => kind = Kind::of(i32::try_from(number(value)?).unwrap_or(-1)),
            (2, 0) => data_offset = number(value)?,
            (3, 0) => data_length = number(value)?,
            (6, 2) => dst.push(extent(value)?),
            (8, 2) => data_sha256 = Some(value.to_vec()),
            _ => {}
        }
    }
    Ok(Operation { kind, data_offset, data_length, dst, data_sha256 })
}

fn extent(bytes: &[u8]) -> Result<Extent> {
    let mut start_block = 0;
    let mut num_blocks = 0;
    for (field, wire, value) in fields(bytes)? {
        match (field, wire) {
            (1, 0) => start_block = number(value)?,
            (2, 0) => num_blocks = number(value)?,
            _ => {}
        }
    }
    Ok(Extent { start_block, num_blocks })
}

/// The fields of a message, as the numbers they carry and the bytes of each.
fn fields(bytes: &[u8]) -> Result<Vec<(u64, u64, &[u8])>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let key = varint(bytes, &mut at)?;
        let (field, wire) = (key >> 3, key & 7);
        match wire {
            // A number, kept as its own bytes for the field to read.
            0 => {
                let start = at;
                varint(bytes, &mut at)?;
                out.push((field, wire, &bytes[start..at]));
            }
            2 => {
                let len = usize::try_from(varint(bytes, &mut at)?).map_err(|_| anyhow::anyhow!("a field of the manifest is too long"))?;
                let end = at.checked_add(len).filter(|end| *end <= bytes.len());
                let end = end.ok_or_else(|| anyhow::anyhow!("a field of the manifest runs past its end"))?;
                out.push((field, wire, &bytes[at..end]));
                at = end;
            }
            // Fixed-width numbers are not among the fields read here; skip
            // them so a newer manifest still parses.
            1 => {
                ensure!(at + 8 <= bytes.len(), "a field of the manifest runs past its end");
                at += 8;
            }
            5 => {
                ensure!(at + 4 <= bytes.len(), "a field of the manifest runs past its end");
                at += 4;
            }
            other => bail!("a field of the manifest has wire type {other}"),
        }
    }
    Ok(out)
}

/// A base-128 number, as protobuf writes them.
fn varint(bytes: &[u8], at: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in 0..10 {
        let byte = *bytes.get(*at).ok_or_else(|| anyhow::anyhow!("a number of the manifest runs past its end"))?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << (shift * 7);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("a number of the manifest is too long")
}

fn number(bytes: &[u8]) -> Result<u64> {
    let mut at = 0;
    let value = varint(bytes, &mut at)?;
    ensure!(at == bytes.len(), "a number of the manifest has bytes after it");
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn varint_bytes(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
        out
    }

    fn key(field: u64, wire: u64) -> Vec<u8> {
        varint_bytes((field << 3) | wire)
    }

    fn len_field(field: u64, value: &[u8]) -> Vec<u8> {
        let mut out = key(field, 2);
        out.extend(varint_bytes(value.len() as u64));
        out.extend_from_slice(value);
        out
    }

    fn num_field(field: u64, value: u64) -> Vec<u8> {
        let mut out = key(field, 0);
        out.extend(varint_bytes(value));
        out
    }

    fn extent(start: u64, blocks: u64) -> Vec<u8> {
        let mut out = num_field(1, start);
        out.extend(num_field(2, blocks));
        out
    }

    fn operation(kind: u64, offset: u64, length: u64, dst: &[u8]) -> Vec<u8> {
        let mut out = num_field(1, kind);
        out.extend(num_field(2, offset));
        out.extend(num_field(3, length));
        out.extend(len_field(6, dst));
        out
    }

    fn partition(name: &str, size: u64, operations: &[Vec<u8>]) -> Vec<u8> {
        let info = num_field(1, size);
        let mut out = len_field(1, name.as_bytes());
        out.extend(len_field(7, &info));
        for op in operations {
            out.extend(len_field(8, op));
        }
        out
    }

    fn partition_with_hash(name: &str, size: u64, hash: &[u8], operations: &[Vec<u8>]) -> Vec<u8> {
        let mut info = num_field(1, size);
        info.extend(len_field(2, hash));
        let mut out = len_field(1, name.as_bytes());
        out.extend(len_field(7, &info));
        for op in operations {
            out.extend(len_field(8, op));
        }
        out
    }

    fn manifest(partitions: &[Vec<u8>]) -> Vec<u8> {
        let mut out = num_field(3, BLOCK);
        for p in partitions {
            out.extend(len_field(13, p));
        }
        out
    }

    fn payload(manifest: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&2u64.to_be_bytes());
        out.extend_from_slice(&(manifest.len() as u64).to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(manifest);
        out.extend_from_slice(data);
        out
    }

    fn one_partition(kind: u64, size: u64, data: &[u8]) -> Vec<u8> {
        let op = operation(kind, 0, data.len() as u64, &extent(0, 1));
        let p = partition("boot", size, &[op]);
        payload(&manifest(&[p]), data)
    }

    #[test]
    fn a_stored_image_is_taken_out_whole() {
        let mut data = vec![0u8; BLOCK as usize];
        data[..5].copy_from_slice(b"hello");
        let bytes = one_partition(0, BLOCK, &data);
        let mut source = Cursor::new(bytes);
        let size = source.get_ref().len() as u64;
        let read = read(&mut source, 0, size).unwrap();
        let p = &read.partitions[0];
        assert_eq!(p.name, "boot");
        assert_eq!(p.size, BLOCK);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("boot.img");
        extract(&mut source, read.data_at, read.block(), p, &out, &mut |_, _| {}).unwrap();
        let image = std::fs::read(&out).unwrap();
        assert_eq!(image.len(), BLOCK as usize);
        assert_eq!(&image[..5], b"hello");
        assert!(image[5..].iter().all(|b| *b == 0));
    }

    #[test]
    fn an_xz_image_is_unpacked() {
        let mut raw = vec![0u8; BLOCK as usize];
        raw[..5].copy_from_slice(b"hello");
        let mut packed = Vec::new();
        lzma_rs::xz_compress(&mut std::io::Cursor::new(&raw), &mut packed).unwrap();
        let bytes = one_partition(8, BLOCK, &packed);
        let mut source = Cursor::new(bytes);
        let size = source.get_ref().len() as u64;
        let read = read(&mut source, 0, size).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("boot.img");
        extract(&mut source, read.data_at, read.block(), &read.partitions[0], &out, &mut |_, _| {}).unwrap();
        let image = std::fs::read(&out).unwrap();
        assert_eq!(image, raw);
    }

    #[test]
    fn a_zeroed_partition_is_all_zeros() {
        let op = operation(6, 0, 0, &extent(0, 1));
        let p = partition("boot", BLOCK, &[op]);
        let bytes = payload(&manifest(&[p]), &[]);
        let mut source = Cursor::new(bytes);
        let size = source.get_ref().len() as u64;
        let read = read(&mut source, 0, size).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("boot.img");
        extract(&mut source, read.data_at, read.block(), &read.partitions[0], &out, &mut |_, _| {}).unwrap();
        let image = std::fs::read(&out).unwrap();
        assert_eq!(image.len(), BLOCK as usize);
        assert!(image.iter().all(|b| *b == 0));
    }

    #[test]
    fn the_newer_delta_steps_are_not_taken_for_replacements() {
        assert_eq!(Kind::of(11), Kind::Unknown(11));
        assert_eq!(Kind::of(12), Kind::Unknown(12));
        assert!(!Kind::of(11).carried());
        assert!(!Kind::of(12).carried());
    }

    #[test]
    fn a_cut_short_header_is_refused() {
        assert!(header(b"CrAU").is_err());
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&2u64.to_be_bytes());
        assert!(header(&bytes).is_err());
    }

    #[test]
    fn a_manifest_larger_than_the_payload_is_refused() {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&2u64.to_be_bytes());
        bytes.extend_from_slice(&u64::MAX.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes());
        let mut source = Cursor::new(bytes);
        assert!(read(&mut source, 0, 24).is_err());
    }

    #[test]
    fn a_step_that_does_not_match_its_hash_is_refused_and_leaves_no_file() {
        let op = operation(0, 0, 5, &extent(0, 1));
        let mut op_with_hash = op.clone();
        op_with_hash.extend(len_field(8, &[0u8; 32]));
        let p = partition_with_hash("boot", BLOCK, &[0u8; 32], &[op_with_hash]);
        let bytes = payload(&manifest(&[p]), b"hello");
        let mut source = Cursor::new(bytes);
        let size = source.get_ref().len() as u64;
        let read = read(&mut source, 0, size).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("boot.img");
        assert!(extract(&mut source, read.data_at, read.block(), &read.partitions[0], &out, &mut |_, _| {}).is_err());
        assert!(!out.exists());
        assert!(!dir.path().join("boot.img.part").exists());
    }

    #[test]
    fn a_step_this_code_does_not_carry_out_is_refused() {
        let op = operation(4, 0, 0, &extent(0, 1));
        let p = partition("boot", BLOCK, &[op]);
        let bytes = payload(&manifest(&[p]), &[]);
        let mut source = Cursor::new(bytes);
        let size = source.get_ref().len() as u64;
        let read = read(&mut source, 0, size).unwrap();
        assert!(!read.partitions[0].carried());
        assert_eq!(read.partitions[0].uncarried(), ["SOURCE_COPY"]);
    }

    #[test]
    fn stopping_takes_the_file_away() {
        let data = vec![0u8; BLOCK as usize];
        let bytes = one_partition(0, BLOCK, &data);
        let mut source = Cursor::new(bytes);
        let size = source.get_ref().len() as u64;
        let read = read(&mut source, 0, size).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("boot.img");
        let mut stop = || true;
        let ended = extract_while(&mut source, read.data_at, read.block(), &read.partitions[0], &out, &mut |_, _| {}, &mut stop);
        assert!(ended.unwrap_err().is::<Cancelled>());
        assert!(!out.exists());
        assert!(!dir.path().join("boot.img.part").exists());
    }
}
