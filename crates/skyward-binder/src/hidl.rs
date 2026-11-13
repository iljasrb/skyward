//! The layout the phone's hardware services speak, as the phone's own
//! libraries write it.
//!
//! A call opens with the name of the interface it is made on, written as
//! plain bytes with a terminator and padded to four. What follows is a run of
//! plain numbers, except for anything longer than a number: those are handed
//! over as *buffer objects*. A buffer object is forty bytes saying where some
//! bytes are, how many there are, whether they sit inside something already
//! handed over, and where in that thing the pointer to them lies.
//!
//! The driver copies what every buffer object points at into the far side's
//! own room and fills in the pointers that lead to the copies, so the far
//! side never reads a pointer written here. [Handed] is a thing to hand over
//! with those pointers named; [Message] writes it and the driver completes
//! it. That is the whole of the layout: text, lists, and the structures the
//! key and credential services trade are all things handed over this way,
//! and the layouts of those structures belong to the services that use them.
//!
//! Every number here was read off the phone's own `libhidlbase.so`, so it is
//! what the phone's services were built to read.

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

/// The structure a counted run of things is handed over as: where they are,
/// and how many there are.
pub const RUN_BYTES: usize = 16;

/// What the driver copies for a buffer is counted in whole words.
const WORD: u64 = 8;

/// Something to hand over whole: its bytes, and for each pointer in them,
/// where that pointer lies and what it leads to. A thing that leads to
/// nothing is handed over on its own.
///
/// The driver fills in the pointers of one thing in the order they lie in
/// it, and refuses a call that asks for them in any other order, so they are
/// kept in that order here.
#[derive(Clone, Default)]
pub struct Handed {
    bytes: Vec<u8>,
    leads: Vec<(usize, Handed)>,
}

impl Handed {
    /// The bytes of a thing, with nothing in it leading anywhere yet.
    pub fn of(bytes: impl Into<Vec<u8>>) -> Handed {
        Handed { bytes: bytes.into(), leads: Vec::new() }
    }

    /// A pointer lying `at` bytes into this thing, leading to `to`.
    pub fn leading_to(mut self, at: usize, to: Handed) -> Handed {
        self.leads.push((at, to));
        self.leads.sort_by_key(|(at, _)| *at);
        self
    }

    /// The structure a counted run of things is handed over as.
    pub fn run(count: u64, to: Handed) -> Handed {
        let mut bytes = vec![0u8; RUN_BYTES];
        // The first word is where the driver puts the address of the copy;
        // what is written here is never read.
        bytes[8..16].copy_from_slice(&count.to_le_bytes());
        Handed::of(bytes).leading_to(0, to)
    }
}

/// A call being written. What the buffer objects point at is kept here too,
/// so it stays where it is until the driver has read it.
#[derive(Default)]
pub struct Message {
    data: Vec<u8>,
    /// Where each buffer object lies among the values, in the order written.
    objects: Vec<u64>,
    kept: Vec<Box<[u8]>>,
    set_aside: u64,
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

    pub fn u64(&mut self, v: u64) {
        self.data.extend_from_slice(&v.to_le_bytes());
    }

    /// Hand over `whole`, and everything it leads to.
    pub fn hand(&mut self, whole: &Handed) {
        self.write(whole, None, 0);
    }

    /// A run of bytes handed over as a list. The far side reads how many
    /// there are, then the bytes.
    pub fn bytes(&mut self, value: &[u8]) {
        self.hand(&Handed::run(value.len() as u64, Handed::of(value)));
    }

    /// A run of characters handed over as text. The far side is told how
    /// many characters there are, and given them with a terminator it is
    /// not told about.
    pub fn text(&mut self, value: &str) {
        let mut raw = value.as_bytes().to_vec();
        raw.push(0);
        self.hand(&Handed::run(value.len() as u64, Handed::of(raw)));
    }

