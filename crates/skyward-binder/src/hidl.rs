//! The older layout, which the phone's hardware services still speak, as the
//! phone's own libraries write it.
//!
//! A call in it opens with the name of the interface it is made on, written
//! as plain bytes with a terminator and padded to four. Everything after it
//! is a run of plain numbers, except for the things that are longer than a
//! number: those are handed over as *buffer objects*. A buffer object is
//! forty bytes saying where some bytes are, how many there are, whether they
//! sit inside something already handed over, and where in it the pointer to
//! them lies. The driver copies what they point at into the far side's own
//! room, rewrites the pointers that lead to it, and then the far side reads
//! the numbers and the copies.
//!
//! That last part is the whole point of the parent fields: the far side
//! never sees a pointer written by this side. Text is handed over as a
//! sixteen-byte count-and-pointer structure with the pointer inside it
//! rewritten to the copy of the text; a list of key parameters is handed
//! over as an array whose every element points at its own bytes the same
//! way.
//!
//! The numbers every field here is written with were read off the phone's
//! own `libhidlbase.so` and `android.hardware.keymaster@4.0.so`, so they
//! match what the phone's services were built to read.

use crate::parcel::Payload;
use anyhow::{bail, Context, Result};

/// A buffer object: the bytes it describes are copied, and whoever holds it
/// reads them where they land.
const BUFFER: u32 = u32::from_be_bytes([b'p', b't', b'*', 0x85]);

/// An object in the sending process, as a transaction carries it.
const OBJECT: u32 = u32::from_be_bytes([b's', b'h', b'*', 0x85]);

/// The bytes this buffer describes sit inside the one named by `parent`.
const HAS_PARENT: u32 = 1;

/// How much room a buffer object takes among the values.
const BUFFER_BYTES: usize = 40;

/// How much room an object takes among the values. The newer layout adds a
/// word saying how far the object's interface travels; this one does not.
const OBJECT_BYTES: usize = 24;

/// The structure a run of bytes is handed over as: where the bytes are, and
/// how many of them there are.
const RUN_BYTES: usize = 16;

/// What the driver copies for a buffer is rounded up to a whole word.
const WORD: u64 = 8;

/// Where in a key parameter its bytes are. The tag is the first word and the
/// number that goes with it the second, so the third and fourth words are
/// the count-and-pointer structure of the bytes.
const PARAM_BYTES: usize = 32;
const PARAM_BLOB: usize = 16;

/// Where in the note the secure world wrote for an answered screen lock its
/// seal is, and how long the note is.
const NOTE_BYTES: usize = 56;
const NOTE_SEAL: usize = 40;

/// Where in the note saying what the secure world saw of a challenge its
/// list of what it checked and its seal are, and how long that note is. The
/// challenge, the time and the security level take five words before the
/// list, and the list takes two before the seal.
const SEEN_BYTES: usize = 56;
const SEEN_VERIFIED: usize = 16;
const SEEN_SEAL: usize = 40;

/// A call being written. What the buffer objects point at is kept here too,
/// so it stays where it is until the driver has read it.
#[derive(Default)]
pub struct Message {
    data: Vec<u8>,
    /// Where each buffer object lies among the values, in the order written.
    objects: Vec<u64>,
    held: Vec<Box<[u8]>>,
    pointed: u64,
}

impl Message {
    /// A call to `interface`, opened with the name the far side checks
    /// before it looks at anything the method itself takes.
    pub fn call(interface: &str) -> Message {
        let mut m = Message::default();
        m.data.extend_from_slice(interface.as_bytes());
        m.data.push(0);
        while !m.data.len().is_multiple_of(4) {
            m.data.push(0);
        }
        m
    }

