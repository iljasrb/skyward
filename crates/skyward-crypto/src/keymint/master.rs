//! The older kind of key service, Keymaster 4.0: what it is asked, and how
//! its answers are read.
//!
//! This one is a hardware service, so everything it is handed is handed over
//! as a [Handed] thing: the driver copies it into the service's own room and
//! fills in the pointers in it. A key parameter is therefore a record of its
//! tag, the one word that goes with it, and where its bytes are; a note the
//! secure world wrote is a record with its seal at the end.

use super::{Decryption, Hardware, MASTER_SERVICE, TIMEOUT};
use crate::token::Token;
use anyhow::{bail, ensure, Context, Result};
use skyward_binder::driver::Handle;
use skyward_binder::hidl::{Answer, Handed, Message};

/// How each call this program makes is numbered.
const HARDWARE_INFO: u32 = 1;
const KEY_CHARACTERISTICS: u32 = 9;
const BEGIN: u32 = 16;
const UPDATE: u32 = 17;
const FINISH: u32 = 18;
const ABORT: u32 = 19;

/// What a parameter is tagged with. These are the Keymaster 4.0 numbers,
/// which differ from the older interface's for the length and the nonce.
const TAG_PURPOSE: u32 = 0x2000_0001;
const TAG_BLOCK_MODE: u32 = 0x2000_0004;
const TAG_PADDING: u32 = 0x2000_0006;
const TAG_MAC_LENGTH: u32 = 0x3000_03EB;
const TAG_APPLICATION_ID: u32 = 0x9000_0259;
const TAG_NONCE: u32 = 0x9000_03E9;

/// The numbers three of those tags take.
const PURPOSE_DECRYPT: u64 = 1;
const BLOCK_MODE_GCM: u64 = 32;
const PADDING_NONE: u64 = 1;
const SEAL_BITS: u64 = 128;

/// A key parameter: what it is tagged with, the word that goes with it, and
/// the bytes it carries where it carries any. A tag either names a number or
/// a choice from a list, and either way the one word is where it goes.
struct Param {
    tag: u32,
    value: u64,
    bytes: Vec<u8>,
}

/// How long a parameter is, and where in it its bytes are. The tag is the
/// first word and the number that goes with it the second, so the third and
/// fourth words are the count-and-pointer structure of its bytes.
const PARAM_BYTES: usize = 32;
const PARAM_BYTES_AT: usize = 16;

/// How long the note the secure world wrote for an answered screen lock is,
/// and where in it its seal lies.
const NOTE_BYTES: usize = 56;
const NOTE_SEAL: usize = 40;

/// How long the note saying what the secure world saw of a challenge is, and
/// where in it the list of what it checked and its seal lie. The challenge,
/// the time and the security level take five words before the list.
const SEEN_BYTES: usize = 56;
const SEEN_CHECKED: usize = 16;
const SEEN_SEAL: usize = 40;

pub(super) fn hardware(device: &Handle) -> Result<Hardware> {
    let what = "ask the key service about itself";
    let reply = device.transact_within(HARDWARE_INFO, &Message::call(MASTER_SERVICE), Some(TIMEOUT)).context(what)?;
    let mut read = Answer::new(reply.bytes());
    read.status().context(what)?;
    // The older interface says no error code of its own here: the security
    // level it serves the key at comes first.
    Ok(Hardware {
        version: 4,
        security_level: read.i32().context("read which part of the phone holds the key")?,
        name: read.text().context("read the key service's name")?,
        author: read.text().context("read the key service's author")?,
    })
}

pub(super) fn authorizations(device: &Handle, key: &[u8], app_id: &[u8]) -> Result<Vec<(i32, i64)>> {
    let what = "ask what the key may be used for";
    let mut call = Message::call(MASTER_SERVICE);
    call.bytes(key);
    call.bytes(app_id);
    call.bytes(&[]);
    let reply = device.transact_within(KEY_CHARACTERISTICS, &call, Some(TIMEOUT)).context(what)?;
    let mut read = Answer::new(reply.bytes());
    read.status().context(what)?;
    let error = read.i32().context("read whether the key service answered")?;
    ensure!(error == 0, "the key service refused to say what the key may be used for ({error})");
    let (software, hardware) = characteristics(&mut read)?;
    Ok(software
        .into_iter()
        .chain(hardware)
        .map(|p| (p.tag as i32, if p.bytes.is_empty() { p.value as i64 } else { p.bytes.len() as i64 }))
        .collect())
}

pub(super) fn gcm_decrypt(device: &Handle, want: &Decryption) -> Result<Vec<u8>> {
    let params = decrypt_params(want.app_id, want.nonce);
    let handle = begin(device, want.key, &params, want.note)?;
    let mut out = match update(device, handle, want.body, want.note) {
        Ok(out) => out,
        Err(e) => {
            let _ = abort(device, handle);
            return Err(e);
        }
    };
    out.extend_from_slice(&finish(device, handle, want.note)?);
    Ok(out)
}