    /// Write one buffer object and everything that hangs off it. `inside`
    /// says which buffer object it sits in, where its bytes are nowhere to
    /// be found, and `at` says where in that one the pointer to it lies.
    fn write(&mut self, whole: &Handed, inside: Option<usize>, at: usize) {
        debug_assert!(whole.leads.is_empty() || !whole.bytes.is_empty(), "a thing of no bytes cannot lead anywhere");
        let bytes = self.keep(&whole.bytes);
        let Some(which) = self.buffer(bytes, whole.bytes.len(), inside, at) else {
            // A thing whose bytes are nowhere is written pointing nowhere:
            // the driver copies nothing for it and the far side finds
            // nothing there, so there is nothing for it to lead to either.
            return;
        };
        for (at, leads) in &whole.leads {
            self.write(leads, Some(which), *at);
        }
    }

    /// Write a buffer object and say which one it is, or nothing where its
    /// bytes are nowhere: the phone's own libraries write such an object but
    /// leave it out of the list, so the driver copies nothing for it.
    fn buffer(&mut self, bytes: u64, length: usize, inside: Option<usize>, at: usize) -> Option<usize> {
        let mut raw = Vec::with_capacity(BUFFER_BYTES);
        raw.extend_from_slice(&BUFFER.to_le_bytes());
        raw.extend_from_slice(&(if inside.is_some() { HAS_PARENT } else { 0 }).to_le_bytes());
        raw.extend_from_slice(&bytes.to_le_bytes());
        raw.extend_from_slice(&(length as u64).to_le_bytes());
        raw.extend_from_slice(&inside.map_or(0, |p| p as u64).to_le_bytes());
        raw.extend_from_slice(&(at as u64).to_le_bytes());
        self.data.extend_from_slice(&raw);
        if bytes == 0 {
            return None;
        }
        self.objects.push(self.data.len() as u64 - BUFFER_BYTES as u64);
        Some(self.objects.len() - 1)
    }

    /// Hold on to bytes the driver will read, and say where they are. A run
    /// of nothing points nowhere, which is how the far side is told there is
    /// nothing to read.
    fn keep(&mut self, bytes: &[u8]) -> u64 {
        if bytes.is_empty() {
            return 0;
        }
        self.set_aside += (bytes.len() as u64).next_multiple_of(WORD);
        let bytes: Box<[u8]> = bytes.into();
        let at = bytes.as_ptr() as u64;
        self.kept.push(bytes);
        at
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
        self.set_aside
    }
}

