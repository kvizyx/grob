//! The protocol layer of decoding: messages, type definitions, interface
//! headers and skipping values.

use std::borrow::Cow;
use std::sync::Arc;

use super::{Frame, Frames, Position, TOO_BIG};
use crate::types::{Entry, Type, TypeTable, WIRE_TYPE};
use crate::wire::{read_bytes, read_uint, uint_to_float, uint_to_int};
use crate::{Error, TypeId};

/// Longest interface type name accepted, as in Go (decode.go, 1024 bytes).
/// Real names are package paths plus type names, far shorter.
const MAX_INTERFACE_NAME: usize = 1024;

/// The message being read. A value may continue into the next message: Go
/// flushes a message whenever an interface value needs a new type definition.
pub(crate) struct Input<'a> {
    buf: Cow<'a, [u8]>,
    pos: usize,
}

/// Bytes read from the input: borrowed from the stream itself, or from a
/// message the decoder read into its own buffer.
#[cfg(feature = "serde")]
#[derive(Clone, Copy)]
pub(crate) enum Bytes<'a, 'b> {
    Stream(&'a [u8]),
    Message(&'b [u8]),
}

#[cfg(feature = "serde")]
impl<'a> Input<'a> {
    /// A byte string, borrowed from the stream when the current message is
    /// borrowed from it.
    #[inline]
    pub fn bytes_ref(&mut self) -> Result<Bytes<'a, '_>, Error> {
        let (bytes, size) = read_bytes(&self.buf[self.pos..])?;
        let range = self.pos + size - bytes.len()..self.pos + size;
        self.pos += size;
        Ok(match self.buf {
            Cow::Borrowed(stream) => Bytes::Stream(&stream[range]),
            Cow::Owned(ref message) => Bytes::Message(&message[range]),
        })
    }
}

impl Input<'_> {
    #[inline]
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    #[inline]
    pub fn uint(&mut self) -> Result<u64, Error> {
        let (value, size) = read_uint(&self.buf[self.pos..])?;
        self.pos += size;
        Ok(value)
    }

    #[inline]
    pub fn int(&mut self) -> Result<i64, Error> {
        self.uint().map(uint_to_int)
    }

    #[inline]
    pub fn float(&mut self) -> Result<f64, Error> {
        self.uint().map(uint_to_float)
    }

    #[inline]
    pub fn bool(&mut self) -> Result<bool, Error> {
        self.uint().map(|value| value != 0)
    }

    #[inline]
    pub fn bytes(&mut self) -> Result<&[u8], Error> {
        let (bytes, size) = read_bytes(&self.buf[self.pos..])?;
        self.pos += size;
        Ok(bytes)
    }

    /// A string; invalid UTF-8 is replaced.
    pub fn string(&mut self) -> Result<Cow<'_, str>, Error> {
        self.bytes().map(String::from_utf8_lossy)
    }

    /// A collection length, checked against the decoder's sanity limit.
    #[inline]
    pub fn count(&mut self) -> Result<usize, Error> {
        let count = self.uint()?;
        match usize::try_from(count) {
            Ok(size) if count < TOO_BIG => Ok(size),
            _ => Err(Error::limit(format!("collection of {count} elements is too large"))),
        }
    }

    fn take(&mut self, count: usize) -> Result<&[u8], Error> {
        let bytes = self.buf.get(self.pos..self.pos + count).ok_or_else(Error::eof)?;
        self.pos += count;
        Ok(bytes)
    }
}

/// One decoding call: the decoder's types, the input and where more
/// messages come from.
pub(crate) struct Session<'d, 'a> {
    types: &'d mut TypeTable,
    pub input: Input<'a>,
    frames: &'d mut dyn Frames<'a>,
    position: Position,
    max_depth: usize,
    /// How many values started being read, so code handing a value to a
    /// visitor it doesn't control can tell whether the value was consumed.
    #[cfg_attr(not(feature = "serde"), expect(dead_code, reason = "used by the serde deserializer"))]
    pub reads: u64,
}

