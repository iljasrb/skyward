//! The flags in a vbmeta image's header that `avbctl` turns on and off, and
//! where the top-level vbmeta image is on a phone. Reading and writing them
//! here means an install script needs no `libbase.so` to disable
//! verification.

use anyhow::{bail, ensure, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// The fixed size of an `AvbVBMetaImageHeader`.
pub const HEADER_LEN: usize = 256;

const MAGIC: &[u8] = b"AVB0";
/// Where the flags sit among the header's bytes.
const FLAGS_AT: usize = 120;
/// `AVB_VBMETA_IMAGE_FLAGS_HASHTREE_DISABLED`.
const HASHTREE_DISABLED: u32 = 1 << 0;
/// `AVB_VBMETA_IMAGE_FLAGS_VERIFICATION_DISABLED`.
const VERIFICATION_DISABLED: u32 = 1 << 1;

/// The footer that locates a vbmeta image at the end of a boot partition.
const FOOTER_LEN: u64 = 64;
const FOOTER_MAGIC: &[u8] = b"AVBf";
/// Where the footer says its vbmeta image starts.
const FOOTER_VBMETA_AT: usize = 20;

/// A top-level vbmeta header, and the partition and offset it was read
/// from, so a changed header goes back where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub partition: PathBuf,
    pub at: u64,
    pub bytes: [u8; HEADER_LEN],
}

impl Header {
    /// Read the header at `at` in `partition`.
    pub fn open(partition: &Path, at: u64) -> Result<Header> {
        let mut file = File::open(partition).with_context(|| format!("open {}", partition.display()))?;
        file.seek(SeekFrom::Start(at)).with_context(|| format!("seek in {}", partition.display()))?;
        let mut bytes = [0u8; HEADER_LEN];
        file.read_exact(&mut bytes).with_context(|| format!("read the vbmeta header in {}", partition.display()))?;
        ensure!(&bytes[..4] == MAGIC, "{} does not hold a vbmeta header at {at}", partition.display());
        Ok(Header { partition: partition.to_path_buf(), at, bytes })
    }

    /// Write the header back where it was read from.
    pub fn write(&self) -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .open(&self.partition)
            .with_context(|| format!("open {} to write", self.partition.display()))?;
        crate::image::writable(&file).with_context(|| format!("let {} be written", self.partition.display()))?;
        file.seek(SeekFrom::Start(self.at)).with_context(|| format!("seek in {}", self.partition.display()))?;
        file.write_all(&self.bytes).with_context(|| format!("write the vbmeta header in {}", self.partition.display()))?;
        file.sync_all().with_context(|| format!("flush {}", self.partition.display()))?;
        drop(file);
        let back = Header::open(&self.partition, self.at)?;
        ensure!(back.bytes == self.bytes, "{} did not keep the vbmeta header", self.partition.display());
        Ok(())
    }

    fn flags(&self) -> u32 {
        u32::from_be_bytes(self.bytes[FLAGS_AT..FLAGS_AT + 4].try_into().expect("four bytes"))
    }

    fn set_flags(&mut self, flags: u32) {
        self.bytes[FLAGS_AT..FLAGS_AT + 4].copy_from_slice(&flags.to_be_bytes());
    }

    /// Whether the bootloader verifies the slot this header belongs to.
    pub fn verification_enabled(&self) -> bool {
        self.flags() & VERIFICATION_DISABLED == 0
    }

    pub fn set_verification(&mut self, enabled: bool) {
        let mut flags = self.flags();
        if enabled {
            flags &= !VERIFICATION_DISABLED;
        } else {
            flags |= VERIFICATION_DISABLED;
        }
        self.set_flags(flags);
    }

    /// Whether dm-verity is on for the slot this header belongs to.
    pub fn verity_enabled(&self) -> bool {
        self.flags() & HASHTREE_DISABLED == 0
    }

    pub fn set_verity(&mut self, enabled: bool) {
        let mut flags = self.flags();
        if enabled {
            flags &= !HASHTREE_DISABLED;
        } else {
            flags |= HASHTREE_DISABLED;
        }
        self.set_flags(flags);
    }
}

/// The top-level vbmeta header of the slot with `suffix`: the slot's own
/// `vbmeta` partition, the phone's single `vbmeta` partition, or the vbmeta
/// image a `boot` partition's footer locates.
pub fn find(by_name: &Path, suffix: &str) -> Result<Header> {
    for name in [format!("vbmeta{suffix}"), "vbmeta".to_string()] {
        let path = by_name.join(&name);
        if path.exists() {
            return Header::open(&path, 0);
        }
    }
    for name in [format!("boot{suffix}"), "boot".to_string()] {
        let path = by_name.join(&name);
        if path.exists() {
            return in_footer(&path);
        }
    }
    bail!("{} holds neither a vbmeta nor a boot partition", by_name.display())
}

