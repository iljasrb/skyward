//! The cloud storage as a filesystem: the FUSE requests the kernel sends,
//! answered from a WebDAV server, and the mount that puts it at
//! `/mnt/cloud`.
//!
//! A file opened to read is fetched whole into `/tmp` and served from
//! there; a file opened to write is made there and uploaded when it is
//! flushed or closed. The folder's own entries are read with PROPFIND and
//! kept for a moment, so a listing does not ask the server for every row.

use crate::webdav::{now, Backend, Entry, Error, ErrorKind, Kind, Stat};
use anyhow::{anyhow, Context, Result};
use polyfuse_kernel::{
    fuse_attr, fuse_attr_out, fuse_create_in, fuse_dirent, fuse_entry_out, fuse_fsync_in, fuse_getxattr_in,
    fuse_getxattr_out, fuse_in_header, fuse_init_in, fuse_init_out, fuse_kstatfs, fuse_mkdir_in, fuse_open_in, fuse_open_out,
    fuse_opcode, fuse_out_header, fuse_read_in, fuse_release_in, fuse_rename_in, fuse_setattr_in, fuse_statfs_out, fuse_write_in,
    fuse_write_out, FATTR_ATIME, FATTR_ATIME_NOW, FATTR_FH, FATTR_GID, FATTR_MODE, FATTR_MTIME, FATTR_MTIME_NOW, FATTR_SIZE,
    FATTR_UID, FUSE_ASYNC_READ, FUSE_BIG_WRITES, FUSE_COMPAT_22_INIT_OUT_SIZE, FUSE_KERNEL_MINOR_VERSION, FUSE_KERNEL_VERSION,
};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::mem::size_of;
use std::os::unix::fs::{FileExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use zerocopy::{FromBytes, IntoBytes};

/// Where the cloud storage shows.
pub const MOUNT_POINT: &str = "/mnt/cloud";

/// Where files on their way to or from the server wait.
pub const CACHE: &str = "/tmp/skyward-cloud";

const ROOT: u64 = 1;
const TTL: u64 = 5;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const DT_DIR: u32 = 4;
const DT_REG: u32 = 8;
#[cfg(any(target_os = "android", test))]
const REQUEST_BUFFER: usize = 256 * 1024;
const MAX_WRITE: u32 = 128 * 1024;
const DIR_TTL: Duration = Duration::from_secs(5);

/// What a request comes to.
#[derive(Debug, PartialEq, Eq)]
pub enum Handled {
    Reply(Vec<u8>),
    Silent,
    /// Write the reply and stop serving.
    Exit(Vec<u8>),
}

#[derive(Clone)]
struct Node {
    parent: u64,
    name: String,
    path: String,
    stat: Stat,
    /// When the server last said what the entry is.
    at: Instant,
}

struct Handle {
    node: u64,
    file: File,
    temp: PathBuf,
    dirty: bool,
    size: u64,
}

/// The filesystem's state: the entries the kernel knows and the files open
/// to it.
pub struct Server {
    backend: Arc<dyn Backend>,
    nodes: HashMap<u64, Node>,
    by_path: HashMap<String, u64>,
    next_node: u64,
    handles: HashMap<u64, Handle>,
    next_handle: u64,
    temp: PathBuf,
    dirs: HashMap<u64, (Vec<Entry>, Instant)>,
}

impl Server {
    pub fn new(backend: Arc<dyn Backend>) -> Result<Server> {
        Server::with_cache(backend, PathBuf::from(CACHE))
    }

    /// A server whose files on their way to and from the server wait in
    /// `temp`, which the tests give a folder of their own.
    pub fn with_cache(backend: Arc<dyn Backend>, temp: PathBuf) -> Result<Server> {
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).with_context(|| format!("make {}", temp.display()))?;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o700)).with_context(|| format!("keep {}", temp.display()))?;
        let mut nodes = HashMap::new();
        nodes.insert(
            ROOT,
            Node {
                parent: ROOT,
                name: String::new(),
                path: String::new(),
                stat: Stat { kind: Kind::Folder, size: 0, modified: now(), etag: None },
                at: Instant::now(),
            },
        );
        Ok(Server {
            backend,
            nodes,
            by_path: HashMap::from([(String::new(), ROOT)]),
            next_node: 2,
            handles: HashMap::new(),
            next_handle: 1,
            temp,
            dirs: HashMap::new(),
        })
    }

    pub fn handle(&mut self, request: &[u8]) -> Result<Handled> {
        let (header, body) = fuse_in_header::read_from_prefix(request).map_err(|_| anyhow!("a request of {} bytes", request.len()))?;
        let unique = header.unique;
        let Ok(opcode) = fuse_opcode::try_from(header.opcode) else {
            return Ok(Handled::Reply(error(unique, libc::ENOSYS)));
        };
        Ok(match opcode {
            fuse_opcode::FUSE_INIT => {
                let (init, _) = fuse_init_in::read_from_prefix(body).map_err(|_| anyhow!("a short INIT"))?;
                Handled::Reply(self.init(unique, &init))
            }
            fuse_opcode::FUSE_LOOKUP => {
                let name = body.split(|&b| b == 0).next().unwrap_or_default();
                self.lookup(unique, header.nodeid, name)
            }
            fuse_opcode::FUSE_FORGET => {
                let (forget, _) = polyfuse_kernel::fuse_forget_in::read_from_prefix(body).map_err(|_| anyhow!("a short FORGET"))?;
                self.forget(header.nodeid, forget.nlookup);
                Handled::Silent
            }
            fuse_opcode::FUSE_BATCH_FORGET | fuse_opcode::FUSE_INTERRUPT => Handled::Silent,
            fuse_opcode::FUSE_GETATTR => self.getattr(unique, header.nodeid),
            fuse_opcode::FUSE_SETATTR => {
                let (attr, _) = fuse_setattr_in::read_from_prefix(body).map_err(|_| anyhow!("a short SETATTR"))?;
                self.setattr(unique, header.nodeid, &attr)
            }
            fuse_opcode::FUSE_OPEN => {
                let (open, _) = fuse_open_in::read_from_prefix(body).map_err(|_| anyhow!("a short OPEN"))?;
                self.open(unique, header.nodeid, open.flags)
            }
            fuse_opcode::FUSE_CREATE => {
                let (create, rest) = fuse_create_in::read_from_prefix(body).map_err(|_| anyhow!("a short CREATE"))?;
                let name = rest.split(|&b| b == 0).next().unwrap_or_default();
                self.create(unique, header.nodeid, name, create.flags)
            }
            fuse_opcode::FUSE_READ => {
                let (read, _) = fuse_read_in::read_from_prefix(body).map_err(|_| anyhow!("a short READ"))?;
                self.read(unique, &read)
            }
            fuse_opcode::FUSE_WRITE => {
                let (write, data) = fuse_write_in::read_from_prefix(body).map_err(|_| anyhow!("a short WRITE"))?;
                self.write(unique, &write, data)
            }
            fuse_opcode::FUSE_FLUSH => {
                let (flush, _) = polyfuse_kernel::fuse_flush_in::read_from_prefix(body).map_err(|_| anyhow!("a short FLUSH"))?;
                self.flush(unique, flush.fh)
            }
            fuse_opcode::FUSE_RELEASE => {
                let (release, _) = fuse_release_in::read_from_prefix(body).map_err(|_| anyhow!("a short RELEASE"))?;
                self.release(unique, release.fh)
            }
            fuse_opcode::FUSE_FSYNC => {
                let (sync, _) = fuse_fsync_in::read_from_prefix(body).map_err(|_| anyhow!("a short FSYNC"))?;
                self.flush(unique, sync.fh)
            }
            fuse_opcode::FUSE_OPENDIR => Handled::Reply(reply(unique, fuse_open_out::default().as_bytes())),
            fuse_opcode::FUSE_READDIR => {
                let (read, _) = fuse_read_in::read_from_prefix(body).map_err(|_| anyhow!("a short READDIR"))?;
                self.readdir(unique, header.nodeid, read.offset, read.size)
            }
            fuse_opcode::FUSE_RELEASEDIR | fuse_opcode::FUSE_FSYNCDIR => Handled::Reply(reply(unique, &[])),
            fuse_opcode::FUSE_MKDIR => {
                let (mkdir, rest) = fuse_mkdir_in::read_from_prefix(body).map_err(|_| anyhow!("a short MKDIR"))?;
                let name = rest.split(|&b| b == 0).next().unwrap_or_default();
                self.mkdir(unique, header.nodeid, name, mkdir.mode)
            }
            fuse_opcode::FUSE_UNLINK => {
                let name = body.split(|&b| b == 0).next().unwrap_or_default();
                self.unlink(unique, header.nodeid, name)
            }
            fuse_opcode::FUSE_RMDIR => {
                let name = body.split(|&b| b == 0).next().unwrap_or_default();
                self.rmdir(unique, header.nodeid, name)
            }
            fuse_opcode::FUSE_RENAME => {
                let (rename, rest) = fuse_rename_in::read_from_prefix(body).map_err(|_| anyhow!("a short RENAME"))?;
                let mut names = rest.split(|&b| b == 0);
                let old = names.next().unwrap_or_default();
                let new = names.next().unwrap_or_default();
                self.rename(unique, header.nodeid, old, rename.newdir, new)
            }
            fuse_opcode::FUSE_STATFS => self.statfs(unique),
            fuse_opcode::FUSE_ACCESS => Handled::Reply(reply(unique, &[])),
            fuse_opcode::FUSE_GETXATTR => {
                let (_get, _) = fuse_getxattr_in::read_from_prefix(body).map_err(|_| anyhow!("a short GETXATTR"))?;
                Handled::Reply(reply(unique, fuse_getxattr_out { size: 0, ..Default::default() }.as_bytes()))
            }
            fuse_opcode::FUSE_LISTXATTR => {
                let (get, _) = fuse_getxattr_in::read_from_prefix(body).map_err(|_| anyhow!("a short LISTXATTR"))?;
                if get.size == 0 {
                    Handled::Reply(reply(unique, fuse_getxattr_out { size: 0, ..Default::default() }.as_bytes()))
                } else {
                    Handled::Reply(reply(unique, &[]))
                }
            }
            fuse_opcode::FUSE_SETXATTR | fuse_opcode::FUSE_REMOVEXATTR => Handled::Reply(reply(unique, &[])),
            fuse_opcode::FUSE_DESTROY => Handled::Exit(reply(unique, &[])),
            _ => Handled::Reply(error(unique, libc::ENOSYS)),
        })
    }

    fn init(&self, unique: u64, init: &fuse_init_in) -> Vec<u8> {
        let out = fuse_init_out {
            major: FUSE_KERNEL_VERSION,
            minor: init.minor.min(FUSE_KERNEL_MINOR_VERSION),
            max_readahead: init.max_readahead,
            flags: FUSE_ASYNC_READ | FUSE_BIG_WRITES,
            max_background: 16,
            congestion_threshold: 12,
            max_write: MAX_WRITE,
            time_gran: 1,
            ..Default::default()
        };
        let len = if init.minor <= 22 { FUSE_COMPAT_22_INIT_OUT_SIZE } else { size_of::<fuse_init_out>() };
        reply(unique, &out.as_bytes()[..len])
    }

    fn node(&mut self, parent: u64, name: &str, path: String, stat: Stat) -> u64 {
        if let Some(&id) = self.by_path.get(&path) {
            if let Some(node) = self.nodes.get_mut(&id) {
                node.stat = stat;
                node.at = Instant::now();
            }
            return id;
        }
        let id = self.next_node;
        self.next_node += 1;
        self.nodes.insert(id, Node { parent, name: name.to_string(), path: path.clone(), stat, at: Instant::now() });
        self.by_path.insert(path, id);
        id
    }

    fn entry(&self, id: u64) -> fuse_entry_out {
        let node = self.nodes.get(&id).expect("an entry was just made");
        fuse_entry_out {
            nodeid: id,
            generation: id,
            entry_valid: TTL,
            attr_valid: TTL,
            attr: attr(id, &node.stat),
            ..Default::default()
        }
    }

    fn lookup(&mut self, unique: u64, parent: u64, name: &[u8]) -> Handled {
        let Some(name) = name_of(name) else { return Handled::Reply(error(unique, libc::EINVAL)) };
        let Some(parent_node) = self.nodes.get(&parent).cloned() else { return Handled::Reply(error(unique, libc::ENOENT)) };
        if name == "." {
            return Handled::Reply(reply(unique, self.entry(parent).as_bytes()));
        }
        if name == ".." {
            let id = parent_node.parent;
            return Handled::Reply(reply(unique, self.entry(id).as_bytes()));
        }
        let path = join(&parent_node.path, &name);
        match self.backend.stat(&path) {
            Ok(stat) => {
                let id = self.node(parent, &name, path, stat);
                Handled::Reply(reply(unique, self.entry(id).as_bytes()))
            }
            Err(e) => Handled::Reply(error(unique, errno(&e))),
        }
    }

    fn forget(&mut self, node: u64, _lookups: u64) {
        if node == ROOT || self.handles.values().any(|h| h.node == node) {
            return;
        }
        if let Some(n) = self.nodes.remove(&node) {
            self.by_path.remove(&n.path);
            self.dirs.remove(&node);
        }
    }

    fn getattr(&mut self, unique: u64, node: u64) -> Handled {
        let Some(held) = self.nodes.get(&node).cloned() else { return Handled::Reply(error(unique, libc::ENOENT)) };
        let stat = match held.at.elapsed() < DIR_TTL {
            true => held.stat,
            false => self.backend.stat(&held.path).unwrap_or(held.stat),
        };
        if let Some(n) = self.nodes.get_mut(&node) {
            n.stat = stat.clone();
            n.at = Instant::now();
        }
        let out = fuse_attr_out { attr_valid: TTL, attr: attr(node, &stat), ..Default::default() };
        Handled::Reply(reply(unique, out.as_bytes()))
    }

    fn setattr(&mut self, unique: u64, node: u64, set: &fuse_setattr_in) -> Handled {
        let Some(held) = self.nodes.get(&node).cloned() else { return Handled::Reply(error(unique, libc::ENOENT)) };
        if set.valid & FATTR_SIZE != 0 {
            let fh = if set.valid & FATTR_FH != 0 { Some(set.fh) } else { self.handles.iter().find(|(_, h)| h.node == node).map(|(&fh, _)| fh) };
            match fh {
                Some(fh) if self.handles.contains_key(&fh) => {
                    let handle = self.handles.get_mut(&fh).expect("the handle is there");
                    if let Err(e) = handle.file.set_len(set.size) {
                        return Handled::Reply(error(unique, io_errno(&e)));
                    }
                    handle.size = set.size;
                    handle.dirty = true;
                }
                _ => {
                    if let Err(e) = self.truncate(&held.path, set.size) {
                        return Handled::Reply(error(unique, errno(&e)));
                    }
                }
            }
        }
        if let Some(n) = self.nodes.get_mut(&node) {
            if set.valid & (FATTR_MTIME | FATTR_MTIME_NOW) != 0 {
                n.stat.modified = if set.valid & FATTR_MTIME_NOW != 0 { now() } else { set.mtime as i64 };
            } else if set.valid & (FATTR_ATIME | FATTR_ATIME_NOW) != 0 {
                n.stat.modified = if set.valid & FATTR_ATIME_NOW != 0 { now() } else { set.atime as i64 };
            }
            let _ = (FATTR_MODE, FATTR_UID, FATTR_GID);
            n.at = Instant::now();
            let out = fuse_attr_out { attr_valid: TTL, attr: attr(node, &n.stat), ..Default::default() };
            return Handled::Reply(reply(unique, out.as_bytes()));
        }
        Handled::Reply(error(unique, libc::ENOENT))
    }

    fn truncate(&self, path: &str, size: u64) -> std::result::Result<(), Error> {
        let temp = self.temp.join(format!("truncate-{}", std::process::id()));
        let mut file = File::create(&temp).map_err(|e| Error::new(ErrorKind::Network, e.to_string()))?;
        if size > 0 {
            match self.backend.get(path, &mut file) {
                Ok(_) => {}
                Err(e) if e.kind == ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        file.set_len(size).map_err(|e| Error::new(ErrorKind::Network, e.to_string()))?;
        let result = self.backend.put(path, &file);
        let _ = fs::remove_file(&temp);
        result
    }

    fn open(&mut self, unique: u64, node: u64, flags: u32) -> Handled {
        let Some(held) = self.nodes.get(&node).cloned() else { return Handled::Reply(error(unique, libc::ENOENT)) };
        let writable = flags & (libc::O_WRONLY as u32 | libc::O_RDWR as u32) != 0;
        let truncate = flags & libc::O_TRUNC as u32 != 0;
        let fh = self.next_handle;
        self.next_handle += 1;
        let temp = self.temp.join(format!("{node}-{fh}"));
        let mut file = match OpenOptions::new().read(true).write(true).create(true).truncate(true).mode(0o600).open(&temp) {
            Ok(file) => file,
            Err(e) => return Handled::Reply(error(unique, io_errno(&e))),
        };
        if !truncate {
            match self.backend.get(&held.path, &mut file) {
                Ok(_) => {}
                Err(e) if e.kind == ErrorKind::NotFound && writable && flags & libc::O_CREAT as u32 != 0 => {}
                Err(e) => {
                    let _ = fs::remove_file(&temp);
                    return Handled::Reply(error(unique, errno(&e)));
                }
            }
        }
        let size = held.stat.size;
        self.handles.insert(fh, Handle { node, file, temp, dirty: false, size });
        Handled::Reply(reply(unique, fuse_open_out { fh, open_flags: 0, ..Default::default() }.as_bytes()))
    }

    fn create(&mut self, unique: u64, parent: u64, name: &[u8], flags: u32) -> Handled {
        let Some(name) = name_of(name) else { return Handled::Reply(error(unique, libc::EINVAL)) };
        let Some(parent_node) = self.nodes.get(&parent).cloned() else { return Handled::Reply(error(unique, libc::ENOENT)) };
        let path = join(&parent_node.path, &name);
        match self.backend.stat(&path) {
            Ok(_) => return Handled::Reply(error(unique, libc::EEXIST)),
            Err(e) if e.kind == ErrorKind::NotFound => {}
            Err(e) => return Handled::Reply(error(unique, errno(&e))),
        }
        let fh = self.next_handle;
        self.next_handle += 1;
        let temp = self.temp.join(format!("new-{fh}"));
        let file = match OpenOptions::new().read(true).write(true).create(true).truncate(true).mode(0o600).open(&temp) {
            Ok(file) => file,
            Err(e) => return Handled::Reply(error(unique, io_errno(&e))),
        };
        let stat = Stat { kind: Kind::File, size: 0, modified: now(), etag: None };
        let id = self.node(parent, &name, path, stat);
        self.dirs.remove(&parent);
        self.handles.insert(fh, Handle { node: id, file, temp, dirty: true, size: 0 });
        let _ = flags;
        let entry = self.entry(id);
        let open = fuse_open_out { fh, open_flags: 0, ..Default::default() };
        let mut out = Vec::with_capacity(size_of::<fuse_entry_out>() + size_of::<fuse_open_out>());
        out.extend_from_slice(entry.as_bytes());
        out.extend_from_slice(open.as_bytes());
        Handled::Reply(reply(unique, &out))
    }

    fn read(&mut self, unique: u64, read: &fuse_read_in) -> Handled {
        let Some(handle) = self.handles.get(&read.fh) else { return Handled::Reply(error(unique, libc::EBADF)) };
        let mut buf = vec![0u8; read.size as usize];
        match handle.file.read_at(&mut buf, read.offset) {
            Ok(n) => {
                buf.truncate(n);
                Handled::Reply(reply(unique, &buf))
            }
            Err(e) => Handled::Reply(error(unique, io_errno(&e))),
        }
    }

    fn write(&mut self, unique: u64, write: &fuse_write_in, data: &[u8]) -> Handled {
        let Some(handle) = self.handles.get_mut(&write.fh) else { return Handled::Reply(error(unique, libc::EBADF)) };
        let data = &data[..(write.size as usize).min(data.len())];
        match handle.file.write_at(data, write.offset) {
            Ok(n) => {
                handle.dirty = true;
                handle.size = handle.size.max(write.offset + n as u64);
                Handled::Reply(reply(unique, fuse_write_out { size: n as u32, ..Default::default() }.as_bytes()))
            }
            Err(e) => Handled::Reply(error(unique, io_errno(&e))),
        }
    }

    fn flush(&mut self, unique: u64, fh: u64) -> Handled {
        match self.flush_handle(fh) {
            Ok(()) => Handled::Reply(reply(unique, &[])),
            Err(e) => Handled::Reply(error(unique, errno(&e))),
        }
    }

    fn flush_handle(&mut self, fh: u64) -> std::result::Result<(), Error> {
        let Some(node) = self.handles.get(&fh).map(|h| h.node) else { return Ok(()) };
        if !self.handles.get(&fh).is_some_and(|h| h.dirty) {
            return Ok(());
        }
        let Some(path) = self.nodes.get(&node).map(|n| n.path.clone()) else {
            return Err(Error::new(ErrorKind::NotFound, "the entry is gone"));
        };
        let handle = self.handles.get_mut(&fh).expect("the handle is there");
        handle.file.sync_all().map_err(|e| Error::new(ErrorKind::Network, e.to_string()))?;
        self.backend.put(&path, &handle.file)?;
        handle.dirty = false;
        self.dirs.clear();
        let size = handle.size;
        if let Some(n) = self.nodes.get_mut(&node) {
            n.stat.size = size;
            n.stat.modified = now();
            n.at = Instant::now();
        }
        Ok(())
    }

    fn release(&mut self, unique: u64, fh: u64) -> Handled {
        let result = self.flush_handle(fh);
        if let Some(handle) = self.handles.remove(&fh) {
            let _ = fs::remove_file(&handle.temp);
        }
        match result {
            Ok(()) => Handled::Reply(reply(unique, &[])),
            Err(e) => Handled::Reply(error(unique, errno(&e))),
        }
    }

    fn readdir(&mut self, unique: u64, node: u64, offset: u64, size: u32) -> Handled {
        let Some(held) = self.nodes.get(&node).cloned() else { return Handled::Reply(error(unique, libc::ENOENT)) };
        let entries = match self.list(node, &held.path) {
            Ok(entries) => entries,
            Err(e) => return Handled::Reply(error(unique, errno(&e))),
        };
        let mut out = Vec::new();
        for (index, entry) in entries.iter().enumerate().skip(offset as usize) {
            let id = match entry.name.as_str() {
                "." => node,
                ".." => held.parent,
                _ => match self.by_path.get(&join(&held.path, &entry.name)) {
                    Some(&id) => id,
                    None => continue,
                },
            };
            let typ = match entry.stat.kind {
                Kind::Folder => DT_DIR,
                Kind::File => DT_REG,
            };
            let name = entry.name.as_bytes();
            let dirent = fuse_dirent { ino: id, off: index as u64 + 1, namelen: name.len() as u32, typ, name: [] };
            let padded = (name.len() + 7) & !7;
            let need = size_of::<fuse_dirent>() + padded;
            if out.len() + need > size as usize {
                break;
            }
            out.extend_from_slice(dirent.as_bytes());
            out.extend_from_slice(name);
            out.resize(out.len() + (padded - name.len()), 0);
        }
        Handled::Reply(reply(unique, &out))
    }

    fn list(&mut self, node: u64, path: &str) -> std::result::Result<Vec<Entry>, Error> {
        if let Some((entries, at)) = self.dirs.get(&node) {
            if at.elapsed() < DIR_TTL {
                return Ok(entries.clone());
            }
        }
        let mut entries = vec![
            Entry { name: ".".into(), stat: Stat { kind: Kind::Folder, size: 0, modified: now(), etag: None } },
            Entry { name: "..".into(), stat: Stat { kind: Kind::Folder, size: 0, modified: now(), etag: None } },
        ];
        for entry in self.backend.list(path)? {
            let id = self.node(node, &entry.name, join(path, &entry.name), entry.stat.clone());
            let _ = id;
            entries.push(entry);
        }
        self.dirs.insert(node, (entries.clone(), Instant::now()));
        Ok(entries)
    }

    fn mkdir(&mut self, unique: u64, parent: u64, name: &[u8], _mode: u32) -> Handled {
        let Some(name) = name_of(name) else { return Handled::Reply(error(unique, libc::EINVAL)) };
        let Some(parent_node) = self.nodes.get(&parent).cloned() else { return Handled::Reply(error(unique, libc::ENOENT)) };
        let path = join(&parent_node.path, &name);
        match self.backend.mkdir(&path) {
            Ok(()) => {
                let id = self.node(parent, &name, path, Stat { kind: Kind::Folder, size: 0, modified: now(), etag: None });
                self.dirs.remove(&parent);
                Handled::Reply(reply(unique, self.entry(id).as_bytes()))
            }
            Err(e) if e.kind == ErrorKind::Conflict => Handled::Reply(error(unique, libc::EEXIST)),
            Err(e) => Handled::Reply(error(unique, errno(&e))),
        }
    }

    fn unlink(&mut self, unique: u64, parent: u64, name: &[u8]) -> Handled {
        let Some(name) = name_of(name) else { return Handled::Reply(error(unique, libc::EINVAL)) };
        let Some(parent_node) = self.nodes.get(&parent).cloned() else { return Handled::Reply(error(unique, libc::ENOENT)) };
        let path = join(&parent_node.path, &name);
        match self.backend.remove(&path) {
            Ok(()) => {
                self.remove_path(&path);
                self.dirs.remove(&parent);
                Handled::Reply(reply(unique, &[]))
            }
            Err(e) => Handled::Reply(error(unique, errno(&e))),
        }
    }

    fn rmdir(&mut self, unique: u64, parent: u64, name: &[u8]) -> Handled {
        let Some(name) = name_of(name) else { return Handled::Reply(error(unique, libc::EINVAL)) };
        let Some(parent_node) = self.nodes.get(&parent).cloned() else { return Handled::Reply(error(unique, libc::ENOENT)) };
        let path = join(&parent_node.path, &name);
        match self.backend.list(&path) {
            Ok(entries) if !entries.is_empty() => return Handled::Reply(error(unique, libc::ENOTEMPTY)),
            Ok(_) => {}
            Err(e) => return Handled::Reply(error(unique, errno(&e))),
        }
        match self.backend.remove(&path) {
            Ok(()) => {
                self.remove_path(&path);
                self.dirs.remove(&parent);
                Handled::Reply(reply(unique, &[]))
            }
            Err(e) => Handled::Reply(error(unique, errno(&e))),
        }
    }

    fn rename(&mut self, unique: u64, parent: u64, old: &[u8], newparent: u64, new: &[u8]) -> Handled {
        let (Some(old), Some(new)) = (name_of(old), name_of(new)) else { return Handled::Reply(error(unique, libc::EINVAL)) };
        let (Some(from_parent), Some(to_parent)) = (self.nodes.get(&parent).cloned(), self.nodes.get(&newparent).cloned()) else {
            return Handled::Reply(error(unique, libc::ENOENT));
        };
        let from = join(&from_parent.path, &old);
        let to = join(&to_parent.path, &new);
        match self.backend.rename(&from, &to) {
            Ok(()) => {
                let prefix = format!("{from}/");
                for node in self.nodes.values_mut() {
                    if node.path == from {
                        node.path = to.clone();
                        node.name = new.clone();
                        node.parent = newparent;
                    } else if let Some(rest) = node.path.strip_prefix(&prefix) {
                        node.path = format!("{to}/{rest}");
                    }
                }
                self.by_path.clear();
                for (&id, node) in &self.nodes {
                    self.by_path.insert(node.path.clone(), id);
                }
                self.dirs.remove(&parent);
                self.dirs.remove(&newparent);
                Handled::Reply(reply(unique, &[]))
            }
            Err(e) => Handled::Reply(error(unique, errno(&e))),
        }
    }

    fn remove_path(&mut self, path: &str) {
        let prefix = format!("{path}/");
        let gone: Vec<u64> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.path == path || n.path.starts_with(&prefix))
            .map(|(&id, _)| id)
            .collect();
        for id in gone {
            if let Some(n) = self.nodes.remove(&id) {
                self.by_path.remove(&n.path);
            }
            self.dirs.remove(&id);
        }
    }

    fn statfs(&self, unique: u64) -> Handled {
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        let c = std::ffi::CString::new(self.temp.as_os_str().as_encoded_bytes()).expect("the cache path holds no NUL");
        let (blocks, bfree, bavail, bsize, frsize) = if unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0 {
            (st.f_blocks, st.f_bfree, st.f_bavail, st.f_bsize as u32, st.f_frsize as u32)
        } else {
            (0, 0, 0, 4096, 4096)
        };
        let out = fuse_statfs_out {
            st: fuse_kstatfs { blocks, bfree, bavail, files: 0, ffree: 0, bsize, namelen: 255, frsize, ..Default::default() },
        };
        Handled::Reply(reply(unique, out.as_bytes()))
    }
}

