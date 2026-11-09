use crate::token::Token;
use anyhow::{ensure, Context, Result};
use skyward_binder::driver::{Binder, Handle};
use skyward_binder::hidl::{self, Answer, Message, Note};
use skyward_binder::parcel::Parcel;
use skyward_binder::{service, status};
use std::time::Duration;

/// The phone's own key service, the only thing that can use a key the
/// hardware wrapped. The newer kind registers with the service directory;
/// the older kind, which the r8q vendor has, is a hardware service.
pub const SERVICE: &str = "android.hardware.security.keymint.IKeyMintDevice/default";
pub const MASTER_SERVICE: &str = "android.hardware.keymaster@4.0::IKeymasterDevice";

const DEVICE: &str = "android.hardware.security.keymint.IKeyMintDevice";
const OPERATION: &str = "android.hardware.security.keymint.IKeyMintOperation";

const GET_HARDWARE_INFO: u32 = 1;

/// The newer interface's own numbering, which is a different one from the
/// older interface's even where the calls are named alike, and the older
/// interface's, which the key service of this phone speaks.
const MINT_UPGRADE_KEY: u32 = 6;
const MINT_CONVERT_STORAGE_KEY: u32 = 13;
const MINT_GET_KEY_CHARACTERISTICS: u32 = 14;
const MINT_BEGIN: u32 = 10;
const MINT_UPDATE: u32 = 2;
const MINT_FINISH: u32 = 3;
const MINT_ABORT: u32 = 4;

/// The top of a tag says what kind of value it carries.
const TAG_KIND: i32 = -268_435_456;
const KIND_ULONG: i32 = 1_342_177_280;
const KIND_ULONG_REP: i32 = -1_610_612_736;
const KIND_DATE: i32 = 1_610_612_736;
const KIND_BYTES: i32 = -1_879_048_192;
const KIND_BIGNUM: i32 = -2_147_483_648;

/// How long the nonce of an AES-GCM key is here.
const NONCE_BYTES: usize = 12;

/// The key service's way of saying the key was made under an older build of
/// the phone's own firmware and has to be renewed before it will be used.
const KEY_REQUIRES_UPGRADE: i32 = -62;

const PURPOSE_DECRYPT: i32 = 1;
const BLOCK_MODE_GCM: i32 = 32;
const PADDING_NONE: i32 = 1;

/// What a key parameter is tagged with in the newer interface. The high bits
/// say which kind of value follows, which is why the numbers look arbitrary.
const TAG_PURPOSE: i32 = 536_870_913;
const TAG_BLOCK_MODE: i32 = 536_870_916;
const TAG_PADDING: i32 = 536_870_918;
const TAG_MAC_LENGTH: i32 = 805_307_371;
const TAG_APPLICATION_ID: i32 = -1_879_047_591;
const TAG_NONCE: i32 = -1_879_047_191;

/// Which arm of a parameter's value is used in the newer interface.
const VALUE_PADDING_MODE: i32 = 3;
const VALUE_KEY_PURPOSE: i32 = 7;
const VALUE_INTEGER: i32 = 11;
const VALUE_BLOB: i32 = 14;
const VALUE_BLOCK_MODE: i32 = 2;

/// The older interface's method numbers, and the tags it writes its
/// parameters with. Its tags are the Keymaster 4.0 ones, which differ from
/// the newer interface's for the length and the nonce.
const MASTER_GET_HARDWARE_INFO: u32 = 1;