pub(super) fn usable(device: &Handle, key: &[u8], app_id: &[u8]) -> Result<()> {
    let handle = begin(device, key, &decrypt_params(app_id, &[0; super::NONCE_BYTES]), None)?;
    abort(device, handle)
}

/// A key the storage hardware is to unwrap itself cannot be made through
/// this kind of key service: the call that makes one belongs to the newer
/// kind, which hands the key back wrapped for the hardware instead of as the
/// key itself.
pub(super) fn ephemeral_storage_key(_: &Handle) -> Result<Vec<u8>> {
    bail!(
        "the phone's key service is the older kind, which has no call that makes a key for the \
         storage hardware to unwrap; the key it holds cannot be given to the hardware again"
    )
}

/// What the key service has to be told to use an AES-GCM key: how it is
/// used, how long its seal is, what it is bound to, and the nonce it was
/// used under.
///
/// What a key is bound to is named only where there is something: the secure
/// world folds the binding into the key it unwraps the key blob with, and a
/// binding to nothing named is not the same to it as none named at all.
fn decrypt_params(app_id: &[u8], nonce: &[u8]) -> Vec<Param> {
    let mut params = vec![
        Param { tag: TAG_BLOCK_MODE, value: BLOCK_MODE_GCM, bytes: Vec::new() },
        Param { tag: TAG_PADDING, value: PADDING_NONE, bytes: Vec::new() },
        Param { tag: TAG_MAC_LENGTH, value: SEAL_BITS, bytes: Vec::new() },
    ];
    if !app_id.is_empty() {
        params.push(Param { tag: TAG_APPLICATION_ID, value: 0, bytes: app_id.to_vec() });
    }
    params.push(Param { tag: TAG_NONCE, value: 0, bytes: nonce.to_vec() });
    params.push(Param { tag: TAG_PURPOSE, value: PURPOSE_DECRYPT, bytes: Vec::new() });
    params
}

/// Open a decryption with the key service and keep hold of it. What comes
/// back is the handle it knows the decryption by.
fn begin(device: &Handle, key: &[u8], params: &[Param], note: Option<&Token>) -> Result<u64> {
    let what = "start a decryption";
    let mut call = Message::call(MASTER_SERVICE);
    call.u32(PURPOSE_DECRYPT as u32);
    call.bytes(key);
    call.hand(&hand_params(params));
    call.hand(&hand_note(note));
    let reply = device.transact_within(BEGIN, &call, Some(TIMEOUT)).context(what)?;
    let mut read = Answer::new(reply.bytes());
    read.status().context(what)?;
    let error = read.i32().context("read what the key service answered")?;
    ensure!(error == 0, "the key service refused to start the operation ({error})");
    let _ = read_params(&mut read).context("read the parameters the operation came back with")?;
    read.u64().context("read the operation the key service opened")
}

fn update(device: &Handle, handle: u64, input: &[u8], note: Option<&Token>) -> Result<Vec<u8>> {
    let what = "feed the decryption";
    let mut call = Message::call(MASTER_SERVICE);
    call.u64(handle);
    call.hand(&hand_params(&[]));
    call.bytes(input);
    call.hand(&hand_note(note));
    call.hand(&hand_seen());
    let reply = device.transact_within(UPDATE, &call, Some(TIMEOUT)).context(what)?;
    let mut read = Answer::new(reply.bytes());
    read.status().context(what)?;
    let error = read.i32().context("read what the key service answered")?;
    ensure!(error == 0, "the key service refused the decryption ({error})");
    let _ = read.u32().context("read how much of the input was taken")?;
    let _ = read_params(&mut read).context("read the parameters the decryption came back with")?;
    Ok(read.bytes().context("read what the decryption gave back")?.to_vec())
}

fn finish(device: &Handle, handle: u64, note: Option<&Token>) -> Result<Vec<u8>> {
    let what = "close the decryption, which is where a wrong key is refused";
    let mut call = Message::call(MASTER_SERVICE);
    call.u64(handle);
    call.hand(&hand_params(&[]));
    call.bytes(&[]);
    call.bytes(&[]);
    call.hand(&hand_note(note));
    call.hand(&hand_seen());
    let reply = device.transact_within(FINISH, &call, Some(TIMEOUT)).context(what)?;
    let mut read = Answer::new(reply.bytes());
    read.status().context(what)?;
    let error = read.i32().context("read what the key service answered")?;
    ensure!(error == 0, "the key service refused to close the decryption ({error})");
    let _ = read_params(&mut read).context("read the parameters the decryption ended with")?;
    Ok(read.bytes().context("read what the decryption ended with")?.to_vec())
}

