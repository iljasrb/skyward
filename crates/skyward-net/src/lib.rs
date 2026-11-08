pub mod dhcp;
pub mod dns;
pub mod download;
// The fork's own GitHub client, which `release.rs` held before the sync.
// Upstream's `github.rs` is the same feature written for its release format,
// so the name matches even though the code does not.
pub mod github;
pub mod http;
#[cfg(target_os = "android")]
pub mod iface;
pub mod reach;
// Upstream's update-server protocol, kept as it is. The fork's own updates
// go through `rom.rs` and its own releases through `github.rs`; this stays
// so a merge of upstream lands here without a conflict.
pub mod release;
pub mod rom;
pub mod supplicant;
pub mod wlan;

/// Work stopped because it was asked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped;

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stopped as asked")
    }
}

impl std::error::Error for Stopped {}
