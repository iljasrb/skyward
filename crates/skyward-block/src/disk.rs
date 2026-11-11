//! The kernel's disks: what it lists under `/sys/block`, the partitions each
//! holds, and the mount that shows one of them.

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

/// Where the kernel lists its disks, each a link to the device it is.
pub const SYS_BLOCK: &str = "/sys/block";

/// Where ueventd makes the nodes of disks and partitions.
pub const DEV_BLOCK: &str = "/dev/block";

/// The filesystems a medium may hold, in the order they are tried, with what
/// each is mounted with to give names as the medium keeps them: FAT and exFAT
/// keep long names in UTF-16, which the kernel otherwise turns into a code
/// page without Chinese.
pub const FILESYSTEMS: [(&str, &str); 5] =
    [("vfat", "utf8"), ("exfat", "iocharset=utf8"), ("ntfs3", "iocharset=utf8"), ("ext4", ""), ("f2fs", "")];

/// A disk as the kernel lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    /// The disk's name, such as `sdg`.
    pub disk: String,
    /// Its partitions by name, each with its size in bytes.
    pub parts: Vec<(String, u64)>,
}

impl Disk {
    /// Whether ueventd has made the nodes of the disk and its partitions,
    /// which come a moment after the kernel lists them.
    pub fn ready(&self) -> bool {
        std::iter::once(&self.disk).chain(self.parts.iter().map(|(name, _)| name)).all(|name| Path::new(DEV_BLOCK).join(name).exists())
    }

    /// What to show of the disk, in the order it is tried: its partitions,
    /// the largest first, or the whole disk for a medium formatted without
    /// a partition table.
    fn candidates(&self) -> Vec<&str> {
        if self.parts.is_empty() {
            return vec![&self.disk];
        }
        let mut parts: Vec<&(String, u64)> = self.parts.iter().collect();
        parts.sort_by_key(|&(_, size)| std::cmp::Reverse(*size));
        parts.into_iter().map(|(name, _)| name.as_str()).collect()
    }
}

/// Whether `word` is a whole part of the path `path` names, as the
/// controllers and buses of a disk's device are.
pub fn part_of(path: &Path, word: &str) -> bool {
    path.components().any(|part| part.as_os_str().to_str() == Some(word))
}

/// The disks among those the kernel lists in `listed` that `keep` takes and
/// that hold anything, by the names of their disks. A medium pulled out while
/// it is looked at is left out.
pub fn disks_in(listed: &Path, keep: impl Fn(&Path) -> bool) -> Result<Vec<Disk>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(listed).with_context(|| format!("read {}", listed.display()))? {
        let link = entry.with_context(|| format!("read {}", listed.display()))?.path();
        let Some(device) = present(fs::canonicalize(&link)).with_context(|| format!("follow {}", link.display()))? else { continue };
        if !keep(&device) {
            continue;
        }
        // A reader with no medium in it is a disk of no size.
        let Some(size) = sectors(&device)? else { continue };
        if size == 0 {
            continue;
        }
        let Some(inside) = present(fs::read_dir(&device)).with_context(|| format!("read {}", device.display()))? else { continue };
        let mut parts = Vec::new();
        for entry in inside {
            let dir = entry.with_context(|| format!("read {}", device.display()))?.path();
            if !dir.join("partition").is_file() {
                continue;
            }
            let Some(size) = sectors(&dir)? else { continue };
            parts.push((name(&dir)?, size * 512));
        }
        parts.sort();
        found.push(Disk { disk: name(&link)?, parts });
    }
    found.sort_by(|a, b| a.disk.cmp(&b.disk));
    Ok(found)
}

/// The size the kernel gives a disk or partition at `dir`, in 512-byte
/// sectors; none once it is gone.
fn sectors(dir: &Path) -> Result<Option<u64>> {
    let path = dir.join("size");
    let Some(text) = present(fs::read_to_string(&path)).with_context(|| format!("read {}", path.display()))? else { return Ok(None) };
    let sectors = text.trim().parse().with_context(|| format!("{} holds {text:?}", path.display()))?;
    Ok(Some(sectors))
}