fn abort(device: &Handle, handle: u64) -> Result<()> {
    let what = "drop the decryption";
    let mut call = Message::call(MASTER_SERVICE);
    call.u64(handle);
    let reply = device.transact_within(ABORT, &call, Some(TIMEOUT)).context(what)?;
    let mut read = Answer::new(reply.bytes());
    read.status().context(what)?;
    let error = read.i32().context("read what the key service answered")?;
    ensure!(error == 0, "the key service refused to drop the decryption ({error})");
    Ok(())
}

/// A list of key parameters as it is handed over: the counted run of records,
/// with each record's bytes hanging off it. The record holding the tag and
/// the number is written whole; only the place of its bytes is left for the
/// driver to fill in.
fn hand_params(params: &[Param]) -> Handed {
    let mut records = vec![0u8; params.len() * PARAM_BYTES];
    for (at, param) in (0..).zip(params).map(|(i, p)| (i * PARAM_BYTES, p)) {
        records[at..at + 4].copy_from_slice(&param.tag.to_le_bytes());
        records[at + 8..at + 16].copy_from_slice(&param.value.to_le_bytes());
        records[at + PARAM_BYTES_AT + 8..at + PARAM_BYTES].copy_from_slice(&(param.bytes.len() as u64).to_le_bytes());
    }
    let mut list = Handed::of(records);
    for (i, param) in params.iter().enumerate() {
        list = list.leading_to(i * PARAM_BYTES + PARAM_BYTES_AT, Handed::of(param.bytes.clone()));
    }
    Handed::run(params.len() as u64, list)
}

/// A list of key parameters as the key service answers it: the counted run of
/// records, then one buffer for each record's bytes, in the same order.
fn read_params(read: &mut Answer) -> Result<Vec<Param>> {
    let (records, count) = read.run().context("read where the parameters are")?;
    if (count as usize) * PARAM_BYTES > records.len() {
        bail!("the key service answered {count} parameters in {} bytes", records.len());
    }
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let at = i * PARAM_BYTES;
        let bytes = read.part()?.unwrap_or(&[]).to_vec();
        out.push(Param {
            tag: u32::from_le_bytes(records[at..at + 4].try_into().expect("four bytes")),
            value: u64::from_le_bytes(records[at + 8..at + 16].try_into().expect("eight bytes")),
            bytes,
        });
    }
    Ok(out)
}

/// What a key may be used for: the parameters the software keeps to and the
/// ones the hardware does. The two lists are held in one structure, with
/// their own records after it.
fn characteristics(read: &mut Answer) -> Result<(Vec<Param>, Vec<Param>)> {
    let Some(whole) = read.part().context("read what the key may be used for")? else {
        return Ok((Vec::new(), Vec::new()));
    };
    if whole.len() < 2 * skyward_binder::hidl::RUN_BYTES {
        bail!("the key service answered {} bytes about the key", whole.len());
    }
    let mut lists = Vec::with_capacity(2);
    for half in 0..2 {
        let at = half * skyward_binder::hidl::RUN_BYTES;
        let count = u64::from_le_bytes(whole[at + 8..at + 16].try_into().expect("eight bytes"));
        let Some(records) = read.part().context("read where the parameters are")? else {
            lists.push(Vec::new());
            continue;
        };
        let mut params = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let at = i * PARAM_BYTES;
            if at + PARAM_BYTES > records.len() {
                bail!("the key service answered {count} parameters in {} bytes", records.len());
            }
            let bytes = read.part()?.unwrap_or(&[]).to_vec();
            params.push(Param {
                tag: u32::from_le_bytes(records[at..at + 4].try_into().expect("four bytes")),
                value: u64::from_le_bytes(records[at + 8..at + 16].try_into().expect("eight bytes")),
                bytes,
            });
        }
        lists.push(params);
    }
    let hardware = lists.pop().unwrap_or_default();
    let software = lists.pop().unwrap_or_default();
    Ok((software, hardware))
}

/// The note the secure world wrote for an answered screen lock. A call that
/// carries none carries a note of nothing, which is what a call that changes
/// nothing is given.
fn hand_note(note: Option<&Token>) -> Handed {
    let seal = note.map_or(&[][..], |n| n.mac.as_slice());
    let mut bytes = vec![0u8; NOTE_BYTES];
    if let Some(note) = note {
        bytes[0..8].copy_from_slice(&(note.challenge as u64).to_le_bytes());
        bytes[8..16].copy_from_slice(&(note.user as u64).to_le_bytes());
        bytes[16..24].copy_from_slice(&(note.authenticator as u64).to_le_bytes());
        bytes[24..28].copy_from_slice(&(note.kind as u32).to_le_bytes());
        bytes[32..40].copy_from_slice(&(note.when as u64).to_le_bytes());
    }
    bytes[NOTE_SEAL + 8..NOTE_BYTES].copy_from_slice(&(seal.len() as u64).to_le_bytes());
    Handed::of(bytes).leading_to(NOTE_SEAL, Handed::of(seal))
}