fn name_of(name: &[u8]) -> Option<String> {
    let name = std::str::from_utf8(name).ok()?;
    if name.is_empty() || name.contains('/') || name.chars().any(char::is_control) {
        return None;
    }
    Some(name.to_string())
}

fn join(parent: &str, name: &str) -> String {
    match parent.is_empty() {
        true => format!("/{name}"),
        false => format!("{parent}/{name}"),
    }
}

fn attr(ino: u64, stat: &Stat) -> fuse_attr {
    let (mode, nlink) = match stat.kind {
        Kind::Folder => (S_IFDIR | 0o755, 2),
        Kind::File => (S_IFREG | 0o644, 1),
    };
    fuse_attr {
        ino,
        size: stat.size,
        blocks: stat.size.div_ceil(512),
        atime: stat.modified as u64,
        mtime: stat.modified as u64,
        ctime: stat.modified as u64,
        mode,
        nlink,
        uid: 0,
        gid: 0,
        blksize: 4096,
        ..Default::default()
    }
}

fn errno(error: &Error) -> i32 {
    match error.kind {
        ErrorKind::NotFound => libc::ENOENT,
        ErrorKind::Forbidden | ErrorKind::Unauthorized => libc::EACCES,
        ErrorKind::Conflict => libc::EEXIST,
        ErrorKind::NoSpace => libc::ENOSPC,
        ErrorKind::Invalid => libc::EINVAL,
        ErrorKind::Network | ErrorKind::Other => libc::EIO,
    }
}