/// How long the phone's key service gets to answer a call. It reaches the
/// secure world, which can be slow to wake, but a service that has not
/// answered by then is one the page should say so about rather than wait
/// on forever.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);
const MASTER_GET_KEY_CHARACTERISTICS: u32 = 9;
const MASTER_BEGIN: u32 = 16;
const MASTER_UPDATE: u32 = 17;
const MASTER_FINISH: u32 = 18;
const MASTER_ABORT: u32 = 19;
const MASTER_PURPOSE_DECRYPT: u64 = 1;
const MASTER_PURPOSE_WRAP_KEY: u64 = 5;
const MASTER_TAG_PURPOSE: u32 = 0x2000_0001;
const MASTER_TAG_BLOCK_MODE: u32 = 0x2000_0004;
const MASTER_TAG_PADDING: u32 = 0x2000_0006;
const MASTER_TAG_MAC_LENGTH: u32 = 0x3000_03EB;
const MASTER_TAG_APPLICATION_ID: u32 = 0x9000_0259;
const MASTER_TAG_NONCE: u32 = 0x9000_03E9;

/// The tag's value and which arm it is written as, in the newer interface.
enum Value<'a> {
    Enum(i32, i32),
    Integer(i32),
    Blob(&'a [u8]),
}

fn parameter(p: &mut Parcel, tag: i32, value: Value) {
    // Each parameter is a structure of its own: it says it is there, then
    // how long it is, so the far side can step over what it does not know.
    p.i32(1);
    let start = p.here();
    p.i32(0);
    p.i32(tag);
    // The value is a structure of its own too, and says so the same way,
    // though it carries no length because only one arm of it is ever set.
    p.i32(1);
    match value {
        Value::Enum(arm, v) => {
            p.i32(arm);
            p.i32(v);
        }
        Value::Integer(v) => {
            p.i32(VALUE_INTEGER);
            p.i32(v);
        }
        Value::Blob(v) => {
            p.i32(VALUE_BLOB);
            p.blob(v);
        }
    }
    let size = (p.here() - start) as i32;
    p.fill(start, size);
}

/// Hand over the note the secure world wrote when the user answered their
/// screen lock. A key guarded by the screen lock is refused without it.
fn vouched(p: &mut Parcel, note: &Token) {
    p.i32(1);
    let start = p.here();
    p.i32(0);
    p.i64(note.challenge);
    p.i64(note.user);
    p.i64(note.authenticator);
    p.i32(note.kind);
    // The time it was written is a structure of its own.
    p.i32(1);
    let when = p.here();
    p.i32(0);
    p.i64(note.when);
    let size = (p.here() - when) as i32;
    p.fill(when, size);
    p.blob(&note.mac);
    let size = (p.here() - start) as i32;
    p.fill(start, size);
}

/// What the phone says about its key hardware.
pub struct Hardware {
    pub version: i32,
    pub security_level: i32,
    pub name: String,
    pub author: String,
}

/// Which interface the phone's key service speaks.
enum Device<'a> {
    /// The newer kind, registered with the service directory.
    Mint(Handle<'a>),
    /// The older kind, a hardware service reached over the other driver.
    Master(Handle<'a>),
}

/// A decryption the key service opened: an object in the newer interface,
/// a number in the older one.
enum Operation<'a> {
    Mint(Handle<'a>),
    Master(u64),
}

/// One key parameter as the older interface writes it: what it is tagged
/// with, the number or choice that goes with it, and the bytes it carries
/// where it carries any.
type Param = (u32, u64, Vec<u8>);

/// The phone's key service, as this program talks to it: the newer interface
/// where the phone has it, the older one where it does not.
pub struct KeyMint<'a> {
    device: Device<'a>,
}

