//! The newer kind of key service, KeyMint: what it is asked, and how its
//! answers are read.
//!
//! The newer layout writes a number where the older one writes four words,
//! and hands everything longer over as a counted run, so a parameter is its
//! tag, which arm of its value is set, and that arm.

use super::{Decryption, Hardware, NONCE_BYTES};
use crate::token::Token;
use anyhow::{bail, Context, Result};
use skyward_binder::driver::Handle;
use skyward_binder::parcel::Parcel;
use skyward_binder::status;

/// The interface of the key itself, and of a decryption it opened.
const DEVICE: &str = "android.hardware.security.keymint.IKeyMintDevice";
const OPERATION: &str = "android.hardware.security.keymint.IKeyMintOperation";

/// How each call this program makes is numbered.
const HARDWARE_INFO: u32 = 1;
const UPDATE: u32 = 2;
const FINISH: u32 = 3;
const ABORT: u32 = 4;
const UPGRADE_KEY: u32 = 6;
const BEGIN: u32 = 10;
const CONVERT_STORAGE_KEY: u32 = 13;
const KEY_CHARACTERISTICS: u32 = 14;

/// The key service's way of saying that a key was made under an older build
/// of the phone's own firmware and has to be renewed before it will be used.
const KEY_REQUIRES_UPGRADE: i32 = -62;

/// The top of a tag says what kind of value carries it.
const TAG_KIND: i32 = -268_435_456;
const KIND_ULONG: i32 = 1_342_177_280;
const KIND_ULONG_REP: i32 = -1_610_612_736;
const KIND_DATE: i32 = 1_610_612_736;
const KIND_BYTES: i32 = -1_879_048_192;
const KIND_BIGNUM: i32 = -2_147_483_648;

/// What a parameter is tagged with. The numbers look arbitrary because the
/// high bits say which kind of value follows.
const TAG_PURPOSE: i32 = 536_870_913;
const TAG_BLOCK_MODE: i32 = 536_870_916;
const TAG_PADDING: i32 = 536_870_918;
const TAG_MAC_LENGTH: i32 = 805_307_371;
const TAG_APPLICATION_ID: i32 = -1_879_047_591;
const TAG_NONCE: i32 = -1_879_047_191;

/// Which arm of a parameter's value is set.
const ARM_BLOCK_MODE: i32 = 2;
const ARM_PADDING_MODE: i32 = 3;
const ARM_KEY_PURPOSE: i32 = 7;
const ARM_INTEGER: i32 = 11;
const ARM_BYTES: i32 = 14;

/// The numbers three of those tags take.
const PURPOSE_DECRYPT: i32 = 1;
const BLOCK_MODE_GCM: i32 = 32;
const PADDING_NONE: i32 = 1;

/// One parameter as it is written: its tag, which arm is set, and the arm.
enum Value<'a> {
    Choice(i32, i32),
    Number(i32),
    Bytes(&'a [u8]),
}