fn io_errno(error: &std::io::Error) -> i32 {
    error.raw_os_error().unwrap_or(libc::EIO)
}

fn reply(unique: u64, body: &[u8]) -> Vec<u8> {
    let len = size_of::<fuse_out_header>() + body.len();
    let header = fuse_out_header { len: len as u32, error: 0, unique };
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(body);
    out
}

fn error(unique: u64, errno: i32) -> Vec<u8> {
    fuse_out_header { len: size_of::<fuse_out_header>() as u32, error: -errno, unique }.as_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Memory {
        files: Mutex<HashMap<String, Vec<u8>>>,
        dirs: Mutex<HashSet<String>>,
    }

    impl Memory {
        fn new() -> Memory {
            let memory = Memory::default();
            memory.dirs.lock().unwrap().insert(String::new());
            memory
        }
    }

    fn parent(path: &str) -> (&str, &str) {
        match path.rsplit_once('/') {
            Some((parent, name)) => (parent, name),
            None => ("", path),
        }
    }

    impl Backend for Memory {
        fn stat(&self, path: &str) -> std::result::Result<Stat, Error> {
            if self.dirs.lock().unwrap().contains(path) {
                return Ok(Stat { kind: Kind::Folder, size: 0, modified: 0, etag: None });
            }
            match self.files.lock().unwrap().get(path) {
                Some(data) => Ok(Stat { kind: Kind::File, size: data.len() as u64, modified: 0, etag: None }),
                None => Err(Error::new(ErrorKind::NotFound, "not found")),
            }
        }

        fn list(&self, path: &str) -> std::result::Result<Vec<Entry>, Error> {
            if !self.dirs.lock().unwrap().contains(path) {
                return Err(Error::new(ErrorKind::NotFound, "not found"));
            }
            let mut entries = Vec::new();
            for (file, data) in self.files.lock().unwrap().iter() {
                if parent(file).0 == path {
                    entries.push(Entry {
                        name: parent(file).1.to_string(),
                        stat: Stat { kind: Kind::File, size: data.len() as u64, modified: 0, etag: None },
                    });
                }
            }
            for dir in self.dirs.lock().unwrap().iter() {
                if dir != path && parent(dir).0 == path {
                    entries.push(Entry {
                        name: parent(dir).1.to_string(),
                        stat: Stat { kind: Kind::Folder, size: 0, modified: 0, etag: None },
                    });
                }
            }
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(entries)
        }

        fn get(&self, path: &str, out: &mut dyn Write) -> std::result::Result<u64, Error> {
            let files = self.files.lock().unwrap();
            let data = files.get(path).ok_or_else(|| Error::new(ErrorKind::NotFound, "not found"))?;
            out.write_all(data).map_err(|e| Error::new(ErrorKind::Network, e.to_string()))?;
            Ok(data.len() as u64)
        }

        fn put(&self, path: &str, file: &File) -> std::result::Result<(), Error> {
            let mut file = file;
            file.seek(SeekFrom::Start(0)).map_err(|e| Error::new(ErrorKind::Network, e.to_string()))?;
            let mut data = Vec::new();
            file.read_to_end(&mut data).map_err(|e| Error::new(ErrorKind::Network, e.to_string()))?;
            self.files.lock().unwrap().insert(path.to_string(), data);
            Ok(())
        }

        fn mkdir(&self, path: &str) -> std::result::Result<(), Error> {
            if self.dirs.lock().unwrap().contains(path) {
                return Err(Error::new(ErrorKind::Conflict, "exists"));
            }
            self.dirs.lock().unwrap().insert(path.to_string());
            Ok(())
        }

        fn remove(&self, path: &str) -> std::result::Result<(), Error> {
            self.files.lock().unwrap().remove(path);
            let prefix = format!("{path}/");
            self.files.lock().unwrap().retain(|p, _| !p.starts_with(&prefix));
            self.dirs.lock().unwrap().retain(|p| p != path && !p.starts_with(&prefix));
            Ok(())
        }

        fn rename(&self, from: &str, to: &str) -> std::result::Result<(), Error> {
            let mut files = self.files.lock().unwrap();
            if let Some(data) = files.remove(from) {
                files.insert(to.to_string(), data);
            }
            let prefix = format!("{from}/");
            let moved: Vec<(String, Vec<u8>)> = files.iter().filter(|(p, _)| p.starts_with(&prefix)).map(|(p, d)| (p.clone(), d.clone())).collect();
            for (path, data) in moved {
                files.remove(&path);
                files.insert(format!("{to}/{}", &path[prefix.len()..]), data);
            }
            drop(files);
            let mut dirs = self.dirs.lock().unwrap();
            if dirs.remove(from) {
                dirs.insert(to.to_string());
            }
            let moved: Vec<String> = dirs.iter().filter(|p| p.starts_with(&prefix)).cloned().collect();
            for path in moved {
                dirs.remove(&path);
                dirs.insert(format!("{to}/{}", &path[prefix.len()..]));
            }
            Ok(())
        }

        fn quota(&self) -> std::result::Result<Option<u64>, Error> {
            Ok(Some(1 << 30))
        }
    }

    fn request_raw(opcode: fuse_opcode, node: u64, body: &[u8]) -> Vec<u8> {
        let header = fuse_in_header {
            len: (size_of::<fuse_in_header>() + body.len()) as u32,
            opcode: opcode as u32,
            unique: 1,
            nodeid: node,
            ..Default::default()
        };
        let mut out = header.as_bytes().to_vec();
        out.extend_from_slice(body);
        out
    }

    fn request<T: IntoBytes + zerocopy::Immutable>(opcode: fuse_opcode, node: u64, body: &T) -> Vec<u8> {
        request_raw(opcode, node, body.as_bytes())
    }

    fn reply(handled: Handled) -> Vec<u8> {
        match handled {
            Handled::Reply(reply) => reply,
            other => panic!("{other:?}"),
        }
    }

    fn body(reply: &[u8]) -> &[u8] {
        &reply[size_of::<fuse_out_header>()..]
    }

    fn lookup(server: &mut Server, name: &str) -> fuse_entry_out {
        let mut raw = name.as_bytes().to_vec();
        raw.push(0);
        let reply = reply(server.handle(&request_raw(fuse_opcode::FUSE_LOOKUP, ROOT, &raw)).unwrap());
        fuse_entry_out::read_from_prefix(body(&reply)).unwrap().0
    }

    fn open(server: &mut Server, node: u64, flags: u32) -> fuse_open_out {
        let reply = reply(server.handle(&request(fuse_opcode::FUSE_OPEN, node, &fuse_open_in { flags, ..Default::default() })).unwrap());
        fuse_open_out::read_from_prefix(body(&reply)).unwrap().0
    }

    #[test]
    fn a_file_is_read_from_the_server_and_written_back() {
        let memory = Arc::new(Memory::new());
        memory.files.lock().unwrap().insert("/hello.txt".into(), b"hi".to_vec());
        let temp = tempfile::tempdir().unwrap();
        let mut server = Server::with_cache(memory.clone(), temp.path().to_path_buf()).unwrap();
        let entry = lookup(&mut server, "hello.txt");
        assert_eq!((entry.attr.size, entry.attr.mode & S_IFREG), (2, S_IFREG));
        let opened = open(&mut server, entry.nodeid, libc::O_RDONLY as u32);
        let read = reply(
            server
                .handle(&request(
                    fuse_opcode::FUSE_READ,
                    entry.nodeid,
                    &fuse_read_in { fh: opened.fh, offset: 0, size: 16, ..Default::default() },
                ))
                .unwrap(),
        );
        assert_eq!(body(&read), b"hi");
        reply(server.handle(&request(fuse_opcode::FUSE_RELEASE, entry.nodeid, &fuse_release_in { fh: opened.fh, ..Default::default() })).unwrap());

        let mut raw = b"new.txt\0".to_vec();
        let create = fuse_create_in { flags: (libc::O_RDWR | libc::O_CREAT) as u32, mode: 0o644, ..Default::default() };
        let mut create_body = create.as_bytes().to_vec();
        create_body.append(&mut raw);
        let created = reply(server.handle(&request_raw(fuse_opcode::FUSE_CREATE, ROOT, &create_body)).unwrap());
        let entry = fuse_entry_out::read_from_prefix(body(&created)).unwrap().0;
        let opened = fuse_open_out::read_from_prefix(&body(&created)[size_of::<fuse_entry_out>()..]).unwrap().0;
        let write = fuse_write_in { fh: opened.fh, offset: 0, size: 5, ..Default::default() };
        let mut write_body = write.as_bytes().to_vec();
        write_body.extend_from_slice(b"hello");
        reply(server.handle(&request_raw(fuse_opcode::FUSE_WRITE, entry.nodeid, &write_body)).unwrap());
        reply(server.handle(&request(fuse_opcode::FUSE_FLUSH, entry.nodeid, &polyfuse_kernel::fuse_flush_in { fh: opened.fh, ..Default::default() })).unwrap());
        reply(server.handle(&request(fuse_opcode::FUSE_RELEASE, entry.nodeid, &fuse_release_in { fh: opened.fh, ..Default::default() })).unwrap());
        assert_eq!(memory.files.lock().unwrap().get("/new.txt").map(Vec::as_slice), Some(&b"hello"[..]));
    }

    #[test]
    fn a_folder_is_listed_made_renamed_and_removed() {
        let memory = Arc::new(Memory::new());
        let temp = tempfile::tempdir().unwrap();
        let mut server = Server::with_cache(memory.clone(), temp.path().to_path_buf()).unwrap();
        let mut raw = b"folder\0".to_vec();
        let mkdir = fuse_mkdir_in { mode: 0o755, ..Default::default() };
        let mut packet = mkdir.as_bytes().to_vec();
        packet.append(&mut raw);
        let made = reply(server.handle(&request_raw(fuse_opcode::FUSE_MKDIR, ROOT, &packet)).unwrap());
        let entry = fuse_entry_out::read_from_prefix(body(&made)).unwrap().0;
        assert_eq!(entry.attr.mode & S_IFDIR, S_IFDIR);
        let read = reply(
            server
                .handle(&request(
                    fuse_opcode::FUSE_READDIR,
                    ROOT,
                    &fuse_read_in { fh: 0, offset: 0, size: 4096, ..Default::default() },
                ))
                .unwrap(),
        );
        let names = dirents(body(&read));
        assert!(names.contains(&"folder".to_string()), "{names:?}");

        let rename = fuse_rename_in { newdir: ROOT };
        let mut packet = rename.as_bytes().to_vec();
        packet.extend_from_slice(b"folder\0renamed\0");
        reply(server.handle(&request_raw(fuse_opcode::FUSE_RENAME, ROOT, &packet)).unwrap());
        assert!(memory.dirs.lock().unwrap().contains("/renamed"));

        let raw = b"renamed\0".to_vec();
        reply(server.handle(&request_raw(fuse_opcode::FUSE_RMDIR, ROOT, &raw)).unwrap());
        assert!(!memory.dirs.lock().unwrap().contains("/renamed"));
    }

    fn dirents(body: &[u8]) -> Vec<String> {
        let mut names = Vec::new();
        let mut at = 0;
        while at + size_of::<fuse_dirent>() <= body.len() {
            let (dirent, rest) = fuse_dirent::read_from_prefix(&body[at..]).unwrap();
            let len = dirent.namelen as usize;
            let padded = (len + 7) & !7;
            if rest.len() < padded {
                break;
            }
            names.push(String::from_utf8_lossy(&rest[..len]).into_owned());
            at += size_of::<fuse_dirent>() + padded;
        }
        names
    }
}