impl<'a> KeyMint<'a> {
    /// Reach the running key service, or say that none is. The newer kind
    /// is reached over the phone's own driver; the older kind is a hardware
    /// service, reached over the second driver, which `hardware` is.
    ///
    /// The phone's own key service takes a while to come up: it has to reach
    /// the secure world and load what it keeps there first. So this waits for
    /// it rather than giving up at once, and the page that asked says it is
    /// waiting while it does.
    pub fn open(binder: &'a Binder, hardware: Option<&'a Binder>) -> Result<KeyMint<'a>> {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(90);
        let mut why = None;
        loop {
            match service::find(binder, SERVICE) {
                Ok(Some(device)) => return Ok(KeyMint { device: Device::Mint(device) }),
                Ok(None) => {}
                Err(e) => why = Some(format!("{e:#}")),
            }
            #[cfg(target_os = "android")]
            if let Some(hardware) = hardware {
                match hidl::find(hardware, MASTER_SERVICE, "default") {
                    Ok(Some(device)) => return Ok(KeyMint { device: Device::Master(device) }),
                    Ok(None) => {}
                    Err(e) => why = Some(format!("{e:#}")),
                }
            }
            if std::time::Instant::now() >= until {
                let why = why.map(|w| format!(": {w}")).unwrap_or_default();
                anyhow::bail!("{SERVICE} and {MASTER_SERVICE} are not running after 90 seconds{why}");
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }

    pub fn hardware(&self) -> Result<Hardware> {
        match &self.device {
            Device::Mint(d) => {
                let reply = d.transact(GET_HARDWARE_INFO, &Parcel::call(DEVICE)).context("ask the key service about itself")?;
                let mut read = reply.read();
                status::ok(&mut read).context("ask the key service about itself")?;
                let end = read.structure()?;
                let info = Hardware {
                    version: read.i32()?,
                    security_level: read.i32()?,
                    name: read.string()?,
                    author: read.string()?,
                };
                read.go(end)?;
                Ok(info)
            }
            Device::Master(d) => {
                let reply = d.transact_within(MASTER_GET_HARDWARE_INFO, &Message::call(MASTER_SERVICE), Some(ANSWER_TIMEOUT)).context("ask the key service about itself")?;
                let mut read = Answer::new(reply.bytes());
                read.status().context("ask the key service about itself")?;
                Ok(Hardware {
                    version: 4,
                    security_level: read.i32()?,
                    name: read.text().context("read the key service's name")?,
                    author: read.text().context("read the key service's author")?,
                })
            }
        }
    }

    /// Turn a key that only the storage hardware can use into the form it
    /// takes for this start of the phone. The key itself never leaves the
    /// secure world: what comes back is wrapped again, for the hardware to
    /// unwrap as it reads and writes.
    pub fn ephemeral_storage_key(&self, key: &[u8]) -> Result<Vec<u8>> {
        match &self.device {
            Device::Mint(_) => match self.convert(key) {
                Err(e) if status::refused_with(&e) == Some(KEY_REQUIRES_UPGRADE) => {
                    // As with a key that decrypts, the key is renewed for the
                    // firmware running now, as Android's keystore2 renews one
                    // for vold, and the one on disk is left as it is. A storage
                    // key is bound to nothing that has to be given again.
                    let renewed = self.upgrade(key, |call| call.i32(0)).context("renew the storage key for the firmware running now")?;
                    self.convert(&renewed)
                }
                converted => converted,
            },
            Device::Master(_) => {
                // The older interface has no call that converts a storage key
                // on its own; wrapping it with itself is what the newer one
                // does behind that call. The operation's output is the key in
                // the form the storage hardware takes for this start.
                let operation = self.master_begin(MASTER_PURPOSE_WRAP_KEY, key, &[], None)?;
                let Operation::Master(handle) = operation else { anyhow::bail!("the key service is not the older kind") };
                self.master_finish(&handle, &[], &[], None)
            }
        }
    }

    fn convert(&self, key: &[u8]) -> Result<Vec<u8>> {
        let Device::Mint(d) = &self.device else { anyhow::bail!("the key service has no storage-key conversion") };
        let mut call = Parcel::call(DEVICE);
        call.blob(key);
        let reply = d.transact(MINT_CONVERT_STORAGE_KEY, &call).context("make a storage key for this start")?;
        let mut read = reply.read();
        status::ok(&mut read).context("make a storage key for this start")?;
        read.blob().context("read the storage key")
    }

    /// What the key service says a key may be used for: each entry is a tag
    /// and the number or length that came with it.
    pub fn authorizations(&self, key_blob: &[u8], app_id: &[u8]) -> Result<Vec<(i32, i64)>> {
        match &self.device {
            Device::Mint(d) => {
                let mut call = Parcel::call(DEVICE);
                call.blob(key_blob);
                call.blob(app_id);
                call.blob(&[]);
                let reply = d.transact(MINT_GET_KEY_CHARACTERISTICS, &call).context("ask what the key may be used for")?;
                let mut read = reply.read();
                status::ok(&mut read).context("ask what the key may be used for")?;
                let mut out = Vec::new();
                for _ in 0..read.i32()?.max(0) {
                    let end = read.structure()?;
                    read.i32().context("read which part of the phone holds the key")?;
                    for _ in 0..read.i32()?.max(0) {
                        let next = read.structure()?;
                        let tag = read.i32()?;
                        read.i32().context("read whether the tag has a value")?;
                        read.i32().context("read which arm of the value is set")?;
                        let value = match tag & TAG_KIND {
                            KIND_ULONG | KIND_ULONG_REP | KIND_DATE => read.i64()?,
                            KIND_BYTES | KIND_BIGNUM => read.blob()?.len() as i64,
                            _ => read.i32()? as i64,
                        };
                        out.push((tag, value));
                        read.go(next)?;
                    }
                    read.go(end)?;
                }
                Ok(out)
            }
            Device::Master(d) => {
                let mut call = Message::call(MASTER_SERVICE);
                call.bytes(key_blob);
                call.bytes(app_id);
                call.bytes(&[]);
                let reply = d.transact_within(MASTER_GET_KEY_CHARACTERISTICS, &call, Some(ANSWER_TIMEOUT)).context("ask what the key may be used for")?;
                let mut read = Answer::new(reply.bytes());
                read.status().context("ask what the key may be used for")?;
                let error = read.i32().context("read whether the key service answered")?;
                ensure!(error == 0, "the key service refused to say what the key may be used for ({error})");
                let (software, hardware) = read.key_characteristics()?;
                let mut out = Vec::new();
                for p in software.into_iter().chain(hardware) {
                    let value = if p.blob.is_empty() { p.value } else { p.blob.len() as i64 };
                    out.push((p.tag as i32, value));
                }
                Ok(out)
            }
        }
    }

    /// Undo an AES-GCM encryption the key service did with `key_blob`. The
    /// service only hands back the plaintext when `app_id` is the one the
    /// key was made for, so a wrong secret fails here rather than yielding
    /// something that looks like a key. A key guarded by a screen lock
    /// takes the note the secure world wrote when it was answered.
    pub fn gcm_decrypt(&self, key_blob: &[u8], app_id: &[u8], nonce: &[u8], body: &[u8], note: Option<&Token>) -> Result<Vec<u8>> {
        match &self.device {
            Device::Mint(_) => {
                let operation = match self.begin(key_blob, app_id, nonce, note) {
                    Ok(operation) => operation,
                    Err(e) if status::refused_with(&e) == Some(KEY_REQUIRES_UPGRADE) => {
                        // The phone's firmware has moved on since this key was
                        // made. Renewing it gives a key for the firmware running
                        // now; the one on disk is left as it is, so nothing of the
                        // phone's own is changed by reading it here.
                        let renewed = self
                            .upgrade(key_blob, |call| Self::gcm_params(call, app_id, nonce))
                            .context("renew the key for the firmware running now")?;
                        self.begin(&renewed, app_id, nonce, note)?
                    }
                    Err(e) => return Err(e),
                };
                let mut out = self.update(&operation, body)?;
                out.extend_from_slice(&self.finish(&operation)?);
                Ok(out)
            }
            Device::Master(_) => {
                // A key the phone bound to the directory it keeps the key in
                // names that binding; one it did not is refused when the
                // binding is named, so both are tried before giving up.
                let bound = Self::master_gcm_params(app_id, nonce);
                let operation = match self.master_begin(MASTER_PURPOSE_DECRYPT, key_blob, &bound, note) {
                    Ok(operation) => operation,
                    Err(why) if !app_id.is_empty() => {
                        let unbound = Self::master_gcm_params(&[], nonce);
                        match self.master_begin(MASTER_PURPOSE_DECRYPT, key_blob, &unbound, note) {
                            Ok(operation) => operation,
                            Err(also) => {
                                return Err(why.context(format!("the key is not bound to the directory it is kept in either: {also:#}, and {}", self.reads(key_blob))));
                            }
                        }
                    }
                    Err(why) => return Err(why.context(self.reads(key_blob))),
                };
                let Operation::Master(handle) = operation else { anyhow::bail!("the key service is not the older kind") };
                let mut out = self.master_update(&handle, body, note)?;
                out.extend_from_slice(&self.master_finish(&handle, &[], &[], note)?);
                Ok(out)
            }
        }
    }

    /// What the key service says of a key at all, which tells a key it cannot
    /// read apart from one it reads and will not use.
    fn reads(&self, key_blob: &[u8]) -> String {
        match self.authorizations(key_blob, &[]) {
            Ok(tags) => format!("the key service reads the key as {tags:?}"),
            Err(e) => format!("the key service will not read the key at all: {e:#}"),
        }
    }

    /// What the key service has to be told to use an AES-GCM key: how it is
    /// used, how long its tag is, what it is bound to, and the nonce it was
    /// encrypted under.
    ///
    /// What a key is bound to is named only where there is something: the
    /// secure world folds the binding into the key it unwraps the key blob
    /// with, and a binding to nothing named is not the same to it as none
    /// named at all, so naming an empty one leaves it unable to unwrap a
    /// key that was made bound to nothing.
    fn gcm_params(call: &mut Parcel, app_id: &[u8], nonce: &[u8]) {
        call.i32(if app_id.is_empty() { 5 } else { 6 });
        parameter(call, TAG_BLOCK_MODE, Value::Enum(VALUE_BLOCK_MODE, BLOCK_MODE_GCM));
        parameter(call, TAG_PADDING, Value::Enum(VALUE_PADDING_MODE, PADDING_NONE));
        parameter(call, TAG_MAC_LENGTH, Value::Integer(128));
        if !app_id.is_empty() {
            parameter(call, TAG_APPLICATION_ID, Value::Blob(app_id));
        }
        parameter(call, TAG_NONCE, Value::Blob(nonce));
        parameter(call, TAG_PURPOSE, Value::Enum(VALUE_KEY_PURPOSE, PURPOSE_DECRYPT));
    }

    fn master_gcm_params(app_id: &[u8], nonce: &[u8]) -> Vec<Param> {
        let mut params = vec![
            (MASTER_TAG_BLOCK_MODE, BLOCK_MODE_GCM as u64, Vec::new()),
            (MASTER_TAG_PADDING, PADDING_NONE as u64, Vec::new()),
            (MASTER_TAG_MAC_LENGTH, 128, Vec::new()),
        ];
        if !app_id.is_empty() {
            params.push((MASTER_TAG_APPLICATION_ID, 0, app_id.to_vec()));
        }
        params.push((MASTER_TAG_NONCE, 0, nonce.to_vec()));
        params.push((MASTER_TAG_PURPOSE, PURPOSE_DECRYPT as u64, Vec::new()));
        params
    }

    /// Open a decryption with the key service and keep hold of it.
    fn begin(&self, key_blob: &[u8], app_id: &[u8], nonce: &[u8], note: Option<&Token>) -> Result<Operation<'a>> {
        let Device::Mint(d) = &self.device else {
            let params = Self::master_gcm_params(app_id, nonce);
            return self.master_begin(MASTER_PURPOSE_DECRYPT, key_blob, &params, note);
        };
        let mut call = Parcel::call(DEVICE);
        call.i32(PURPOSE_DECRYPT);
        call.blob(key_blob);
        Self::gcm_params(&mut call, app_id, nonce);
        // A key guarded by the screen lock takes the note the secure world
        // wrote when the user answered it; one that is not guarded takes
        // nothing, and is refused nothing for carrying a note either.
        match note {
            Some(note) => vouched(&mut call, note),
            None => call.none(),
        }

        // The decryption the service opens outlives the reply that named
        // it, so take a share of it before the reply's room goes back.
        let reply = d.transact(MINT_BEGIN, &call).context("start a decryption")?;
        let mut read = reply.read();
        status::ok(&mut read).context("start a decryption")?;
        read.structure().context("read what the key service answered")?;
        read.i64().context("read the decryption's challenge")?;
        let params = read.i32().context("read how many parameters the decryption came back with")?;
        for _ in 0..params.max(0) {
            let end = read.structure().context("read a parameter the decryption came back with")?;
            read.go(end)?;
        }
        match read.object().context("read the decryption the key service opened")? {
            Some(h) => Ok(Operation::Mint(Handle::hold(reply.binder(), h)?)),
            None => anyhow::bail!("the key service opened no decryption"),
        }
    }

