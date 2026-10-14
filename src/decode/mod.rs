//! Reading gob streams.

mod session;
mod value;

use std::borrow::Cow;
use std::fmt;
use std::io::{self, Read};

#[cfg(feature = "serde")]
pub(crate) use session::Bytes;
pub(crate) use session::Session;

use crate::types::TypeTable;
use crate::wire::read_uint;
use crate::{Error, TypeId, Value};

/// Largest message, and largest number of elements in a collection, a
/// decoder accepts. Matches Go's `tooBig` (decoder.go): 8 GiB on 64-bit
/// targets, 1 GiB elsewhere. It only bounds lengths read from the input;
/// buffers grow with the data actually received.
const TOO_BIG: u64 = if usize::BITS >= 64 { 1 << 33 } else { 1 << 30 };

/// Default limit on value nesting. Every level of a struct, collection or
/// interface costs stack space while decoding, and recursive types such as
/// linked lists nest as deep as they are long. Decoding 512 levels takes
/// well under the 2 MiB stack of a spawned thread, even in debug builds.
const DEFAULT_MAX_DEPTH: usize = 512;

/// A gob message, and the size of the length before it.
pub(crate) struct Frame<'a> {
    pub data: Cow<'a, [u8]>,
    pub header: usize,
}

/// A source of length-prefixed gob messages.
pub(crate) trait Frames<'a> {
    fn next_frame(&mut self) -> Result<Option<Frame<'a>>, Error>;
}

/// Where reading stands in a stream.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Position {
    /// Bytes taken from the source so far.
    pub consumed: u64,
    /// Where the data of the current message starts.
    pub message: u64,
}

/// Messages borrowed from an in-memory stream.
pub(crate) struct SliceFrames<'a>(pub &'a [u8]);

impl<'a> Frames<'a> for SliceFrames<'a> {
    fn next_frame(&mut self) -> Result<Option<Frame<'a>>, Error> {
        if self.0.is_empty() {
            return Ok(None);
        }
        let (length, prefix) = read_uint(self.0)?;
        let message_length = check_message_length(length)?;
        let (message, rest) = self.0[prefix..].split_at_checked(message_length).ok_or_else(Error::eof)?;
        self.0 = rest;
        Ok(Some(Frame { data: Cow::Borrowed(message), header: prefix }))
    }
}

/// Messages read one at a time from an I/O object.
pub(crate) struct ReadFrames<R>(pub R);

impl<R: Read> Frames<'static> for ReadFrames<R> {
    fn next_frame(&mut self) -> Result<Option<Frame<'static>>, Error> {
        let mut first = [0u8];
        match self.0.read_exact(&mut first) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let mut length = [0u8; 9];
        length[0] = first[0];
        let size = if first[0] < 0x80 { 0 } else { usize::from(first[0].wrapping_neg()).min(8) };
        self.0.read_exact(&mut length[1..=size])?;
        let (length, _) = read_uint(&length[..=size])?;
        let length = check_message_length(length)?;
        // Grow with the data actually received instead of trusting the length.
        let mut message = Vec::new();
        self.0.by_ref().take(length as u64).read_to_end(&mut message)?;
        if message.len() != length {
            return Err(Error::eof());
        }
        Ok(Some(Frame { data: Cow::Owned(message), header: size + 1 }))
    }
}

fn check_message_length(length: u64) -> Result<usize, Error> {
    match usize::try_from(length) {
        Ok(0) => Err(Error::syntax("empty message")),
        Ok(size) if length < TOO_BIG => Ok(size),
        _ => Err(Error::limit(format!("message of {length} bytes is too large"))),
    }
}