impl<'d, 'a> Session<'d, 'a> {
    /// A session reading `pending`, the rest of a message, at `position`,
    /// then messages from `frames`.
    pub fn new(
        types: &'d mut TypeTable,
        pending: Vec<u8>,
        position: Position,
        frames: &'d mut dyn Frames<'a>,
        max_depth: usize,
    ) -> Self {
        let input = Input { buf: Cow::Owned(pending), pos: 0 };
        Self { types, input, frames, position, max_depth, reads: 0 }
    }

    /// The unread rest of the current message, to keep for the next call,
    /// and where it is in the stream.
    pub fn into_pending(self) -> (Vec<u8>, Position) {
        let Input { buf, pos } = self.input;
        let position = Position { message: self.position.message + pos as u64, ..self.position };
        let rest = if pos == buf.len() { Vec::new() } else { buf[pos..].to_vec() };
        (rest, position)
    }

    /// Where reading stands in the stream.
    pub fn offset(&self) -> u64 {
        self.position.message + self.input.pos as u64
    }

    /// The type `id`, which must be known.
    #[inline]
    pub fn entry(&self, id: TypeId) -> Result<Arc<Entry>, Error> {
        self.types.get(id).cloned().ok_or_else(|| Error::syntax(format!("unknown type id {id}")))
    }

    #[inline]
    pub fn check_depth(&self, depth: usize) -> Result<(), Error> {
        if depth > self.max_depth {
            return Err(Error::limit(format!("value nested deeper than {} levels", self.max_depth)));
        }
        Ok(())
    }

    /// Top-level and interface values that are not structs start with a
    /// zero field delta.
    #[inline]
    pub fn singleton_delta(&mut self, id: TypeId) -> Result<(), Error> {
        if !self.types.is_struct(id) && self.input.uint()? != 0 {
            return Err(Error::syntax("non-zero field delta before a single value"));
        }
        Ok(())
    }

    /// Reads type definitions until a value's type id shows up; `None` at a
    /// clean end of stream.
    pub fn next_type(&mut self) -> Result<Option<TypeId>, Error> {
        self.type_sequence(false)
    }

    /// Registers type definitions until a value's type id shows up
    /// (decoder.go decodeTypeSequence).
    fn type_sequence(&mut self, in_interface: bool) -> Result<Option<TypeId>, Error> {
        let mut first = true;
        loop {
            if self.input.remaining() == 0 {
                match self.frames.next_frame()? {
                    Some(Frame { data, header }) => {
                        self.position.message = self.position.consumed + header as u64;
                        self.position.consumed = self.position.message + data.len() as u64;
                        self.input = Input { buf: data, pos: 0 };
                    }
                    None if first && !in_interface => return Ok(None),
                    None => return Err(Error::eof()),
                }
            }
            let id = self.input.int()?;
            let type_id = i32::try_from(id.unsigned_abs())
                .map(TypeId::new)
                .map_err(|_| Error::syntax(format!("type id {id} out of range")))?;
            if id >= 0 {
                return Ok(Some(type_id));
            }
            self.define(type_id)?;
            // Inside an interface value, a definition may be followed by the
            // length of the chunk Go flushed after it.
            if self.input.remaining() > 0 {
                if !in_interface {
                    return Err(Error::syntax("unexpected data after a type definition"));
                }
                self.input.uint()?;
            }
            first = false;
        }
    }

    fn define(&mut self, id: TypeId) -> Result<(), Error> {
        if id < TypeId::FIRST_USER {
            return Err(Error::syntax(format!("type definition uses reserved id {id}")));
        }
        if self.types.contains(id) {
            return Err(Error::syntax(format!("type {id} defined twice")));
        }
        let definition = self.decode_value(WIRE_TYPE, 0)?;
        self.types.insert(id, Type::from_wire(&definition)?);
        Ok(())
    }