#[cfg(any(target_os = "android", test))]
pub use device::{mount, Mount};

#[cfg(any(target_os = "android", test))]
mod device {
    use super::{Server, MOUNT_POINT, REQUEST_BUFFER};
    use anyhow::{Context, Result};
    use std::ffi::CString;
    use std::fs::{self, File, OpenOptions};
    use std::io::{ErrorKind, Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::thread::{self, JoinHandle};

    /// The cloud storage, mounted and served from a thread of its own.
    pub struct Mount {
        point: PathBuf,
        thread: Option<JoinHandle<Result<()>>>,
        unmounted: bool,
    }

    /// Mount `backend` at `/mnt/cloud`, read-write.
    pub fn mount(backend: Arc<dyn super::Backend>) -> Result<Mount> {
        let point = Path::new(MOUNT_POINT);
        if !point.try_exists().with_context(|| format!("look for {MOUNT_POINT}"))? {
            fs::create_dir(point).with_context(|| format!("create {MOUNT_POINT}"))?;
        }
        if skyward_block::mount::is_mount_point(point)? {
            skyward_block::mount::umount_detach(point)?;
        }
        let dev = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open("/dev/fuse")
            .context("open /dev/fuse")?;
        let options = format!("fd={},user_id=0,group_id=0,max_read={},rootmode=040000", dev.as_raw_fd(), super::MAX_WRITE);
        let (source, target, kind, data) =
            (CString::new("/dev/fuse")?, CString::new(MOUNT_POINT)?, CString::new("fuse")?, CString::new(options)?);
        let flags = libc::MS_NOSUID | libc::MS_NODEV;
        if unsafe { libc::mount(source.as_ptr(), target.as_ptr(), kind.as_ptr(), flags, data.as_ptr().cast()) } != 0 {
            return Err(std::io::Error::last_os_error()).context("mount the cloud storage at /mnt/cloud");
        }
        let server = Server::new(backend)?;
        let thread = thread::Builder::new().name("cloud-fuse".into()).spawn(move || serve(dev, server)).context("start the cloud filesystem")?;
        Ok(Mount { point: point.to_path_buf(), thread: Some(thread), unmounted: false })
    }

    fn serve(mut dev: File, mut server: Server) -> Result<()> {
        let mut buf = vec![0u8; REQUEST_BUFFER];
        loop {
            let n = match dev.read(&mut buf) {
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) if e.raw_os_error() == Some(libc::ENODEV) => return Ok(()),
                Err(e) => return Err(e).context("read a cloud filesystem request"),
            };
            let handled = server.handle(&buf[..n])?;
            match handled {
                super::Handled::Reply(reply) => answer(&dev, &reply)?,
                super::Handled::Silent => {}
                super::Handled::Exit(reply) => return answer(&dev, &reply),
            }
        }
    }