/// Reads back what a hardware service answered. A reply is a run of plain
/// numbers with buffer objects among them, in the order the far side wrote
/// them, so it is read back a number or a buffer at a time.
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

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("eight bytes")))
    }

    /// A truth value: a word, of which only the first byte is read.
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.take(4)?[0] != 0)
    }

    /// The bytes of the next buffer object, which is a thing handed over
    /// whole, or a part of one that the far side wrote itself.
    pub fn whole(&mut self) -> Result<&'a [u8]> {
        let (at, length) = self.buffer()?.with_context(|| "the service said nothing where something was expected")?;
        if at == 0 {
            bail!("the service left out something the call asked for");
        }
        Ok(unsafe { std::slice::from_raw_parts(at as *const u8, length as usize) })
    }

    /// The bytes of the next buffer object, where the far side may have
    /// written one pointing nowhere instead: a part of something it wrote in
    /// more than one piece.
    pub fn part(&mut self) -> Result<Option<&'a [u8]>> {
        match self.buffer()? {
            Some((0, _)) | None => Ok(None),
            Some((at, length)) => Ok(Some(unsafe { std::slice::from_raw_parts(at as *const u8, length as usize) })),
        }
    }

    /// A counted run of things: how many there are, and the bytes they are
    /// in. A run is handed over as two buffer objects, the structure saying
    /// where the things are and the things themselves, and both are read.
    pub fn run(&mut self) -> Result<(&'a [u8], u64)> {
        let frame = self.whole().context("read where a run of things is")?;
        if frame.len() < RUN_BYTES {
            bail!("the service answered {} bytes where a run of things belongs", frame.len());
        }
        let count = u64::from_le_bytes(frame[8..16].try_into().expect("eight bytes"));
        Ok((self.part()?.unwrap_or(&[]), count))
    }

    /// A run of bytes the service answered with.
    pub fn bytes(&mut self) -> Result<&'a [u8]> {
        Ok(self.run()?.0)
    }

    /// Text the service answered with, which is a run of characters ending
    /// in a terminator the far side did not count.
    pub fn text(&mut self) -> Result<String> {
        let (bytes, count) = self.run()?;
        let count = (count as usize).min(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..count]).into_owned())
    }

    /// The handle of an object in the answering process.
    pub fn object(&mut self) -> Result<Option<u32>> {
        let raw = self.take(OBJECT_BYTES)?;
        let kind = u32::from_le_bytes(raw[0..4].try_into().expect("four bytes"));
        let handle = u32::from_le_bytes(raw[8..12].try_into().expect("four bytes"));
        Ok((kind == OBJECT).then_some(handle))
    }

    /// The next buffer object: where its bytes landed and how many there
    /// are, or nothing where the far side wrote one pointing nowhere. An
    /// object written pointing nowhere is not in the driver's list either,
    /// so the far side never filled it in, and it stands for no bytes.
    fn buffer(&mut self) -> Result<Option<(u64, u64)>> {
        if self.at >= self.data.len() {
            return Ok(None);
        }
        let raw = self.take(BUFFER_BYTES)?;
        let kind = u32::from_le_bytes(raw[0..4].try_into().expect("four bytes"));
        if kind != BUFFER {
            bail!("the reply holds {kind:#x} where a buffer was expected");
        }
        Ok(Some((
            u64::from_le_bytes(raw[8..16].try_into().expect("eight bytes")),
            u64::from_le_bytes(raw[16..24].try_into().expect("eight bytes")),
        )))
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

    /// A buffer object as the far side receives it: the bytes are at `at`,
    /// and it says so the way the driver's own copy would.
    fn object(at: u64, length: usize, inside: u64, pointer_at: u64) -> Vec<u8> {
        let mut raw = BUFFER.to_le_bytes().to_vec();
        raw.extend_from_slice(&(if inside != 0 { HAS_PARENT } else { 0 }).to_le_bytes());
        raw.extend_from_slice(&at.to_le_bytes());
        raw.extend_from_slice(&(length as u64).to_le_bytes());
        raw.extend_from_slice(&inside.to_le_bytes());
        raw.extend_from_slice(&pointer_at.to_le_bytes());
        raw
    }

    /// The buffers of a written call, as the driver reads them: what each
    /// one says, and the bytes it points at.
    fn written(m: &Message) -> Vec<(u32, u64, usize, Vec<u8>)> {
        m.objects
            .iter()
            .map(|at| {
                let raw = &m.data[*at as usize..*at as usize + BUFFER_BYTES];
                assert_eq!(u32::from_le_bytes(raw[0..4].try_into().unwrap()), BUFFER);
                let flags = u32::from_le_bytes(raw[4..8].try_into().unwrap());
                let points = u64::from_le_bytes(raw[8..16].try_into().unwrap());
                let length = u64::from_le_bytes(raw[16..24].try_into().unwrap()) as usize;
                let inside = u64::from_le_bytes(raw[24..32].try_into().unwrap());
                let pointer_at = u64::from_le_bytes(raw[32..40].try_into().unwrap()) as usize;
                let bytes = unsafe { std::slice::from_raw_parts(points as *const u8, length) }.to_vec();
                (flags, inside, pointer_at, bytes)
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
    fn text_is_a_run_saying_how_much_there_is_and_then_the_text() {
        let mut m = Message::call("a.b@1.0::IC");
        m.text("default");
        let parts = written(&m);
        assert_eq!(parts.len(), 2, "the run, then the text");
        assert_eq!(parts[0].0, 0, "the run hangs off nothing");
        assert_eq!(parts[0].3.len(), RUN_BYTES);
        assert_eq!(u64::from_le_bytes(parts[0].3[8..16].try_into().unwrap()), 7, "the terminator is not counted");
        assert_eq!(parts[1].0, HAS_PARENT, "the text hangs off the run");
        assert_eq!(parts[1].1, 0, "inside the first buffer written, named by its place in the list");
        assert_eq!(parts[1].2, 0, "at its first word, where its pointer lies");
        assert_eq!(parts[1].3, b"default\0", "but it is carried");
    }

    #[test]
    fn a_run_says_where_its_bytes_are_with_the_pointer_it_leaves_for_the_driver() {
        let mut m = Message::call("a.b@1.0::IC");
        m.bytes(&[9; 5]);
        let parts = written(&m);
        assert_eq!(parts[0].3[0..8], [0u8; 8], "the driver fills this in; nothing written here is read");
        assert_eq!(m.pointed(), (RUN_BYTES as u64).next_multiple_of(WORD) + 8, "room for both, in whole words");
    }

    #[test]
    fn a_run_of_nothing_is_written_pointing_nowhere() {
        let mut m = Message::call("a.b@1.0::IC");
        m.bytes(&[]);
        assert_eq!(m.objects.len(), 1, "only the run is named; the bytes are nowhere");
        assert_eq!(m.data.len(), 12 + 2 * BUFFER_BYTES, "both are written all the same");
        assert_eq!(m.pointed(), RUN_BYTES as u64, "nothing is set aside for bytes nobody reads");
    }

    #[test]
    fn a_thing_leading_to_others_says_where_each_pointer_lies() {
        let mut record = vec![0u8; 32];
        record[0..4].copy_from_slice(&7u32.to_le_bytes());
        // Named out of order, kept in the order they lie in.
        let whole = Handed::of(record).leading_to(24, Handed::of(vec![1u8; 4])).leading_to(16, Handed::of(vec![2u8; 3]));
        let mut m = Message::call("a.b@1.0::IC");
        m.hand(&whole);
        let parts = written(&m);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].3.len(), 32);
        assert_eq!((parts[1].1, parts[1].2, parts[1].3.clone()), (0, 16, vec![2u8; 3]), "the lower one first");
        assert_eq!((parts[2].1, parts[2].2, parts[2].3.clone()), (0, 24, vec![1u8; 4]));
    }

    #[test]
    fn a_thing_that_leads_nowhere_is_handed_over_on_its_own() {
        let mut m = Message::call("a.b@1.0::IC");
        m.hand(&Handed::of(vec![3u8; 24]));
        let parts = written(&m);
        assert_eq!(parts.len(), 1);
        assert_eq!((parts[0].0, parts[0].3.clone()), (0, vec![3u8; 24]));
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
        assert!(Answer::new(&[0; 8]).whole().is_err());
    }

    #[test]
    fn a_truth_value_is_a_byte_padded_to_four() {
        let mut read = Answer::new(&[1, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0]);
        assert!(read.bool().unwrap());
        assert!(!read.bool().unwrap());
        assert_eq!(read.i32().unwrap(), 7, "what follows it starts four bytes on");
        assert!(Answer::new(&[1, 0]).bool().is_err());
    }

    #[test]
    fn bytes_are_read_back_where_the_driver_put_them() {
        let payload = b"abc".to_vec();
        let mut run = [0u8; RUN_BYTES];
        run[8..16].copy_from_slice(&3u64.to_le_bytes());
        let mut raw = object(run.as_ptr() as u64, RUN_BYTES, 0, 0);
        raw.extend_from_slice(&object(payload.as_ptr() as u64, payload.len(), 0, 0));
        assert_eq!(Answer::new(&raw).bytes().unwrap(), b"abc");
    }

    #[test]
    fn a_run_is_read_as_the_two_buffers_it_was_written_as() {
        let payload = b"abc".to_vec();
        let mut run = [0u8; RUN_BYTES];
        run[8..16].copy_from_slice(&3u64.to_le_bytes());
        let mut raw = object(run.as_ptr() as u64, RUN_BYTES, 0, 0);
        raw.extend_from_slice(&object(payload.as_ptr() as u64, payload.len(), 0, 0));
        let mut read = Answer::new(&raw);
        let (bytes, count) = read.run().unwrap();
        assert_eq!(count, 3, "the count comes from the structure, not from the bytes");
        assert_eq!(bytes, b"abc");
        assert!(read.run().is_err(), "both buffers were taken");
    }

    #[test]
    fn a_run_of_nothing_is_read_as_nothing() {
        let mut run = [0u8; RUN_BYTES];
        run[8..16].copy_from_slice(&0u64.to_le_bytes());
        let mut raw = object(run.as_ptr() as u64, RUN_BYTES, 0, 0);
        raw.extend_from_slice(&object(0, 0, 0, 0));
        let mut read = Answer::new(&raw);
        assert_eq!(read.bytes().unwrap(), b"");
        assert!(read.part().unwrap().is_none(), "and the far side is done");
    }

    #[test]
    fn text_is_read_back_without_the_terminator() {
        let payload = b"default\0".to_vec();
        let mut run = [0u8; RUN_BYTES];
        run[8..16].copy_from_slice(&7u64.to_le_bytes());
        let mut raw = object(run.as_ptr() as u64, RUN_BYTES, 0, 0);
        raw.extend_from_slice(&object(payload.as_ptr() as u64, payload.len(), 0, 0));
        assert_eq!(Answer::new(&raw).text().unwrap(), "default");
    }

    /// The driver's own rules for the buffers of a call, as the phone's
    /// kernel states them: every buffer named must point at real bytes and
    /// fit in the room set aside for them, and a pointer inside another
    /// buffer may only be filled in from a buffer just handled or one of its
    /// own, and never before where that one was itself filled in.
    fn check(m: &Message) {
        assert_eq!(m.set_aside % WORD, 0, "the room set aside is a whole number of words");
        let (mut last, mut last_min) = (0usize, 0usize);
        let mut room = m.set_aside;
        for (i, at) in m.objects.iter().enumerate() {
            let raw = &m.data[*at as usize..*at as usize + BUFFER_BYTES];
            let flags = u32::from_le_bytes(raw[4..8].try_into().unwrap());
            let points = u64::from_le_bytes(raw[8..16].try_into().unwrap());
            let length = u64::from_le_bytes(raw[16..24].try_into().unwrap());
            let inside = u64::from_le_bytes(raw[24..32].try_into().unwrap()) as usize;
            let pointer_at = u64::from_le_bytes(raw[32..40].try_into().unwrap()) as usize;
            assert_ne!(points, 0, "buffer {i} points nowhere");
            assert!(length <= room, "buffer {i} asks for more room than was set aside");
            room -= length.next_multiple_of(WORD);
            if flags & HAS_PARENT != 0 {
                assert!(inside < i, "buffer {i} names a parent that was not handed over before it");
                let parent = m.objects[inside] as usize;
                let (mut walk, mut least, mut steps) = (last, last_min, 0);
                while walk != parent {
                    assert!(walk != 0, "buffer {i} names a parent that is not in reach");
                    let raw = &m.data[walk..walk + BUFFER_BYTES];
                    assert_ne!(u32::from_le_bytes(raw[4..8].try_into().unwrap()) & HAS_PARENT, 0, "buffer {i} names a parent that is not in reach");
                    least = u64::from_le_bytes(raw[32..40].try_into().unwrap()) as usize + 8;
                    walk = m.objects[u64::from_le_bytes(raw[24..32].try_into().unwrap()) as usize] as usize;
                    steps += 1;
                    assert!(steps <= m.objects.len(), "the buffers lead round in a ring");
                }
                assert!(pointer_at >= least, "buffer {i} fills in a place before the last one filled in");
                let length = u64::from_le_bytes(m.data[parent + 16..parent + 24].try_into().unwrap());
                assert!(length >= 8 && (pointer_at as u64) <= length - 8, "buffer {i} fills in a place past where its parent ends");
            }
            (last, last_min) = (*at as usize, 0);
        }
    }

    #[test]
    fn a_call_with_a_list_of_records_keeps_to_the_drivers_rules() {
        let records: Vec<u8> = (0..6u32).flat_map(|i| vec![i as u8; 32]).collect();
        let mut list = Handed::of(records);
        for i in 0..6 {
            list = list.leading_to(i * 32 + 16, Handed::of(vec![7u8; 12]));
        }
        let mut m = Message::call("a.b@1.0::IC");
        m.u32(1);
        m.bytes(&[7; 546]);
        m.hand(&Handed::run(6, list));
        m.hand(&Handed::of(vec![0u8; 56]).leading_to(40, Handed::of(Vec::new())));
        check(&m);
    }

    #[test]
    fn a_pointer_that_reaches_backwards_is_refused() {
        let mut m = Message::call("a.b@1.0::IC");
        let mut list = Handed::of(vec![0u8; 64]);
        for i in [1, 0] {
            list = list.leading_to(i * 32 + 16, Handed::of(vec![1u8; 4]));
        }
        m.hand(&Handed::run(2, list));
        // Kept in the order they lie in, so the driver is never asked for
        // one before another.
        check(&m);
    }
}