    pub fn u32(&mut self, v: u32) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.u32(v as u32);
    }

    pub fn u64(&mut self, v: u64) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    /// A run of bytes handed over as a list, or as text. Both are a
    /// sixteen-byte structure saying where the bytes are and how many there
    /// are, then the bytes. Text counts its characters without the
    /// terminator the far side is given.
    pub fn bytes(&mut self, value: &[u8]) {
        let at = self.keep(value.to_vec());
        self.run(at, value.len() as u64, value.len());
    }

    pub fn text(&mut self, value: &str) {
        let mut raw = value.as_bytes().to_vec();
        raw.push(0);
        let at = self.keep(raw);
        self.run(at, value.len() as u64, value.len() + 1);
    }

    /// A list of key parameters: what each is tagged with, the number that
    /// goes with it, and the bytes it carries where it carries any. The
    /// number is a whole word, whether what it holds is a number or a choice
    /// from a list of them; the key service reads it as one or the other by
    /// the tag.
    pub fn params(&mut self, params: &[(u32, u64, Vec<u8>)]) {
        let mut array = Vec::with_capacity(params.len() * PARAM_BYTES);
        let mut blobs = Vec::with_capacity(params.len());
        for (tag, value, blob) in params {
            let mut raw = vec![0u8; PARAM_BYTES];
            raw[0..4].copy_from_slice(&tag.to_le_bytes());
            raw[8..16].copy_from_slice(&value.to_le_bytes());
            let at = self.keep(blob.clone());
            raw[PARAM_BLOB..PARAM_BLOB + 8].copy_from_slice(&at.to_le_bytes());
            raw[PARAM_BLOB + 8..PARAM_BLOB + 16].copy_from_slice(&(blob.len() as u64).to_le_bytes());
            array.extend_from_slice(&raw);
            blobs.push((at, blob.len()));
        }
        let at = self.keep(array.clone());
        let mut frame = Vec::with_capacity(RUN_BYTES);
        frame.extend_from_slice(&at.to_le_bytes());
        frame.extend_from_slice(&(blobs.len() as u64).to_le_bytes());
        let frame_at = self.keep(frame);
        let held = self.buffer(frame_at, RUN_BYTES, None, 0);
        let held = self.buffer(at, array.len(), held, 0);
        for (i, (at, length)) in blobs.into_iter().enumerate() {
            // The far side finds the bytes by where in the array it already
            // holds the pointer to them lies. The driver only lets the
            // pointers in one buffer be filled in in the order they lie in
            // it, so each says where its own place is.
            self.buffer(at, length, held, i * PARAM_BYTES + PARAM_BLOB);
        }
    }

    /// A note the secure world wrote for an answered screen lock: the
    /// challenge it answered, whose it was, what it was answered with, when,
    /// and the seal over all of it. A call that carries no note carries one
    /// of nothing.
    pub fn note(&mut self, note: Option<&Note>) {
        let mut raw = vec![0u8; NOTE_BYTES];
        let seal = note.map_or(&[][..], |n| n.seal);
        if let Some(note) = note {
            raw[0..8].copy_from_slice(&note.challenge.to_le_bytes());
            raw[8..16].copy_from_slice(&note.user.to_le_bytes());
            raw[16..24].copy_from_slice(&note.authenticator.to_le_bytes());
            raw[24..28].copy_from_slice(&note.kind.to_le_bytes());
            raw[32..40].copy_from_slice(&note.when.to_le_bytes());
        }
        let at = self.keep(seal.to_vec());
        raw[NOTE_SEAL..NOTE_SEAL + 8].copy_from_slice(&at.to_le_bytes());
        raw[NOTE_SEAL + 8..NOTE_SEAL + 16].copy_from_slice(&(seal.len() as u64).to_le_bytes());
        let held = self.keep(raw);
        let note = self.buffer(held, NOTE_BYTES, None, 0);
        self.buffer(at, seal.len(), note, NOTE_SEAL);
    }

    /// The note saying what the secure world saw of a challenge: which
    /// challenge, when, what it checked, and the seal over all of it.
    /// Nothing seen is a note of nothing, which is what a call that changes
    /// nothing is given.
    pub fn seen(&mut self) {
        let held = self.keep(vec![0u8; SEEN_BYTES]);
        let seen = self.buffer(held, SEEN_BYTES, None, 0);
        let checked = self.keep(Vec::new());
        self.buffer(checked, 0, seen, SEEN_VERIFIED);
        let seal = self.keep(Vec::new());
        self.buffer(seal, 0, seen, SEEN_SEAL);
    }

    /// Write a sixteen-byte structure saying where a run of bytes is and how
    /// many of them there are, and then the bytes, saying they lie inside
    /// the structure at its first word.
    fn run(&mut self, at: u64, count: u64, length: usize) {
        let mut frame = Vec::with_capacity(RUN_BYTES);
        frame.extend_from_slice(&at.to_le_bytes());
        frame.extend_from_slice(&count.to_le_bytes());
        let frame_at = self.keep(frame);
        let held = self.buffer(frame_at, RUN_BYTES, None, 0);
        self.buffer(at, length, held, 0);
    }

    /// Hold on to bytes the driver will read, and say where they are. A run
    /// of nothing points nowhere, which is how the far side is told there is
    /// nothing to read.
    fn keep(&mut self, bytes: Vec<u8>) -> u64 {
        if bytes.is_empty() {
            return 0;
        }
        self.pointed += (bytes.len() as u64).next_multiple_of(WORD);
        let bytes = bytes.into_boxed_slice();
        let at = bytes.as_ptr() as u64;
        self.held.push(bytes);
        at
    }

    /// Write a buffer object and say which one it is, or nothing where its
    /// bytes are nowhere: the phone's own libraries write such an object but
    /// leave it out of the list, so the driver copies nothing for it and the
    /// far side finds nothing there. Where it hangs off another, `offset`
    /// says where in that other the pointer leading here lies.
    fn buffer(&mut self, at: u64, length: usize, parent: Option<usize>, offset: usize) -> Option<usize> {
        let mut raw = Vec::with_capacity(BUFFER_BYTES);
        raw.extend_from_slice(&BUFFER.to_le_bytes());
        raw.extend_from_slice(&(if parent.is_some() { HAS_PARENT } else { 0 }).to_le_bytes());
        raw.extend_from_slice(&at.to_le_bytes());
        raw.extend_from_slice(&(length as u64).to_le_bytes());
        raw.extend_from_slice(&parent.map_or(0, |p| p as u64).to_le_bytes());
        raw.extend_from_slice(&(offset as u64).to_le_bytes());
        self.data.extend_from_slice(&raw);
        if at == 0 {
            return None;
        }
        self.objects.push(self.data.len() as u64 - BUFFER_BYTES as u64);
        Some(self.objects.len() - 1)
    }
}

