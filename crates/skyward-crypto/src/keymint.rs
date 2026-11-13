//! The phone's key service: the only thing that can use a key the phone's
//! secure world wrapped.
//!
//! A phone has one of two kinds. The newer kind, KeyMint, registers with the
//! service directory and speaks the newer layout; the older kind, Keymaster
//! 4.0, is a hardware service and speaks the older one. Both say which key
//! they hold, open a decryption with a key the phone keeps wrapped, feed it,
//! and close it, which is what opening the data partition needs; the rest of
//! what this program asks of them is in the two files beside this one.

use crate::token::Token;
use anyhow::Result;
#[cfg(target_os = "android")]
use anyhow::bail;
use skyward_binder::driver::Handle;
#[cfg(target_os = "android")]
use skyward_binder::driver::Binder;
#[cfg(target_os = "android")]
use skyward_binder::{hidl, service};
use std::time::Duration;

mod master;
mod mint;

/// The newer kind, as it registers with the service directory.
const SERVICE: &str = "android.hardware.security.keymint.IKeyMintDevice/default";

/// The older kind, a hardware service.
const MASTER_SERVICE: &str = "android.hardware.keymaster@4.0::IKeymasterDevice";

/// The instance either kind answers as.
const INSTANCE: &str = "default";

/// How long a call to the secure world is given. It can be slow to wake, but
/// a service that has not answered by then is one to say so about rather
/// than wait on.
const TIMEOUT: Duration = Duration::from_secs(15);

/// How long either kind is waited for at start-up, which is as long as the
/// secure world takes to load what it keeps.
const ARRIVAL: Duration = Duration::from_secs(90);

/// How long the nonce of an AES-GCM key is, which is what a key the phone
/// wrapped is wrapped under.
const NONCE_BYTES: usize = 12;

/// What the phone says about its key hardware.
pub struct Hardware {
    pub version: i32,
    pub security_level: i32,
    pub name: String,
    pub author: String,
}

/// A decryption the key service is asked for.
#[derive(Clone, Copy)]
pub struct Decryption<'a> {
    /// The key as the phone keeps it.
    pub key: &'a [u8],
    /// What the key is bound to, where it is bound to anything.
    pub app_id: &'a [u8],
    /// The nonce it was used under.
    pub nonce: &'a [u8],
    /// What was encrypted, with the seal over it at the end.
    pub body: &'a [u8],
    /// The note the secure world wrote where the key is guarded by the
    /// screen lock.
    pub note: Option<&'a Token>,
}

/// Which kind of key service the phone has.
enum Device<'a> {
    Mint(Handle<'a>),
    Master(Handle<'a>),
}

/// The phone's key service, as this program talks to it.
pub struct KeyMint<'a> {
    device: Device<'a>,
}

impl<'a> KeyMint<'a> {
    /// Reach the running key service, or say that none is.
    ///
    /// The phone's own key service takes a while to come up: it has to reach
    /// the secure world and load what it keeps there first. So this waits for
    /// it rather than giving up at once, and the page that asked says it is
    /// waiting while it does.
    #[cfg(target_os = "android")]
    pub fn open(binder: &'a Binder, hardware: Option<&'a Binder>) -> Result<KeyMint<'a>> {
        let until = std::time::Instant::now() + ARRIVAL;
        let mut why = None;
        loop {
            match service::find(binder, SERVICE) {
                Ok(Some(device)) => return Ok(KeyMint { device: Device::Mint(device) }),
                Ok(None) => {}
                Err(e) => why = Some(format!("{e:#}")),
            }
            if let Some(hardware) = hardware {
                match hidl::find(hardware, MASTER_SERVICE, INSTANCE) {
                    Ok(Some(device)) => return Ok(KeyMint { device: Device::Master(device) }),
                    Ok(None) => {}
                    Err(e) => why = Some(format!("{e:#}")),
                }
            }
            if std::time::Instant::now() >= until {
                let why = why.map(|w| format!(": {w}")).unwrap_or_default();
                bail!("{SERVICE} and {MASTER_SERVICE} are not running after {} seconds{why}", ARRIVAL.as_secs());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// What the phone says about its key hardware.
    pub fn hardware(&self) -> Result<Hardware> {
        match &self.device {
            Device::Mint(device) => mint::hardware(device),
            Device::Master(device) => master::hardware(device),
        }
    }

    /// Turn a key that only the storage hardware can use into the form it
    /// takes for this start of the phone. The key itself never leaves the
    /// secure world: what comes back is wrapped again, for the hardware to
    /// unwrap as it reads and writes.
    pub fn ephemeral_storage_key(&self, key: &[u8]) -> Result<Vec<u8>> {
        match &self.device {
            Device::Mint(device) => mint::ephemeral_storage_key(device, key),
            Device::Master(device) => master::ephemeral_storage_key(device),
        }
    }

    /// What the key service says a key may be used for: each entry is a tag
    /// and the number or length that came with it.
    pub fn authorizations(&self, key: &[u8], app_id: &[u8]) -> Result<Vec<(i32, i64)>> {
        match &self.device {
            Device::Mint(device) => mint::authorizations(device, key, app_id),
            Device::Master(device) => master::authorizations(device, key, app_id),
        }
    }

    /// Undo an AES-GCM encryption the key service did with `want.key`. The
    /// service only hands back the plaintext when `want.app_id` is the one
    /// the key was made for, so a wrong secret fails here rather than
    /// yielding something that looks like a key.
    pub fn gcm_decrypt(&self, want: &Decryption) -> Result<Vec<u8>> {
        match &self.device {
            Device::Mint(device) => mint::gcm_decrypt(device, want),
            Device::Master(device) => master::gcm_decrypt(device, want),
        }
    }

    /// Whether the secure world will unwrap the key at all: a decryption is
    /// opened on it under a nonce of nothing and dropped again, which reads
    /// nothing and changes nothing. It tells a key the secure world cannot
    /// unwrap, as one made under another build or bound to something not
    /// given back to it is, apart from one that merely decrypts to nothing.
    pub fn usable(&self, key: &[u8], app_id: &[u8]) -> Result<()> {
        match &self.device {
            Device::Mint(device) => mint::usable(device, key, app_id),
            Device::Master(device) => master::usable(device, key, app_id),
        }
    }
}