    /// A key for the firmware running now, from one made under an older
    /// build. What it is bound to has to be given again, as `params` write
    /// it, or the service cannot tell it is the right key.
    fn upgrade(&self, key_blob: &[u8], params: impl FnOnce(&mut Parcel)) -> Result<Vec<u8>> {
        let Device::Mint(d) = &self.device else { anyhow::bail!("the key service has no key renewal") };
        let mut call = Parcel::call(DEVICE);
        call.blob(key_blob);
        params(&mut call);
        let reply = d.transact(MINT_UPGRADE_KEY, &call).context("renew the key")?;
        let mut read = reply.read();
        status::ok(&mut read).context("renew the key")?;
        read.blob().context("read the renewed key")
    }

    fn update(&self, operation: &Operation, input: &[u8]) -> Result<Vec<u8>> {
        match operation {
            Operation::Mint(operation) => {
                let mut call = Parcel::call(OPERATION);
                call.blob(input);
                call.none();
                call.none();
                let reply = operation.transact(MINT_UPDATE, &call).context("feed the decryption")?;
                let mut read = reply.read();
                status::ok(&mut read).context("feed the decryption")?;
                read.blob().context("read what the decryption gave back")
            }
            Operation::Master(handle) => self.master_update(handle, input, None),
        }
    }