impl Payload for Message {
    fn values(&self) -> &[u8] {
        &self.data
    }

    fn addresses(&self) -> &[u64] {
        &self.objects
    }

    fn pointed(&self) -> u64 {
        self.pointed
    }
}

/// A note the secure world wrote when the user answered their screen lock,
/// as the older interface hands it over.
#[derive(Clone, Copy, Default)]
pub struct Note<'a> {
    pub challenge: u64,
    pub user: u64,
    pub authenticator: u64,
    pub kind: u32,
    pub when: u64,
    pub seal: &'a [u8],
}

/// One key parameter as the older key interface writes it: a tag, the
/// number that came with it, and its bytes where it carries any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub tag: u32,
    pub value: i64,
    pub blob: Vec<u8>,
}

/// Reads back what a hardware service answered.
pub struct Answer<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Answer<'a> {
    pub fn new(data: &'a [u8]) -> Answer<'a> {
        Answer { data, at: 0 }
    }

    /// Whether the call went through. A service that refused says so first,
    /// and says why in the text that follows.
    pub fn status(&mut self) -> Result<()> {
        let code = self.i32().context("read whether the service took the call")?;
        if code == 0 {
            return Ok(());
        }
        match self.why() {
            Ok(why) if !why.is_empty() => bail!("the service refused the call: {why}"),
            _ => bail!("the service refused the call with its code {code}"),
        }
    }

    /// Why the service refused, as the phone's own libraries write it: the
    /// count of a run of UTF-16 characters, those characters, and a
    /// terminator the count leaves out.
    fn why(&mut self) -> Result<String> {
        let units = self.i32()?;
        if units < 0 {
            bail!("text of length {units} was read where some was expected");
        }
        let raw = self.take((units as usize + 1) * 2)?;
        let utf16: Vec<u16> = raw[..units as usize * 2].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        Ok(String::from_utf16_lossy(&utf16))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    /// A truth value: a word, of which only the first byte is read.
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.take(4)?[0] != 0)
    }

    /// The run the next pair of buffer objects leads to: how many bytes it
    /// says there are, and where the driver put them.
    ///
    /// A run is handed over as two: the sixteen-byte structure saying where
    /// the bytes are and how many, and then the bytes themselves. The far
    /// side reads both, so a reader that read only the first would take the
    /// second for whatever it is looking for next.
    fn run(&mut self) -> Result<(u64, u64)> {
        let (at, length) = self.buffer()?.with_context(|| "the service left out something the call asked for")?;
        if length < RUN_BYTES as u64 {
            bail!("the service answered {length} bytes where a run of bytes was expected");
        }
        let raw = unsafe { std::slice::from_raw_parts(at as *const u8, RUN_BYTES) };
        let count = u64::from_le_bytes(raw[8..16].try_into().expect("eight bytes"));
        let (at, _) = self.buffer()?.with_context(|| "the service said where a run of bytes is and not where they are")?;
        Ok((at, count))
    }

    /// A run of bytes the service answered with.
    pub fn bytes(&mut self) -> Result<&'a [u8]> {
        let (at, count) = self.run()?;
        if at == 0 || count == 0 {
            return Ok(&[]);
        }
        Ok(unsafe { std::slice::from_raw_parts(at as *const u8, count as usize) })
    }

    /// Text the service answered with, which is a run of bytes ending in a
    /// terminator the count leaves out.
    pub fn text(&mut self) -> Result<String> {
        Ok(String::from_utf8_lossy(self.bytes()?).into_owned())
    }

    /// The bytes the next buffer object points at. A structure of a fixed
    /// size, and a run that lives inside such a structure, are each handed
    /// over as a single buffer, so they are read a single buffer at a time.
    pub fn pointed(&mut self) -> Result<&'a [u8]> {
        match self.buffer()?.with_context(|| "the service said nothing where something was expected")? {
            (0, _) => bail!("the service left out something the call asked for"),
            (at, length) => Ok(unsafe { std::slice::from_raw_parts(at as *const u8, length as usize) }),
        }
    }

    /// A run of key parameters, each with its own bytes.
    pub fn key_params(&mut self) -> Result<Vec<Param>> {
        let (at, count) = self.run()?;
        if at == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let raw = unsafe { std::slice::from_raw_parts((at as usize + i * PARAM_BYTES) as *const u8, PARAM_BYTES) };
            let tag = u32::from_le_bytes(raw[0..4].try_into().expect("four bytes"));
            let value = i64::from_le_bytes(raw[8..16].try_into().expect("eight bytes"));
            let blob = match self.buffer()? {
                Some((at, length)) if at != 0 => unsafe { std::slice::from_raw_parts(at as *const u8, length as usize) }.to_vec(),
                _ => Vec::new(),
            };
            out.push(Param { tag, value, blob });
        }
        Ok(out)
    }

    /// What a key may be used for: the parameters the software keeps to and
    /// the ones the hardware does, for the one security level the older
    /// interface speaks of.
    pub fn key_characteristics(&mut self) -> Result<(Vec<Param>, Vec<Param>)> {
        let (at, length) = self.buffer()?.with_context(|| "the service said nothing about the key")?;
        if at == 0 || length < 32 {
            return Ok((Vec::new(), Vec::new()));
        }
        let raw = unsafe { std::slice::from_raw_parts(at as *const u8, 32) };
        let mut held = Vec::with_capacity(2);
        for half in 0..2 {
            let field = half * 16;
            let elements = u64::from_le_bytes(raw[field + 8..field + 16].try_into().expect("eight bytes"));
            let Some((array, _)) = self.buffer()? else {
                held.push(Vec::new());
                continue;
            };
            let mut params = Vec::with_capacity(elements as usize);
            for i in 0..elements as usize {
                let raw = unsafe { std::slice::from_raw_parts((array as usize + i * PARAM_BYTES) as *const u8, PARAM_BYTES) };
                let tag = u32::from_le_bytes(raw[0..4].try_into().expect("four bytes"));
                let value = i64::from_le_bytes(raw[8..16].try_into().expect("eight bytes"));
                let blob = match self.buffer()? {
                    Some((at, length)) if at != 0 => unsafe { std::slice::from_raw_parts(at as *const u8, length as usize) }.to_vec(),
                    _ => Vec::new(),
                };
                params.push(Param { tag, value, blob });
            }
            held.push(params);
        }
        let hardware = held.pop().unwrap_or_default();
        let software = held.pop().unwrap_or_default();
        Ok((software, hardware))
    }

    /// The handle of an object in the answering process.
    pub fn object(&mut self) -> Result<Option<u32>> {
        let raw = self.take(OBJECT_BYTES)?;
        let kind = u32::from_le_bytes(raw[0..4].try_into().expect("four bytes"));
        let handle = u32::from_le_bytes(raw[8..12].try_into().expect("four bytes"));
        Ok((kind == OBJECT).then_some(handle))
    }

    /// The next buffer object: where its bytes landed, and how many there
    /// are. An object written pointing nowhere says so by leading nowhere.
    fn buffer(&mut self) -> Result<Option<(u64, u64)>> {
        if self.at >= self.data.len() {
            return Ok(None);
        }
        let raw = self.take(BUFFER_BYTES)?;
        let kind = u32::from_le_bytes(raw[0..4].try_into().expect("four bytes"));
        if kind != BUFFER {
            bail!("the reply holds {kind:#x} where a buffer was expected");
        }
        let at = u64::from_le_bytes(raw[8..16].try_into().expect("eight bytes"));
        let length = u64::from_le_bytes(raw[16..24].try_into().expect("eight bytes"));
        Ok(Some((at, length)))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at + n;
        if end > self.data.len() {
            bail!("the reply holds {} bytes, {n} more were read at {}", self.data.len(), self.at);
        }
        let out = &self.data[self.at..end];
        self.at = end;
        Ok(out)
    }
}