/// Reads gob streams, like Go's `gob.Decoder`.
///
/// A decoder remembers the type definitions it has seen, so keep one decoder
/// per stream and feed it the stream in order. Independent streams, such as
/// messages that were each encoded by a new Go encoder, need a decoder each:
/// they reuse the same type ids.
///
/// Values decode into [`Value`]s, or, with the `serde` feature, into any type
/// implementing `serde::Deserialize` (see [`Decoder::deserialize`]).
///
/// Decoding untrusted input is safe: malformed data is an error, never a
/// panic, lengths read from the input are checked before anything is
/// allocated for them, and nesting is limited (see
/// [`with_max_depth`](Self::with_max_depth)).
///
/// # Examples
///
/// ```
/// use grob::{Decoder, Encoder, TypeId, Value};
///
/// let mut encoder = Encoder::new();
/// encoder.encode(TypeId::STRING, &"hello".into())?;
/// encoder.encode(TypeId::INT, &42.into())?;
/// let stream = encoder.finish();
///
/// let mut decoder = Decoder::new();
/// let values = decoder.decode_all(&stream)?;
/// assert_eq!(values, ["hello".into(), Value::Int(42)]);
/// # Ok::<(), grob::Error>(())
/// ```
pub struct Decoder {
    types: TypeTable,
    /// Unread bytes of the current message, kept between calls.
    pending: Vec<u8>,
    position: Position,
    max_depth: usize,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            types: TypeTable::default(),
            pending: Vec::new(),
            position: Position::default(),
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }
}

impl fmt::Debug for Decoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decoder").field("max_depth", &self.max_depth).finish_non_exhaustive()
    }
}

impl Decoder {
    /// Creates a decoder for a new stream.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Limits how deeply values may nest; deeper input is an error.
    ///
    /// Every struct, collection and interface level counts, so a recursive
    /// type like a linked list nests as deep as it is long. The limit bounds
    /// the stack used while decoding and defaults to 512, which this crate's
    /// decoding fits in a 2 MiB thread stack. `Deserialize` implementations
    /// add stack frames of their own for every level: with large ones in
    /// debug builds, lower the limit or decode on a thread with more stack.
    #[must_use]
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Decodes the first value in `bytes`.
    ///
    /// Type definitions before the value are remembered for later calls;
    /// anything after the value is ignored.
    ///
    /// # Errors
    ///
    /// When `bytes` holds no value, is cut short or is not valid gob data, or
    /// when the value nests deeper than the limit.
    pub fn decode(&mut self, bytes: &[u8]) -> Result<Value, Error> {
        self.read(&mut SliceFrames(bytes), value::decode_top)?.ok_or_else(Error::eof)
    }

    /// Decodes every value in `bytes`.
    ///
    /// # Errors
    ///
    /// When `bytes` is cut short or is not valid gob data, or when a value
    /// nests deeper than the limit.
    pub fn decode_all(&mut self, bytes: &[u8]) -> Result<Vec<Value>, Error> {
        let mut frames = SliceFrames(bytes);
        let mut values = Vec::new();
        while let Some(value) = self.read(&mut frames, value::decode_top)? {
            values.push(value);
        }
        Ok(values)
    }

    /// Reads the next value from `reader`, or `None` at the end of the stream.
    ///
    /// It reads exactly the messages that make up the value, so it can be
    /// called repeatedly on a connection.
    ///
    /// # Errors
    ///
    /// When reading fails, the stream ends in the middle of a value, the data
    /// is not valid gob, or the value nests deeper than the limit.
    pub fn decode_from(&mut self, reader: impl Read) -> Result<Option<Value>, Error> {
        self.read(&mut ReadFrames(reader), value::decode_top)
    }

    /// Reads type definitions up to the next value, then lets `read` consume
    /// the value, which has type `id`. `None` at a clean end of stream.
    pub(crate) fn read<'a, T>(
        &mut self,
        frames: &mut dyn Frames<'a>,
        read: impl FnOnce(&mut Session<'_, 'a>, TypeId) -> Result<T, Error>,
    ) -> Result<Option<T>, Error> {
        let pending = std::mem::take(&mut self.pending);
        let mut session = Session::new(&mut self.types, pending, self.position, frames, self.max_depth);
        let result = match session.next_type() {
            Ok(Some(id)) => read(&mut session, id).map(Some),
            other => other.map(|_| None),
        };
        match result {
            Ok(value) => {
                (self.pending, self.position) = session.into_pending();
                Ok(value)
            }
            Err(error) => {
                // The stream is out of sync now: drop the partial message.
                let offset = session.offset();
                self.position = session.into_pending().1;
                self.position.message = self.position.consumed;
                Err(error.at_offset(offset))
            }
        }
    }
}