    fn finish(&self, operation: &Operation) -> Result<Vec<u8>> {
        match operation {
            Operation::Mint(operation) => {
                let mut call = Parcel::call(OPERATION);
                call.opt_blob(None);
                call.opt_blob(None);
                call.none();
                call.none();
                call.opt_blob(None);
                let reply = operation.transact(MINT_FINISH, &call).context("close the decryption")?;
                let mut read = reply.read();
                status::ok(&mut read).context("close the decryption, which is where a wrong key is refused")?;
                read.blob().context("read what the decryption ended with")
            }
            Operation::Master(handle) => self.master_finish(handle, &[], &[], None),
        }
    }

    fn abort(&self, operation: &Operation) -> Result<()> {
        match operation {
            Operation::Mint(operation) => {
                let reply = operation.transact(MINT_ABORT, &Parcel::call(OPERATION)).context("drop the decryption")?;
                status::ok(&mut reply.read()).context("drop the decryption")
            }
            Operation::Master(handle) => self.master_abort(handle),
        }
    }

    /// Whether the secure world will unwrap the key at all: a decryption is
    /// opened on it under a nonce of nothing and dropped again, which reads
    /// nothing and changes nothing. It tells a key the secure world cannot
    /// unwrap, as one made under another build or bound to something not
    /// given back to it is, apart from one that merely decrypts to nothing.
    pub fn usable(&self, key_blob: &[u8], app_id: &[u8]) -> Result<()> {
        let operation = self.begin(key_blob, app_id, &[0; NONCE_BYTES], None)?;
        self.abort(&operation)
    }