pub(super) fn hardware(device: &Handle) -> Result<Hardware> {
    let what = "ask the key service about itself";
    let reply = device.transact(HARDWARE_INFO, &Parcel::call(DEVICE)).context(what)?;
    let mut read = reply.read();
    status::ok(&mut read).context(what)?;
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

pub(super) fn authorizations(device: &Handle, key: &[u8], app_id: &[u8]) -> Result<Vec<(i32, i64)>> {
    let what = "ask what the key may be used for";
    let mut call = Parcel::call(DEVICE);
    call.blob(key);
    call.blob(app_id);
    call.blob(&[]);
    let reply = device.transact(KEY_CHARACTERISTICS, &call).context(what)?;
    let mut read = reply.read();
    status::ok(&mut read).context(what)?;
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

pub(super) fn gcm_decrypt(device: &Handle, want: &Decryption) -> Result<Vec<u8>> {
    let operation = match open_for_decryption(device, want) {
        Ok(operation) => operation,
        Err(e) if status::refused_with(&e) == Some(KEY_REQUIRES_UPGRADE) => {
            // The phone's firmware has moved on since this key was made.
            // Renewing it gives a key for the firmware running now; the one on
            // disk is left as it is, so nothing of the phone's own is changed
            // by reading it here.
            let renewed = upgrade(device, want.key, |call| gcm_params(call, want.app_id, want.nonce))
                .context("renew the key for the firmware running now")?;
            open_for_decryption(device, &Decryption { key: &renewed, ..*want })?
        }
        Err(e) => return Err(e),
    };
    let mut out = update(&operation, want.body)?;
    out.extend_from_slice(&finish(&operation)?);
    Ok(out)
}

pub(super) fn usable(device: &Handle, key: &[u8], app_id: &[u8]) -> Result<()> {
    let want = Decryption { key, app_id, nonce: &[0; NONCE_BYTES], body: &[], note: None };
    let operation = open_for_decryption(device, &want)?;
    abort(&operation)
}

/// Turn a key into the form the storage hardware takes for this start. The
/// key service wraps it again under the key it holds in hardware, and what
/// comes back is what the hardware unwraps as it reads and writes.
pub(super) fn ephemeral_storage_key(device: &Handle, key: &[u8]) -> Result<Vec<u8>> {
    match convert(device, key) {
        Err(e) if status::refused_with(&e) == Some(KEY_REQUIRES_UPGRADE) => {
            // As with a key that decrypts, the key is renewed for the firmware
            // running now, and the one on disk is left as it is. A storage key
            // is bound to nothing that has to be given again.
            let renewed = upgrade(device, key, |call| call.i32(0)).context("renew the storage key for the firmware running now")?;
            convert(device, &renewed)
        }
        made => made,
    }
}

fn convert(device: &Handle, key: &[u8]) -> Result<Vec<u8>> {
    let what = "make a storage key for this start";
    let mut call = Parcel::call(DEVICE);
    call.blob(key);
    let reply = device.transact(CONVERT_STORAGE_KEY, &call).context(what)?;
    let mut read = reply.read();
    status::ok(&mut read).context(what)?;
    read.blob().context("read the storage key")
}

/// A key for the firmware running now, from one made under an older build.
fn upgrade(device: &Handle, key: &[u8], params: impl FnOnce(&mut Parcel)) -> Result<Vec<u8>> {
    let what = "renew the key";
    let mut call = Parcel::call(DEVICE);
    call.blob(key);
    params(&mut call);
    let reply = device.transact(UPGRADE_KEY, &call).context(what)?;
    let mut read = reply.read();
    status::ok(&mut read).context(what)?;
    read.blob().context("read the renewed key")
}

/// Open a decryption with the key service and keep hold of it, because it
/// outlives the reply that named it. What comes back is the decryption
/// itself, which is an object of the key service as the key is.
fn open_for_decryption<'a>(device: &Handle<'a>, want: &Decryption) -> Result<Handle<'a>> {
    let mut call = Parcel::call(DEVICE);
    call.i32(PURPOSE_DECRYPT);
    call.blob(want.key);
    gcm_params(&mut call, want.app_id, want.nonce);
    // A key guarded by the screen lock takes the note the secure world wrote
    // when the user answered it; one that is not guarded takes nothing, and is
    // refused nothing for carrying a note either.
    match want.note {
        Some(note) => vouched(&mut call, note),
        None => call.none(),
    }
    let what = "start a decryption";
    let reply = device.transact(BEGIN, &call).context(what)?;
    let mut read = reply.read();
    status::ok(&mut read).context(what)?;
    read.structure().context("read what the key service answered")?;
    read.i64().context("read the decryption's challenge")?;
    for _ in 0..read.i32().context("read how many parameters the decryption came back with")?.max(0) {
        let end = read.structure().context("read a parameter the decryption came back with")?;
        read.go(end)?;
    }
    match read.object().context("read the decryption the key service opened")? {
        Some(handle) => Ok(Handle::hold(reply.binder(), handle)?),
        None => bail!("the key service opened no decryption"),
    }
}