/// The note saying what the secure world saw of a challenge: which challenge,
/// when, what it checked, and the seal over all of it. Nothing seen is a note
/// of nothing.
fn hand_seen() -> Handed {
    Handed::of(vec![0u8; SEEN_BYTES])
        .leading_to(SEEN_CHECKED, Handed::of(Vec::new()))
        .leading_to(SEEN_SEAL, Handed::of(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use skyward_binder::parcel::Payload;

    /// The buffers a handed-over thing is written as, as the driver reads
    /// them: where each pointer lies, how many bytes it leads to, and the
    /// bytes themselves.
    fn written(whole: &Handed) -> Vec<(u64, usize, Vec<u8>)> {
        let mut m = Message::call(MASTER_SERVICE);
        m.hand(whole);
        m.addresses()
            .iter()
            .map(|at| {
                let raw = &m.values()[*at as usize..*at as usize + 40];
                let points = u64::from_le_bytes(raw[8..16].try_into().unwrap());
                let length = u64::from_le_bytes(raw[16..24].try_into().unwrap()) as usize;
                let pointer_at = u64::from_le_bytes(raw[32..40].try_into().unwrap());
                (pointer_at, length, unsafe { std::slice::from_raw_parts(points as *const u8, length) }.to_vec())
            })
            .collect()
    }

    #[test]
    fn a_list_of_parameters_is_a_counted_run_with_their_bytes_hanging_off_it() {
        let params = decrypt_params(b"bound", b"nonce");
        let parts = written(&hand_params(&params));
        assert_eq!(parts.len(), 1 + 1 + 2, "the run, the records, and the two lots of bytes");
        assert_eq!(parts[1].1, params.len() * PARAM_BYTES, "one record for each parameter");
        let records = &parts[1].2;
        assert_eq!(u32::from_le_bytes(records[0..4].try_into().unwrap()), TAG_BLOCK_MODE);
        assert_eq!(u64::from_le_bytes(records[8..16].try_into().unwrap()), BLOCK_MODE_GCM, "the choice sits in the word that goes with the tag");
        let at: Vec<u64> = (2..4).map(|i| parts[i].0).collect();
        let expected = vec![
            (3 * PARAM_BYTES + PARAM_BYTES_AT) as u64,
            (4 * PARAM_BYTES + PARAM_BYTES_AT) as u64,
        ];
        assert_eq!(at, expected, "each pointer lies where its own parameter's bytes go");
        assert_eq!(parts[3].2, b"nonce");
    }

    #[test]
    fn a_key_bound_to_nothing_is_told_so_by_saying_nothing() {
        assert!(decrypt_params(b"", b"nonce").iter().all(|p| p.tag != TAG_APPLICATION_ID));
        assert!(decrypt_params(b"bound", b"nonce").iter().any(|p| p.tag == TAG_APPLICATION_ID && p.bytes == b"bound"));
    }

    #[test]
    fn a_note_is_one_structure_with_its_seal_hanging_off_it() {
        let note = Token { challenge: 1, user: 2, authenticator: 3, kind: 4, when: 5, mac: vec![9; 32] };
        let parts = written(&hand_note(Some(&note)));
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].1, NOTE_BYTES);
        assert_eq!(u64::from_le_bytes(parts[0].2[0..8].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(parts[0].2[24..28].try_into().unwrap()), 4);
        assert_eq!(u64::from_le_bytes(parts[0].2[32..40].try_into().unwrap()), 5, "when is a whole word, past the choice");
        assert_eq!(parts[1].0, NOTE_SEAL as u64, "the seal is pointed at from its own place in the note");
        assert_eq!(parts[1].2, vec![9u8; 32]);
    }

    #[test]
    fn a_note_of_nothing_is_still_a_note() {
        let parts = written(&hand_note(None));
        assert_eq!(parts.len(), 1, "the seal of nothing is written but not named");
        assert_eq!(parts[0].2, vec![0u8; NOTE_BYTES]);
    }

    #[test]
    fn the_note_of_what_was_seen_holds_its_places_for_the_list_and_the_seal() {
        let parts = written(&hand_seen());
        assert_eq!(parts.len(), 1, "a list of nothing and a seal of nothing are written but not named");
        assert_eq!(parts[0].2, vec![0u8; SEEN_BYTES]);
    }
}