    /// The note the secure world wrote for an answered screen lock, as the
    /// older interface hands it over. A call that carries none carries one
    /// of nothing.
    fn note_of(note: Option<&Token>) -> Note<'_> {
        match note {
            Some(note) => Note {
                challenge: note.challenge as u64,
                user: note.user as u64,
                authenticator: note.authenticator as u64,
                kind: note.kind as u32,
                when: note.when as u64,
                seal: note.mac.as_slice(),
            },
            None => Note::default(),
        }
    }

    fn master_begin(&self, purpose: u64, key: &[u8], params: &[Param], note: Option<&Token>) -> Result<Operation<'a>> {
        let Device::Master(d) = &self.device else { anyhow::bail!("the key service is not the older kind") };
        let mut call = Message::call(MASTER_SERVICE);
        call.u32(purpose as u32);
        call.bytes(key);
        call.params(params);
        call.note(Some(&Self::note_of(note)));
        let reply = d.transact_within(MASTER_BEGIN, &call, Some(ANSWER_TIMEOUT)).context("start a decryption")?;
        let mut read = Answer::new(reply.bytes());
        read.status().context("start a decryption")?;
        let error = read.i32().context("read what the key service answered")?;
        ensure!(error == 0, "the key service refused to start the operation ({error})");
        let _ = read.key_params().context("read the parameters the operation came back with")?;
        Ok(Operation::Master(read.u64().context("read the operation the key service opened")?))
    }

    fn master_update(&self, handle: &u64, input: &[u8], note: Option<&Token>) -> Result<Vec<u8>> {
        let Device::Master(d) = &self.device else { anyhow::bail!("the key service is not the older kind") };
        let mut call = Message::call(MASTER_SERVICE);
        call.u64(*handle);
        call.params(&[]);
        call.bytes(input);
        call.note(Some(&Self::note_of(note)));
        call.seen();
        let reply = d.transact_within(MASTER_UPDATE, &call, Some(ANSWER_TIMEOUT)).context("feed the decryption")?;
        let mut read = Answer::new(reply.bytes());
        read.status().context("feed the decryption")?;
        let error = read.i32().context("read what the key service answered")?;
        ensure!(error == 0, "the key service refused the decryption ({error})");
        let _ = read.u32().context("read how much of the input was taken")?;
        let _ = read.key_params().context("read the parameters the decryption came back with")?;
        Ok(read.bytes().context("read what the decryption gave back")?.to_vec())
    }

    fn master_finish(&self, handle: &u64, input: &[u8], signature: &[u8], note: Option<&Token>) -> Result<Vec<u8>> {
        let Device::Master(d) = &self.device else { anyhow::bail!("the key service is not the older kind") };
        let mut call = Message::call(MASTER_SERVICE);
        call.u64(*handle);
        call.params(&[]);
        call.bytes(input);
        call.bytes(signature);
        call.note(Some(&Self::note_of(note)));
        call.seen();
        let reply = d.transact_within(MASTER_FINISH, &call, Some(ANSWER_TIMEOUT)).context("close the decryption")?;
        let mut read = Answer::new(reply.bytes());
        read.status().context("close the decryption")?;
        let error = read.i32().context("read what the key service answered")?;
        ensure!(error == 0, "the key service refused to close the decryption, which is where a wrong key is refused ({error})");
        let _ = read.key_params().context("read the parameters the decryption ended with")?;
        Ok(read.bytes().context("read what the decryption ended with")?.to_vec())
    }

    fn master_abort(&self, handle: &u64) -> Result<()> {
        let Device::Master(d) = &self.device else { anyhow::bail!("the key service is not the older kind") };
        let mut call = Message::call(MASTER_SERVICE);
        call.u64(*handle);
        let reply = d.transact_within(MASTER_ABORT, &call, Some(ANSWER_TIMEOUT)).context("drop the decryption")?;
        let mut read = Answer::new(reply.bytes());
        read.status().context("drop the decryption")?;
        let error = read.i32().context("read what the key service answered")?;
        ensure!(error == 0, "the key service refused to drop the decryption ({error})");
        Ok(())
    }
}