/// The hardware service serving `interface` as `instance`, or `None` while
/// nothing has registered it.
#[cfg(target_os = "android")]
pub fn find<'a>(binder: &'a crate::driver::Binder, interface: &str, instance: &str) -> Result<Option<crate::driver::Handle<'a>>> {
    use crate::driver::Handle;
    // The directory the phone's hardware services register with, which is
    // a second one beside the directory the rest of the phone uses, and
    // the call that asks it for a service.
    const DIRECTORY: &str = "android.hidl.manager@1.0::IServiceManager";
    const GET: u32 = 1;
    let directory = binder.directory()?;
    let mut call = Message::call(DIRECTORY);
    call.text(interface);
    call.text(instance);
    let reply = directory
        .transact_within(GET, &call, Some(std::time::Duration::from_secs(5)))
        .with_context(|| format!("look up {interface}/{instance}"))?;
    let mut read = Answer::new(reply.bytes());
    read.status().with_context(|| format!("look up {interface}/{instance}"))?;
    match read.object().with_context(|| format!("read what the directory answered for {interface}"))? {
        Some(handle) => Ok(Some(Handle::hold(reply.binder(), handle)?)),
        None => Ok(None),
    }
}

/// The name a hardware service gives for the interface it was reached by,
/// which is what a call that changes nothing answers.
#[cfg(target_os = "android")]
pub fn describes(service: &crate::driver::Handle) -> Result<String> {
    // Every hardware service serves this interface besides its own, and a
    // call on it is named as being on it whatever else the service serves.
    const BASE: &str = "android.hidl.base@1.0::IBase";
    const DESCRIBE: u32 = 0x0f44_5343;
    let reply = service.transact(DESCRIBE, &Message::call(BASE)).context("ask the service what it serves")?;
    let mut read = Answer::new(reply.bytes());
    read.status().context("ask the service what it serves")?;
    read.text().context("read the name of what the service serves")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the buffers of a written call back out of its own memory, the
    /// way the driver does.
    fn written(m: &Message) -> Vec<(u32, u64, usize, Vec<u8>)> {
        m.objects
            .iter()
            .map(|at| {
                let raw = &m.data[*at as usize..*at as usize + BUFFER_BYTES];
                let flags = u32::from_le_bytes(raw[4..8].try_into().unwrap());
                let points = u64::from_le_bytes(raw[8..16].try_into().unwrap());
                let length = u64::from_le_bytes(raw[16..24].try_into().unwrap()) as usize;
                let inside = u64::from_le_bytes(raw[24..32].try_into().unwrap()) as usize;
                let offset = u64::from_le_bytes(raw[32..40].try_into().unwrap()) as usize;
                let bytes = unsafe { std::slice::from_raw_parts(points as *const u8, length) }.to_vec();
                assert_eq!(u32::from_le_bytes(raw[0..4].try_into().unwrap()), BUFFER);
                (flags, inside as u64, offset, bytes)
            })
            .collect()
    }

    #[test]
    fn a_call_opens_with_the_interface_name_and_a_terminator() {
        let m = Message::call("a.b@1.0::IC");
        assert_eq!(&m.data[..11], b"a.b@1.0::IC");
        assert_eq!(m.data.len(), 12, "the name, its terminator, and padding to four");
        assert_eq!(m.data[11], 0);
        assert!(m.objects.is_empty());
    }

    #[test]
    fn numbers_are_written_where_they_fall() {
        let mut m = Message::call("a.b@1.0::IC");
        m.u32(7);
        m.u64(1 << 40);
        assert_eq!(m.data.len(), 12 + 4 + 8, "a word is not moved to a boundary of its own");
        assert_eq!(u32::from_le_bytes(m.data[12..16].try_into().unwrap()), 7);
        assert_eq!(u64::from_le_bytes(m.data[16..24].try_into().unwrap()), 1 << 40);
    }

    #[test]
    fn text_is_a_run_saying_where_it_is_and_then_the_text() {
        let mut m = Message::call("a.b@1.0::IC");
        m.text("default");
        let parts = written(&m);
        assert_eq!(parts.len(), 2, "the run, then the text");
        assert_eq!(parts[0].0, 0, "the run hangs off nothing");
        assert_eq!(parts[0].3.len(), RUN_BYTES);
        assert_eq!(u64::from_le_bytes(parts[0].3[8..16].try_into().unwrap()), 7, "the terminator is not counted");
        assert_eq!(parts[1].0, HAS_PARENT, "the text hangs off the run");
        assert_eq!(parts[1].1, 0, "inside the first buffer written, named by its place in the list");
        assert_eq!(parts[1].2, 0, "at its first word");
        assert_eq!(parts[1].3, b"default\0", "but it is carried");
    }

    #[test]
    fn a_run_points_at_the_bytes_it_is_followed_by() {
        let mut m = Message::call("a.b@1.0::IC");
        m.bytes(&[9; 5]);
        let parts = written(&m);
        let in_run = u64::from_le_bytes(parts[0].3[0..8].try_into().unwrap());
        let raw = &m.data[m.objects[1] as usize..];
        assert_eq!(in_run, u64::from_le_bytes(raw[8..16].try_into().unwrap()), "the run's first word is where the bytes land");
        assert_eq!(m.pointed, (RUN_BYTES as u64).next_multiple_of(WORD) + 8, "both rounded up to a word");
    }

    #[test]
    fn a_run_of_nothing_is_written_pointing_nowhere() {
        let mut m = Message::call("a.b@1.0::IC");
        m.bytes(&[]);
        assert_eq!(m.objects.len(), 1, "only the run is named; the bytes are nowhere");
        assert_eq!(m.data.len(), 12 + 2 * BUFFER_BYTES);
        assert_eq!(m.pointed, RUN_BYTES as u64, "nothing is set aside for bytes nobody reads");
    }

    #[test]
    fn a_parameter_carries_its_tag_its_number_and_its_bytes() {
        let mut m = Message::call("a.b@1.0::IC");
        m.params(&[(0x9000_02C1, 0, b"nonce".to_vec())]);
        let parts = written(&m);
        assert_eq!(parts.len(), 3, "the run, the array, and the parameter's bytes");
        let array = &parts[1].3;
        assert_eq!(array.len(), PARAM_BYTES);
        assert_eq!(u32::from_le_bytes(array[0..4].try_into().unwrap()), 0x9000_02C1);
        assert_eq!(i64::from_le_bytes(array[8..16].try_into().unwrap()), 0);
        assert_eq!(parts[2].1, 1, "the bytes hang off the array, not the run");
        assert_eq!(parts[2].2, PARAM_BLOB, "the pointer to them lies after the tag and its number");
        assert_eq!(parts[2].3, b"nonce");
    }

    #[test]
    fn a_parameter_that_chooses_from_a_list_keeps_the_choice_alone_in_its_word() {
        let mut m = Message::call("a.b@1.0::IC");
        m.params(&[(0x2000_0004, 32, Vec::new())]);
        let array = written(&m)[1].3.clone();
        assert_eq!(i64::from_le_bytes(array[8..16].try_into().unwrap()), 32, "the choice is the number that came with it");
    }

    #[test]
    fn a_run_of_no_parameters_carries_none() {
        let mut m = Message::call("a.b@1.0::IC");
        m.params(&[]);
        assert_eq!(m.objects.len(), 1, "the run is named; an array of nothing is not");
        assert_eq!(m.data.len(), 12 + 2 * BUFFER_BYTES);
    }

    #[test]
    fn a_note_is_one_structure_with_its_seal_hanging_off_it() {
        let mut m = Message::call("a.b@1.0::IC");
        let note = Note { challenge: 1, user: 2, authenticator: 3, kind: 4, when: 5, seal: b"seal" };
        m.note(Some(&note));
        let parts = written(&m);
        assert_eq!(parts.len(), 2);
        let raw = &parts[0].3;
        assert_eq!(raw.len(), NOTE_BYTES);
        assert_eq!(u64::from_le_bytes(raw[0..8].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(raw[24..28].try_into().unwrap()), 4);
        assert_eq!(u64::from_le_bytes(raw[32..40].try_into().unwrap()), 5, "when is a whole word, past the choice");
        assert_eq!(parts[1].2, NOTE_SEAL, "the seal is pointed at from its own place in the note");
        assert_eq!(parts[1].3, b"seal");
    }

    #[test]
    fn a_note_of_nothing_is_still_a_note() {
        let mut m = Message::call("a.b@1.0::IC");
        m.note(None);
        let parts = written(&m);
        assert_eq!(parts[0].3.len(), NOTE_BYTES);
        assert_eq!(parts[0].3, vec![0u8; NOTE_BYTES]);
    }

    #[test]
    fn the_note_of_what_was_seen_holds_its_places_for_the_list_and_the_seal() {
        let mut m = Message::call("a.b@1.0::IC");
        m.seen();
        let parts = written(&m);
        assert_eq!(parts.len(), 1, "a list of nothing and a seal of nothing are written but not named");
        assert_eq!(parts[0].3.len(), SEEN_BYTES, "the far side reads a whole note even when it says nothing");
        assert_eq!(parts[0].3, vec![0u8; SEEN_BYTES]);
        assert_eq!(m.data.len() - 12, 3 * BUFFER_BYTES, "the note and the two places in it");
        let places: Vec<u64> = (1..3)
            .map(|i| {
                let at = 12 + i * BUFFER_BYTES;
                u64::from_le_bytes(m.data[at + 32..at + 40].try_into().unwrap())
            })
            .collect();
        assert_eq!(places, vec![SEEN_VERIFIED as u64, SEEN_SEAL as u64]);
    }

    #[test]
    fn a_service_that_refused_the_call_is_an_error() {
        let mut reply = 3i32.to_le_bytes().to_vec();
        reply.extend_from_slice(&(-1i32).to_le_bytes());
        assert!(Answer::new(&reply).status().is_err());
        assert!(Answer::new(&0i32.to_le_bytes()).status().is_ok());
    }

    #[test]
    fn reading_past_the_end_of_a_reply_is_an_error() {
        assert!(Answer::new(&[]).i32().is_err());
        assert!(Answer::new(&[0; 8]).bytes().is_err());
    }

    #[test]
    fn a_truth_value_is_a_byte_padded_to_four() {
        let mut read = Answer::new(&[1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0]);
        assert!(read.bool().unwrap());
        assert!(!read.bool().unwrap());
        assert_eq!(read.i32().unwrap(), 7, "what follows it starts four bytes on");
        assert!(Answer::new(&[1, 0]).bool().is_err());
    }

    /// One buffer object as the far side receives it: the bytes are at
    /// `at`, and it says so like the driver's own copy would.
    fn object(at: u64, length: usize, parent: u64, offset: u64) -> Vec<u8> {
        let mut raw = BUFFER.to_le_bytes().to_vec();
        raw.extend_from_slice(&(if parent != 0 { HAS_PARENT } else { 0 }).to_le_bytes());
        raw.extend_from_slice(&at.to_le_bytes());
        raw.extend_from_slice(&(length as u64).to_le_bytes());
        raw.extend_from_slice(&parent.to_le_bytes());
        raw.extend_from_slice(&offset.to_le_bytes());
        raw
    }

    #[test]
    fn bytes_are_read_back_where_the_driver_put_them() {
        let payload = b"abc".to_vec();
        let mut run = (payload.as_ptr() as u64).to_le_bytes().to_vec();
        run.extend_from_slice(&3u64.to_le_bytes());
        let at = run.as_ptr() as u64;
        let mut raw = object(at, RUN_BYTES, 0, 0);
        raw.extend_from_slice(&object(payload.as_ptr() as u64, payload.len(), 0, 0));
        assert_eq!(Answer::new(&raw).bytes().unwrap(), b"abc");
    }

    #[test]
    fn a_run_is_read_as_the_two_buffers_it_was_written_as() {
        let payload = b"abc".to_vec();
        let mut run = (payload.as_ptr() as u64).to_le_bytes().to_vec();
        run.extend_from_slice(&3u64.to_le_bytes());
        let mut raw = object(run.as_ptr() as u64, RUN_BYTES, 0, 0);
        raw.extend_from_slice(&object(payload.as_ptr() as u64, payload.len(), 0, 0));
        let mut read = Answer::new(&raw);
        let (at, count) = read.run().unwrap();
        assert_eq!(count, 3, "the count comes from the structure, not from the bytes");
        assert_eq!(at, payload.as_ptr() as u64, "and the bytes from the second buffer");
        assert!(read.run().is_err(), "both buffers were taken");
    }

    /// The driver's own rules for the buffers of a call, as the phone's
    /// kernel states them: every buffer named must point at real bytes and
    /// fit in the room set aside for them, and filling in a pointer inside
    /// another buffer may only reach backwards from a buffer just handled or
    /// one of its own parents, and never before where that one was itself
    /// filled in.
    fn check(m: &Message) {
        assert_eq!(m.pointed % WORD, 0, "the room set aside is a whole number of words");
        let (mut last, mut last_min) = (0usize, 0usize);
        let mut room = m.pointed;
        for (i, at) in m.objects.iter().enumerate() {
            let raw = &m.data[*at as usize..*at as usize + BUFFER_BYTES];
            let flags = u32::from_le_bytes(raw[4..8].try_into().unwrap());
            let points = u64::from_le_bytes(raw[8..16].try_into().unwrap());
            let length = u64::from_le_bytes(raw[16..24].try_into().unwrap());
            let parent = u64::from_le_bytes(raw[24..32].try_into().unwrap()) as usize;
            let offset = u64::from_le_bytes(raw[32..40].try_into().unwrap()) as usize;
            assert_ne!(points, 0, "buffer {i} points nowhere");
            assert!(length <= room, "buffer {i} asks for more room than was set aside");
            room -= length.next_multiple_of(WORD);
            if flags & HAS_PARENT != 0 {
                assert!(parent < i, "buffer {i} names a parent that was not handed over before it");
                let parent_at = m.objects[parent] as usize;
                let mut walk = last;
                let mut min = last_min;
                let mut steps = 0;
                while walk != parent_at {
                    assert!(walk != 0, "buffer {i} names a parent that is not in reach");
                    let raw = &m.data[walk..walk + BUFFER_BYTES];
                    assert_ne!(u32::from_le_bytes(raw[4..8].try_into().unwrap()) & HAS_PARENT, 0, "buffer {i} names a parent that is not in reach");
                    min = u64::from_le_bytes(raw[32..40].try_into().unwrap()) as usize + 8;
                    walk = m.objects[u64::from_le_bytes(raw[24..32].try_into().unwrap()) as usize] as usize;
                    steps += 1;
                    assert!(steps <= m.objects.len(), "the buffers lead round in a ring");
                }
                assert!(offset >= min, "buffer {i} fills in a place before the last one filled in");
                let length = u64::from_le_bytes(m.data[parent_at + 16..parent_at + 24].try_into().unwrap());
                assert!(length >= 8 && (offset as u64) <= length - 8, "buffer {i} fills in a place past where its parent ends");
            }
            (last, last_min) = (*at as usize, 0);
        }
    }

    #[test]
    fn a_call_with_many_parameters_keeps_to_the_drivers_rules() {
        let mut m = Message::call("android.hardware.keymaster@4.0::IKeymasterDevice");
        m.u32(1);
        m.bytes(&[7; 546]);
        m.params(&[
            (0x2000_0004, 32, Vec::new()),
            (0x2000_0006, 1, Vec::new()),
            (0x3000_000A, 128, Vec::new()),
            (0x9000_0259, 0, vec![9; 64]),
            (0x9000_02C1, 0, vec![3; 12]),
            (0x2000_0001, 1, Vec::new()),
        ]);
        m.note(None);
        check(&m);
        assert_eq!(m.objects.len(), 2 + 2 + 2 + 1, "the key, the parameters and their two runs of bytes, and the note");
    }

    #[test]
    fn every_pointer_is_filled_in_from_its_own_place_in_the_array() {
        let mut m = Message::call("a.b@1.0::IC");
        m.params(&[(1, 0, vec![1; 32]), (2, 0, vec![2; 32])]);
        check(&m);
        let offsets: Vec<u64> = (2..4).map(|i| u64::from_le_bytes(m.data[m.objects[i] as usize + 32..m.objects[i] as usize + 40].try_into().unwrap())).collect();
        assert_eq!(offsets, vec![PARAM_BLOB as u64, (PARAM_BYTES + PARAM_BLOB) as u64]);
    }

    #[test]
    fn a_call_whose_second_parameter_reaches_backwards_is_refused() {
        let mut m = Message::call("a.b@1.0::IC");
        let at = m.keep(vec![1; 32]);
        let frame = m.keep(vec![0; RUN_BYTES]);
        let held = m.buffer(frame, RUN_BYTES, None, 0);
        let array = m.buffer(at, PARAM_BYTES, Some(held.unwrap()), 0).unwrap();
        // The second buffer fills in the same place again, which the driver
        // will not have: it only lets a pointer be filled in past the last
        // one filled in.
        m.buffer(at, 32, Some(array), 0);
        m.buffer(at, 32, Some(array), 0);
        assert!(std::panic::catch_unwind(|| check(&m)).is_err());
    }
}