    /// Reads an interface header: the name of the concrete type (empty for
    /// nil), its type id, possibly after type definitions, and the value's
    /// length. Leaves the input at the concrete value, past its singleton
    /// delta. `None` for nil.
    pub fn interface_header(&mut self) -> Result<Option<(String, TypeId)>, Error> {
        let length = self.input.uint()?;
        if length == 0 {
            return Ok(None);
        }
        let length = usize::try_from(length)
            .ok()
            .filter(|length| *length <= MAX_INTERFACE_NAME)
            .ok_or_else(|| Error::limit(format!("interface type name of {length} bytes is too long")))?;
        let name = String::from_utf8_lossy(self.input.take(length)?).into_owned();
        let id = self.type_sequence(true)?.ok_or_else(Error::eof)?;
        // The value's length only matters for skipping it without its type.
        self.input.uint()?;
        self.singleton_delta(id)?;
        Ok(Some((name, id)))
    }

    /// Reads the field delta of a struct's next field; returns the field's
    /// index, or `None` at the end of the struct.
    #[inline]
    pub fn next_field(&mut self, last: &mut Option<usize>, count: usize) -> Result<Option<usize>, Error> {
        let delta = self.input.uint()?;
        if delta == 0 {
            return Ok(None);
        }
        let index = match *last {
            Some(last) => usize::try_from(delta).ok().and_then(|delta| last.checked_add(delta)),
            None => usize::try_from(delta - 1).ok(),
        };
        match index {
            Some(index) if index < count => {
                *last = Some(index);
                Ok(Some(index))
            }
            _ => Err(Error::syntax("struct field number out of range")),
        }
    }

    /// Reads past a value of type `id` without keeping it, checking it the
    /// same way as decoding it.
    #[cfg(feature = "serde")]
    pub fn skip_value(&mut self, id: TypeId, depth: usize) -> Result<(), Error> {
        self.check_depth(depth)?;
        match id {
            TypeId::BOOL | TypeId::INT | TypeId::UINT | TypeId::FLOAT => self.input.uint().map(drop),
            TypeId::COMPLEX => self.input.uint().and_then(|_| self.input.uint()).map(drop),
            TypeId::BYTES | TypeId::STRING => self.input.bytes().map(drop),
            TypeId::INTERFACE => match self.interface_header()? {
                Some((_, id)) => self.skip_value(id, depth + 1),
                None => Ok(()),
            },
            _ => {
                let entry = self.entry(id)?;
                match &entry.ty {
                    Type::Struct { fields, .. } => {
                        let mut last = None;
                        while let Some(index) = self.next_field(&mut last, fields.len())? {
                            self.skip_value(fields[index].id, depth + 1)?;
                        }
                        Ok(())
                    }
                    Type::Array { elem, len, .. } => {
                        let count = self.array_count(*len)?;
                        (0..count).try_for_each(|_| self.skip_value(*elem, depth + 1))
                    }
                    Type::Slice { elem, .. } => {
                        let count = self.input.count()?;
                        (0..count).try_for_each(|_| self.skip_value(*elem, depth + 1))
                    }
                    Type::Map { key, elem, .. } => {
                        let count = self.input.count()?;
                        (0..count).try_for_each(|_| {
                            self.skip_value(*key, depth + 1)?;
                            self.skip_value(*elem, depth + 1)
                        })
                    }
                    Type::GobEncoder { .. } | Type::BinaryMarshaler { .. } | Type::TextMarshaler { .. } => {
                        self.input.bytes().map(drop)
                    }
                }
            }
        }
    }

    /// Reads an array's element count, which must match its type.
    pub fn array_count(&mut self, len: usize) -> Result<usize, Error> {
        let count = self.input.count()?;
        if count != len {
            return Err(Error::syntax(format!("array of {len} elements holds {count}")));
        }
        Ok(count)
    }
}
