//! Cloud storage: a WebDAV server used as a folder, mounted through FUSE so
//! the rest of the recovery treats it like any other storage.

pub mod config;
pub mod fuse;
pub mod http;
pub mod webdav;

#[cfg(any(target_os = "android", test))]
pub mod worker;