fn update(operation: &Handle<'_>, input: &[u8]) -> Result<Vec<u8>> {
    let what = "feed the decryption";
    let mut call = Parcel::call(OPERATION);
    call.blob(input);
    call.none();
    call.none();
    let reply = operation.transact(UPDATE, &call).context(what)?;
    let mut read = reply.read();
    status::ok(&mut read).context(what)?;
    read.blob().context("read what the decryption gave back")
}

fn finish(operation: &Handle<'_>) -> Result<Vec<u8>> {
    let what = "close the decryption, which is where a wrong key is refused";
    let mut call = Parcel::call(OPERATION);
    call.opt_blob(None);
    call.opt_blob(None);
    call.none();
    call.none();
    call.opt_blob(None);
    let reply = operation.transact(FINISH, &call).context(what)?;
    let mut read = reply.read();
    status::ok(&mut read).context(what)?;
    read.blob().context("read what the decryption ended with")
}

fn abort(operation: &Handle<'_>) -> Result<()> {
    let what = "drop the decryption";
    let reply = operation.transact(ABORT, &Parcel::call(OPERATION)).context(what)?;
    let mut read = reply.read();
    status::ok(&mut read).context(what)
}

/// What the key service has to be told to use an AES-GCM key: how it is
/// used, how long its seal is, what it is bound to, and the nonce it was
/// used under.
///
/// What a key is bound to is named only where there is something: the secure
/// world folds the binding into the key it unwraps the key blob with, and a
/// binding to nothing named is not the same to it as none named at all, so
/// naming an empty one leaves it unable to unwrap a key that was made bound
/// to nothing.
fn gcm_params(call: &mut Parcel, app_id: &[u8], nonce: &[u8]) {
    call.i32(if app_id.is_empty() { 5 } else { 6 });
    parameter(call, TAG_BLOCK_MODE, Value::Choice(ARM_BLOCK_MODE, BLOCK_MODE_GCM));
    parameter(call, TAG_PADDING, Value::Choice(ARM_PADDING_MODE, PADDING_NONE));
    parameter(call, TAG_MAC_LENGTH, Value::Number(128));
    if !app_id.is_empty() {
        parameter(call, TAG_APPLICATION_ID, Value::Bytes(app_id));
    }
    parameter(call, TAG_NONCE, Value::Bytes(nonce));
    parameter(call, TAG_PURPOSE, Value::Choice(ARM_KEY_PURPOSE, PURPOSE_DECRYPT));
}

fn parameter(call: &mut Parcel, tag: i32, value: Value) {
    // Each parameter is a structure of its own: it says it is there, then how
    // long it is, so the far side can step over what it does not know.
    call.i32(1);
    let start = call.here();
    call.i32(0);
    call.i32(tag);
    // The value is a structure of its own too, and says so the same way,
    // though it carries no length because only one arm of it is ever set.
    call.i32(1);
    match value {
        Value::Choice(arm, v) => {
            call.i32(arm);
            call.i32(v);
        }
        Value::Number(v) => {
            call.i32(ARM_INTEGER);
            call.i32(v);
        }
        Value::Bytes(v) => {
            call.i32(ARM_BYTES);
            call.blob(v);
        }
    }
    let size = (call.here() - start) as i32;
    call.fill(start, size);
}

/// Hand over the note the secure world wrote when the user answered their
/// screen lock. A key guarded by the screen lock is refused without it.
fn vouched(call: &mut Parcel, note: &Token) {
    call.i32(1);
    let start = call.here();
    call.i32(0);
    call.i64(note.challenge);
    call.i64(note.user);
    call.i64(note.authenticator);
    call.i32(note.kind);
    // The time it was written is a structure of its own.
    call.i32(1);
    let when = call.here();
    call.i32(0);
    call.i64(note.when);
    let size = (call.here() - when) as i32;
    call.fill(when, size);
    call.blob(&note.mac);
    let size = (call.here() - start) as i32;
    call.fill(start, size);
}