/// The vbmeta header a boot partition's AVB footer locates.
fn in_footer(boot: &Path) -> Result<Header> {
    let mut file = File::open(boot).with_context(|| format!("open {}", boot.display()))?;
    let size = file.seek(SeekFrom::End(0)).with_context(|| format!("find the size of {}", boot.display()))?;
    ensure!(size >= FOOTER_LEN, "{} is too short for an AVB footer", boot.display());
    file.seek(SeekFrom::Start(size - FOOTER_LEN)).with_context(|| format!("seek in {}", boot.display()))?;
    let mut footer = [0u8; FOOTER_LEN as usize];
    file.read_exact(&mut footer).with_context(|| format!("read the AVB footer of {}", boot.display()))?;
    ensure!(&footer[..4] == FOOTER_MAGIC, "{} does not end in an AVB footer", boot.display());
    let at = u64::from_be_bytes(footer[FOOTER_VBMETA_AT..FOOTER_VBMETA_AT + 8].try_into().expect("eight bytes"));
    let end = at.checked_add(HEADER_LEN as u64).with_context(|| format!("{}'s footer points past its end", boot.display()))?;
    ensure!(end <= size, "{}'s footer points past its end", boot.display());
    Header::open(boot, at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn header(flags: u32) -> [u8; HEADER_LEN] {
        let mut bytes = [0u8; HEADER_LEN];
        bytes[..4].copy_from_slice(MAGIC);
        bytes[FLAGS_AT..FLAGS_AT + 4].copy_from_slice(&flags.to_be_bytes());
        bytes
    }

    #[test]
    fn verification_and_verity_are_separate_flags() {
        let mut h = Header { partition: PathBuf::from("vbmeta"), at: 0, bytes: header(0) };
        assert!(h.verification_enabled());
        assert!(h.verity_enabled());
        h.set_verification(false);
        assert!(!h.verification_enabled());
        assert!(h.verity_enabled());
        h.set_verity(false);
        assert!(!h.verification_enabled());
        assert!(!h.verity_enabled());
        h.set_verification(true);
        assert!(h.verification_enabled());
        assert!(!h.verity_enabled());
        h.set_verity(true);
        assert!(h.verification_enabled());
        assert!(h.verity_enabled());
    }

    #[test]
    fn a_vbmeta_partition_is_found_first() {
        let dir = tempfile::tempdir().unwrap();
        let vbmeta = dir.path().join("vbmeta_a");
        fs::write(&vbmeta, header(0)).unwrap();
        let found = find(dir.path(), "_a").unwrap();
        assert_eq!(found.partition, vbmeta);
        assert_eq!(found.at, 0);
    }

    #[test]
    fn a_boot_footer_locates_the_vbmeta_header() {
        let dir = tempfile::tempdir().unwrap();
        let boot = dir.path().join("boot_b");
        let mut bytes = vec![0u8; 8192];
        bytes[4096..4096 + HEADER_LEN].copy_from_slice(&header(0));
        let footer = bytes.len() - FOOTER_LEN as usize;
        bytes[footer..footer + 4].copy_from_slice(FOOTER_MAGIC);
        bytes[footer + 4..footer + 8].copy_from_slice(&1u32.to_be_bytes());
        bytes[footer + 8..footer + 12].copy_from_slice(&0u32.to_be_bytes());
        bytes[footer + 12..footer + 20].copy_from_slice(&4096u64.to_be_bytes());
        bytes[footer + 20..footer + 28].copy_from_slice(&4096u64.to_be_bytes());
        bytes[footer + 28..footer + 36].copy_from_slice(&256u64.to_be_bytes());
        fs::write(&boot, &bytes).unwrap();
        let found = find(dir.path(), "_b").unwrap();
        assert_eq!(found.partition, boot);
        assert_eq!(found.at, 4096);
    }

    #[test]
    fn a_changed_header_is_written_back() {
        let dir = tempfile::tempdir().unwrap();
        let vbmeta = dir.path().join("vbmeta");
        fs::write(&vbmeta, header(0)).unwrap();
        let mut found = Header::open(&vbmeta, 0).unwrap();
        found.set_verification(false);
        found.write().unwrap();
        let back = Header::open(&vbmeta, 0).unwrap();
        assert!(!back.verification_enabled());
        assert!(back.verity_enabled());
    }

    #[test]
    fn a_partition_that_is_not_vbmeta_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vbmeta");
        fs::write(&path, [0u8; HEADER_LEN]).unwrap();
        assert!(Header::open(&path, 0).is_err());
    }
}