    fn answer(dev: &File, reply: &[u8]) -> Result<()> {
        match { dev }.write_all(reply) {
            Ok(()) => Ok(()),
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => Ok(()),
            Err(e) => Err(e).context("write a cloud filesystem reply"),
        }
    }

    impl Mount {
        pub fn is_mounted() -> Result<bool> {
            skyward_block::mount::is_mount_point(Path::new(MOUNT_POINT))
        }

        /// Unmount the storage and wait for its thread to end.
        pub fn unmount(mut self) -> Result<()> {
            self.unmounted = true;
            let point = CString::new(self.point.as_os_str().as_encoded_bytes())?;
            let unmounted = if unsafe { libc::umount2(point.as_ptr(), libc::MNT_DETACH) } != 0 {
                let e = std::io::Error::last_os_error();
                match e.raw_os_error() {
                    Some(libc::EINVAL | libc::ENOENT) => Ok(()),
                    _ => Err(e).context("unmount the cloud storage"),
                }
            } else {
                Ok(())
            };
            let served = self.thread.take().map(|t| t.join().expect("the cloud filesystem does not panic"));
            match (unmounted, served) {
                (Err(e), _) => Err(e),
                (Ok(()), Some(Err(e))) => Err(e),
                _ => Ok(()),
            }
        }
    }

    impl Drop for Mount {
        fn drop(&mut self) {
            if self.unmounted {
                return;
            }
            if let Ok(point) = CString::new(self.point.as_os_str().as_encoded_bytes()) {
                unsafe { libc::umount2(point.as_ptr(), libc::MNT_DETACH) };
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

}