fn name(path: &Path) -> Result<String> {
    Ok(path.file_name().with_context(|| format!("{} has no name", path.display()))?.to_string_lossy().into_owned())
}

/// What `r` holds, or nothing for what was gone when it was looked at.
fn present<T>(r: std::io::Result<T>) -> std::io::Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// How a medium's files are shown: read only, as a drive is that is looked at
/// to be read from, or read and written, as the card files are saved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    ReadWrite,
}

impl Access {
    /// The words the kernel takes for this access, before the filesystem's
    /// own: nothing on either medium runs, and neither is a device.
    fn words(self) -> &'static [&'static str] {
        match self {
            Access::ReadOnly => &["ro", "nosuid", "nodev", "noexec"],
            Access::ReadWrite => &["rw", "nosuid", "nodev"],
        }
    }
}

/// Show `disk` at `at` as `access` says: the first of its partitions, or its
/// disk, that holds a filesystem the kernel reads. Gives the partition and
/// the filesystem shown.
pub fn mount(disk: &Disk, at: &Path, access: Access) -> Result<(String, &'static str)> {
    fs::create_dir_all(at).with_context(|| format!("create {}", at.display()))?;
    let mut tried = Vec::new();
    for name in disk.candidates() {
        let device = Path::new(DEV_BLOCK).join(name);
        for (filesystem, own) in FILESYSTEMS {
            let mut options: Vec<String> = access.words().iter().map(|word| word.to_string()).collect();
            options.extend([own.to_string()].into_iter().filter(|own| !own.is_empty()));
            let options = &options;
            match crate::mount::mount(&device, at, filesystem, options) {
                Ok(()) => return Ok((name.to_string(), filesystem)),
                Err(e) => tried.push(format!("{name} as {filesystem}: {}", e.root_cause())),
            }
        }
    }
    bail!("{} holds no filesystem the kernel reads: {}", disk.disk, tried.join("; "))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::os::unix::fs::symlink;

    /// A disk at `path` under `root`'s devices, with `sectors` and these
    /// partitions, each with its sectors, listed in `root`'s block folder
    /// as the kernel lists its disks.
    pub fn disk(root: &Path, path: &str, sectors: u64, parts: &[(&str, u64)]) {
        let dir = root.join("devices").join(path);
        fs::create_dir_all(dir.join("queue")).unwrap();
        fs::write(dir.join("size"), format!("{sectors}\n")).unwrap();
        for (k, (part, sectors)) in parts.iter().enumerate() {
            fs::create_dir_all(dir.join(part)).unwrap();
            fs::write(dir.join(part).join("partition"), format!("{}\n", k + 1)).unwrap();
            fs::write(dir.join(part).join("size"), format!("{sectors}\n")).unwrap();
        }
        let name = dir.file_name().unwrap().to_owned();
        fs::create_dir_all(root.join("block")).unwrap();
        symlink(&dir, root.join("block").join(name)).unwrap();
    }

    /// A root of a test's own, named for the test that makes it.
    pub fn root(what: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("skyward-{what}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn the_largest_partition_is_tried_first_and_a_disk_without_any_whole() {
        let mac = Disk { disk: "sdg".into(), parts: vec![("sdg1".into(), 209_715_200), ("sdg2".into(), 63_000_000_000)] };
        assert_eq!(mac.candidates(), ["sdg2", "sdg1"], "the data before the EFI partition in front of it");
        let floppy = Disk { disk: "sdh".into(), parts: Vec::new() };
        assert_eq!(floppy.candidates(), ["sdh"]);
    }

    #[test]
    fn a_word_is_a_part_of_a_path_only_as_a_whole_part() {
        let path = Path::new("/sys/devices/platform/soc/8804000.sdhci/mmc_host/mmc0/mmc0:0001/block/mmcblk0");
        assert!(part_of(path, "8804000.sdhci"));
        assert!(!part_of(path, "8804000"), "a name is taken whole, not by its start");
    }
}
