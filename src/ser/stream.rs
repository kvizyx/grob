//! Serializing straight to gob bytes, against definitions the stream has.
//!
//! Every value is checked against the type expected where it appears.
//! Anything the definitions do not cover (a new field, another type, an
//! unregistered interface name) makes writing fail: with
//! [`WriteError::Mismatch`] for types the encoder inferred, so it infers
//! wider ones, and with a data error for types the user declared. Nothing
//! reaches the stream unless the whole value fits.

use std::fmt;

use serde::ser::{self, Impossible, Serialize};

use super::infer::tuple_variant;
use super::value::ValueSerializer;
use crate::encode::field_delta;
use crate::external::{Marshaler, marshaler};
use crate::types::{Entry, TypeTable};
use crate::wire::{Writer, push_uint};
use crate::{Encoder, Error, Type, TypeId};

/// Why writing failed.
#[derive(Debug)]
pub(super) enum WriteError {
    /// The value does not fit the stream's inferred definitions.
    Mismatch,
    /// The value cannot be encoded.
    Failed(Error),
}

impl WriteError {
    /// Adds a step to the path of a real error.
    fn map(self, step: impl FnOnce(Error) -> Error) -> Self {
        match self {
            WriteError::Failed(error) => WriteError::Failed(step(error)),
            WriteError::Mismatch => WriteError::Mismatch,
        }
    }
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::Mismatch => f.write_str("value does not match the stream's type definitions"),
            WriteError::Failed(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for WriteError {}

impl ser::Error for WriteError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        WriteError::Failed(Error::data(msg.to_string()))
    }
}

impl From<Error> for WriteError {
    fn from(error: Error) -> Self {
        WriteError::Failed(error)
    }
}

type Result<T, E = WriteError> = std::result::Result<T, E>;

/// What a written value turned out to be: struct fields leave zero values
/// out, and elements may not be nil.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Written {
    Value,
    Zero,
    Nil,
}

impl Written {
    #[inline]
    fn zero_if(zero: bool) -> Self {
        if zero { Written::Zero } else { Written::Value }
    }
}

/// Buffers reused across values.
#[derive(Default)]
pub(crate) struct Scratch {
    /// The fields of the structs being written, innermost last.
    fields: Vec<WrittenField>,
    /// For putting a struct's fields in order.
    reorder: Vec<u8>,
}

/// A struct field already written: its index in the definition, and where
/// its value's bytes are.
struct WrittenField {
    index: usize,
    start: usize,
    end: usize,
}

/// The type a value must have where it appears.
#[derive(Clone, Copy)]
enum Expect {
    Type(TypeId),
    /// One raw byte of a `[]byte`.
    Byte,
    /// The bytes of a value that encodes itself: a byte string, a string,
    /// or a sequence of bytes.
    External,
}

/// Writes a top-level value of type `id` in a message body. `declared` says
/// whether the user declared the types, rather than the encoder inferring
/// them.
pub(super) fn write_top<T: Serialize + ?Sized>(
    encoder: &Encoder,
    w: &mut Writer,
    scratch: &mut Scratch,
    (id, declared): (TypeId, bool),
    value: &T,
) -> Result<()> {
    if !encoder.types().is_struct(id) {
        w.uint(0);
    }
    match value.serialize(StreamSerializer { encoder, w, scratch, expect: Expect::Type(id), declared })? {
        Written::Nil => Err(Error::data("cannot encode a nil value").into()),
        Written::Value | Written::Zero => Ok(()),
    }
}

/// A description of type `ty` for error messages, when it has no name.
fn describe(ty: &Type) -> String {
    match ty {
        Type::Array { len, .. } => format!("an array of {len}"),
        Type::Slice { .. } => "a slice".to_owned(),
        Type::Struct { .. } => "a struct".to_owned(),
        Type::Map { .. } => "a map".to_owned(),
        Type::GobEncoder { .. } | Type::BinaryMarshaler { .. } | Type::TextMarshaler { .. } => {
            "a self-encoding type".to_owned()
        }
    }
}

fn basic_name(id: TypeId) -> &'static str {
    match id {
        TypeId::BOOL => "bool",
        TypeId::INT => "int",
        TypeId::UINT => "uint",
        TypeId::FLOAT => "float",
        TypeId::COMPLEX => "complex",
        TypeId::BYTES => "[]byte",
        TypeId::STRING => "string",
        _ => "interface",
    }
}

/// Serializes a value to gob bytes, against the types an encoder defined.
pub(super) struct StreamSerializer<'e> {
    encoder: &'e Encoder,
    w: &'e mut Writer,
    scratch: &'e mut Scratch,
    expect: Expect,
    /// The user declared the types: mismatches are errors to report.
    declared: bool,
}

impl<'e> StreamSerializer<'e> {
    #[inline]
    fn at(&mut self, id: TypeId) -> StreamSerializer<'_> {
        self.with(Expect::Type(id))
    }

    #[inline]
    fn with(&mut self, expect: Expect) -> StreamSerializer<'_> {
        StreamSerializer { encoder: self.encoder, w: self.w, scratch: self.scratch, expect, declared: self.declared }
    }

    #[inline]
    fn types(&self) -> &'e TypeTable {
        self.encoder.types()
    }

    /// The value, a `what`, does not fit the expected type.
    #[cold]
    fn mismatch(&self, what: fmt::Arguments<'_>) -> WriteError {
        if !self.declared {
            return WriteError::Mismatch;
        }
        let expected = match self.expect {
            Expect::Type(id) => match self.types().get(id) {
                Some(entry) if !entry.ty.name().is_empty() => entry.ty.name().to_owned(),
                Some(entry) => describe(&entry.ty),
                None if id.is_basic() => basic_name(id).to_owned(),
                None => format!("type {id}, which is not defined"),
            },
            Expect::Byte => "a byte".to_owned(),
            Expect::External => "bytes".to_owned(),
        };
        WriteError::Failed(Error::data(format!("cannot serialize {what} as {expected}")))
    }

    #[inline]
    fn expect(&self, id: TypeId, what: fmt::Arguments<'_>) -> Result<()> {
        match self.expect {
            Expect::Type(expected) if expected == id => Ok(()),
            _ => Err(self.mismatch(what)),
        }
    }

    /// The definition of the expected type, for a `what`.
    #[inline]
    fn entry(&self, what: fmt::Arguments<'_>) -> Result<&'e Entry> {
        match self.expect {
            Expect::Type(id) => self.types().get(id).map(AsRef::as_ref).ok_or_else(|| self.mismatch(what)),
            Expect::Byte | Expect::External => Err(self.mismatch(what)),
        }
    }

    /// Whether the expected type takes a byte string: a `[]byte`, or a type
    /// that encodes itself.
    #[inline]
    fn wants_bytes(&self) -> bool {
        match self.expect {
            Expect::External | Expect::Type(TypeId::BYTES) => true,
            Expect::Type(id) if !id.is_basic() => self.types().get(id).is_some_and(|entry| entry.ty.is_external()),
            Expect::Type(_) | Expect::Byte => false,
        }
    }

    /// Writes a byte string, where the expected type is `id`, `[]byte` or a
    /// type that encodes itself.
    #[inline]
    fn byte_string(self, id: TypeId, bytes: &[u8], what: fmt::Arguments<'_>) -> Result<Written> {
        if !matches!(self.expect, Expect::Type(expected) if expected == id) && !self.wants_bytes() {
            return Err(self.mismatch(what));
        }
        self.w.bytes(bytes);
        Ok(Written::zero_if(bytes.is_empty()))
    }

    /// The expected struct type. Inferred types must be the serde struct
    /// `name`; declared ones only need matching fields.
    #[inline]
    fn struct_entry(&self, name: &str) -> Result<&'e Entry> {
        let entry = self.entry(format_args!("struct {name}"))?;
        match &entry.ty {
            Type::Struct { name: defined, .. } if self.declared || defined == name => Ok(entry),
            _ => Err(self.mismatch(format_args!("struct {name}"))),
        }
    }

    /// Writes the header of an interface value of concrete type `name`;
    /// returns the concrete type and where the length-prefixed value starts.
    fn interface_header(&mut self, name: &str) -> Result<(TypeId, usize)> {
        self.expect(TypeId::INTERFACE, format_args!("interface value {name}"))?;
        let Some(id) = self.encoder.registered(name) else {
            if self.declared {
                return Err(Error::data(format!("type not registered for interface: {name}")).into());
            }
            return Err(WriteError::Mismatch);
        };
        self.w.bytes(name.as_bytes());
        self.w.int(id.get().into());
        let start = self.w.len();
        if !self.types().is_struct(id) {
            self.w.uint(0);
        }
        Ok((id, start))
    }

    /// Writes an element, which gob always sends, so it may not be nil.
    #[inline]
    fn element<T: Serialize + ?Sized>(&mut self, id: TypeId, value: &T, what: &str) -> Result<()> {
        match value.serialize(self.at(id))? {
            Written::Nil => Err(Error::data(format!("cannot encode a nil {what}")).into()),
            Written::Value | Written::Zero => Ok(()),
        }
    }
}

impl<'e> ser::Serializer for StreamSerializer<'e> {
    type Ok = Written;
    type Error = WriteError;
    type SerializeSeq = StreamSeq<'e>;
    type SerializeTuple = StreamSeq<'e>;
    type SerializeTupleStruct = StreamSeq<'e>;
    type SerializeTupleVariant = Impossible<Written, WriteError>;
    type SerializeMap = StreamMap<'e>;
    type SerializeStruct = StreamStruct<'e>;
    type SerializeStructVariant = StreamStruct<'e>;

    fn serialize_bool(self, v: bool) -> Result<Written> {
        self.expect(TypeId::BOOL, format_args!("a bool"))?;
        self.w.bool(v);
        Ok(Written::zero_if(!v))
    }

    fn serialize_i8(self, v: i8) -> Result<Written> {
        self.serialize_i64(v.into())
    }

    fn serialize_i16(self, v: i16) -> Result<Written> {
        self.serialize_i64(v.into())
    }

    fn serialize_i32(self, v: i32) -> Result<Written> {
        self.serialize_i64(v.into())
    }

    /// Signed integers also fit `uint` types when they are not negative.
    #[inline]
    fn serialize_i64(self, v: i64) -> Result<Written> {
        match self.expect {
            Expect::Type(TypeId::INT) => self.w.int(v),
            Expect::Type(TypeId::UINT) if v >= 0 => self.w.uint(v.cast_unsigned()),
            _ => return Err(self.mismatch(format_args!("integer {v}"))),
        }
        Ok(Written::zero_if(v == 0))
    }

    fn serialize_i128(self, v: i128) -> Result<Written> {
        self.serialize_i64(i64::try_from(v).map_err(|_| Error::data(format!("{v} overflows int64")))?)
    }

    #[inline]
    fn serialize_u8(self, v: u8) -> Result<Written> {
        if let Expect::Byte = self.expect {
            self.w.raw(&[v]);
            return Ok(Written::zero_if(v == 0));
        }
        self.serialize_u64(v.into())
    }

    fn serialize_u16(self, v: u16) -> Result<Written> {
        self.serialize_u64(v.into())
    }

    fn serialize_u32(self, v: u32) -> Result<Written> {
        self.serialize_u64(v.into())
    }

    /// Unsigned integers also fit `int` types when they are small enough.
    #[inline]
    fn serialize_u64(self, v: u64) -> Result<Written> {
        match self.expect {
            Expect::Type(TypeId::UINT) => self.w.uint(v),
            Expect::Type(TypeId::INT) if v <= i64::MAX.cast_unsigned() => self.w.int(v.cast_signed()),
            _ => return Err(self.mismatch(format_args!("unsigned integer {v}"))),
        }
        Ok(Written::zero_if(v == 0))
    }

    fn serialize_u128(self, v: u128) -> Result<Written> {
        self.serialize_u64(u64::try_from(v).map_err(|_| Error::data(format!("{v} overflows uint64")))?)
    }

    fn serialize_f32(self, v: f32) -> Result<Written> {
        self.serialize_f64(v.into())
    }

    #[inline]
    fn serialize_f64(self, v: f64) -> Result<Written> {
        self.expect(TypeId::FLOAT, format_args!("a float"))?;
        self.w.float(v);
        Ok(Written::zero_if(v == 0.0))
    }

    fn serialize_char(self, v: char) -> Result<Written> {
        self.serialize_str(v.encode_utf8(&mut [0; 4]))
    }

    #[inline]
    fn serialize_str(self, v: &str) -> Result<Written> {
        self.byte_string(TypeId::STRING, v.as_bytes(), format_args!("a string"))
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<Written> {
        self.byte_string(TypeId::BYTES, v, format_args!("bytes"))
    }

    fn serialize_none(self) -> Result<Written> {
        Ok(Written::Nil)
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Written> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Written> {
        self.serialize_unit_struct("")
    }

    fn serialize_unit_struct(self, name: &'static str) -> Result<Written> {
        self.struct_entry(name)?;
        self.w.uint(0);
        Ok(Written::Value)
    }

    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> Result<Written> {
        self.serialize_str(variant)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(mut self, name: &'static str, value: &T) -> Result<Written> {
        let (fits, what, zero): (fn(&Type) -> bool, _, _) = match marshaler(name) {
            Some((Marshaler::Binary, zero)) => {
                (|ty| matches!(ty, Type::BinaryMarshaler { .. }), "BinaryMarshaler", zero)
            }
            Some((Marshaler::Gob, zero)) => (|ty| matches!(ty, Type::GobEncoder { .. }), "GobEncoder", zero),
            None => return value.serialize(self),
        };
        if !fits(&self.entry(format_args!("a {what} value"))?.ty) {
            return Err(self.mismatch(format_args!("a {what} value")));
        }
        self.expect = Expect::External;
        // Zero values still have their bytes, for when they are elements.
        match value.serialize(self)? {
            Written::Value if zero => Ok(Written::Zero),
            written => Ok(written),
        }
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        mut self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Written> {
        let (id, start) = self.interface_header(variant)?;
        if value.serialize(self.at(id))? == Written::Nil {
            return Err(Error::data(format!("cannot encode a nil pointer inside interface value {variant}")).into());
        }
        self.w.prefix_length(start);
        Ok(Written::Value)
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<StreamSeq<'e>> {
        let elem = match self.expect {
            Expect::External | Expect::Type(TypeId::BYTES) => Elem::Byte,
            _ => match self.entry(format_args!("a sequence"))?.ty {
                Type::Slice { elem, .. } => Elem::Type(elem),
                // A sequence of the right length fills an array.
                Type::Array { elem, len: defined, .. } if len == Some(defined) => Elem::Type(elem),
                ref ty if ty.is_external() => Elem::Byte,
                _ => return Err(self.mismatch(format_args!("a sequence"))),
            },
        };
        // A `[]byte` is prefixed by its length in bytes, which is also its
        // element count.
        // Every element takes at least a byte.
        if let Some(len) = len {
            self.w.reserve(len);
        }
        let count_at = match (elem, len) {
            (Elem::Type(_), Some(len)) => {
                self.w.uint(len as u64);
                None
            }
            _ => Some(self.w.len()),
        };
        Ok(StreamSeq { serializer: self, elem, count: 0, count_at })
    }

    /// Tuples fill arrays of their length, slices and byte strings.
    fn serialize_tuple(self, len: usize) -> Result<StreamSeq<'e>> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<StreamSeq<'e>> {
        self.serialize_tuple(len)
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Impossible<Written, WriteError>> {
        Err(tuple_variant(variant).into())
    }

    fn serialize_map(self, len: Option<usize>) -> Result<StreamMap<'e>> {
        let Type::Map { key, elem, .. } = self.entry(format_args!("a map"))?.ty else {
            return Err(self.mismatch(format_args!("a map")));
        };
        let count_at = match len {
            Some(len) => {
                self.w.uint(len as u64);
                None
            }
            None => Some(self.w.len()),
        };
        Ok(StreamMap { serializer: self, key, elem, count: 0, count_at })
    }

    fn serialize_struct(self, name: &'static str, _: usize) -> Result<StreamStruct<'e>> {
        let entry = self.struct_entry(name)?;
        Ok(StreamStruct::new(self, entry, None))
    }

    fn serialize_struct_variant(
        mut self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<StreamStruct<'e>> {
        let (id, start) = self.interface_header(variant)?;
        self.expect = Expect::Type(id);
        let entry = self.struct_entry(variant)?;
        Ok(StreamStruct::new(self, entry, Some(start)))
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

/// The elements of a sequence.
#[derive(Clone, Copy)]
enum Elem {
    Type(TypeId),
    /// The raw bytes of a byte string.
    Byte,
}

/// A sequence, tuple or array.
pub(super) struct StreamSeq<'e> {
    serializer: StreamSerializer<'e>,
    elem: Elem,
    count: usize,
    /// Where the element count goes once it is known: for sequences of
    /// unknown length, and for byte strings.
    count_at: Option<usize>,
}

impl StreamSeq<'_> {
    #[inline]
    fn push<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        let index = self.count;
        self.count += 1;
        match self.elem {
            Elem::Type(id) => self.serializer.element(id, value, "element").map_err(|e| e.map(|e| e.in_element(index))),
            Elem::Byte => value.serialize(self.serializer.with(Expect::Byte)).map(drop),
        }
    }

    fn finish(self) -> Written {
        if let Some(start) = self.count_at {
            self.serializer.w.prefix_uint(start, self.count as u64);
        }
        Written::zero_if(self.count == 0)
    }
}

impl ser::SerializeSeq for StreamSeq<'_> {
    type Ok = Written;
    type Error = WriteError;

    #[inline]
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.push(value)
    }

    fn end(self) -> Result<Written> {
        Ok(self.finish())
    }
}

impl ser::SerializeTuple for StreamSeq<'_> {
    type Ok = Written;
    type Error = WriteError;

    #[inline]
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.push(value)
    }

    fn end(self) -> Result<Written> {
        Ok(self.finish())
    }
}

impl ser::SerializeTupleStruct for StreamSeq<'_> {
    type Ok = Written;
    type Error = WriteError;

    #[inline]
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.push(value)
    }

    fn end(self) -> Result<Written> {
        Ok(self.finish())
    }
}

/// A map.
pub(super) struct StreamMap<'e> {
    serializer: StreamSerializer<'e>,
    key: TypeId,
    elem: TypeId,
    count: usize,
    count_at: Option<usize>,
}

impl ser::SerializeMap for StreamMap<'_> {
    type Ok = Written;
    type Error = WriteError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<()> {
        self.count += 1;
        let index = self.count - 1;
        self.serializer.element(self.key, key, "map key").map_err(|e| e.map(|e| e.in_entry(index)))
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        let index = self.count - 1;
        self.serializer.element(self.elem, value, "map value").map_err(|e| e.map(|e| e.in_entry(index)))
    }

    fn end(self) -> Result<Written> {
        if let Some(start) = self.count_at {
            self.serializer.w.prefix_uint(start, self.count as u64);
        }
        Ok(Written::zero_if(self.count == 0))
    }
}

/// A struct. Fields are written as they come, with their field deltas,
/// while they come in definition order; otherwise they are put in order at
/// the end.
pub(super) struct StreamStruct<'e> {
    serializer: StreamSerializer<'e>,
    entry: &'e Entry,
    /// This struct's fields in `scratch.fields` start here.
    base: usize,
    /// Where the first field starts.
    start: usize,
    /// The last field written, while fields come in order.
    last: Option<usize>,
    in_order: bool,
    /// Where to start looking for the next field's name.
    guess: usize,
    /// For enum struct variants: where the interface's value starts.
    interface_start: Option<usize>,
}

impl<'e> StreamStruct<'e> {
    fn new(serializer: StreamSerializer<'e>, entry: &'e Entry, interface_start: Option<usize>) -> Self {
        let base = serializer.scratch.fields.len();
        let start = serializer.w.len();
        Self { serializer, entry, base, start, last: None, in_order: true, guess: 0, interface_start }
    }

    #[inline]
    fn index_of(&mut self, name: &str) -> Option<usize> {
        let names = &self.entry.field_names;
        let guess = self.guess.min(names.len());
        let index = names[guess..]
            .iter()
            .position(|n| **n == *name)
            .map(|i| i + guess)
            .or_else(|| names[..guess].iter().position(|n| **n == *name))?;
        self.guess = index + 1;
        Some(index)
    }

    fn field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        self.write_field(key, value).map_err(|e| e.map(|e| e.in_field(key)))
    }

    #[inline]
    fn write_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        let Some(index) = self.index_of(key) else {
            // Not in the definition: fine if it is left out anyway.
            if value.serialize(ValueSerializer)?.is_zero() {
                return Ok(());
            }
            if self.serializer.declared {
                let name = self.entry.ty.name();
                return Err(Error::data(format!("the declared type {name} has no field {key}")).into());
            }
            return Err(WriteError::Mismatch);
        };
        let Type::Struct { fields, .. } = &self.entry.ty else { unreachable!("struct writers hold struct types") };
        let delta_start = self.serializer.w.len();
        if self.in_order && self.last.is_none_or(|last| index > last) {
            self.serializer.w.uint(field_delta(self.last, index));
        } else {
            self.in_order = false;
        }
        let start = self.serializer.w.len();
        match value.serialize(self.serializer.at(fields[index].id))? {
            Written::Value => {
                if self.in_order {
                    self.last = Some(index);
                }
                let end = self.serializer.w.len();
                self.serializer.scratch.fields.push(WrittenField { index, start, end });
            }
            Written::Zero | Written::Nil => self.serializer.w.truncate(delta_start),
        }
        Ok(())
    }

    fn finish(self) -> Result<Written> {
        let Self { serializer, base, start, in_order, interface_start, .. } = self;
        if !in_order {
            reorder(serializer.w, serializer.scratch, base, start)?;
        }
        serializer.scratch.fields.truncate(base);
        serializer.w.uint(0);
        if let Some(start) = interface_start {
            serializer.w.prefix_length(start);
        }
        Ok(Written::Value)
    }
}

/// Rewrites a struct's fields, written from `start` on, in definition order
/// with new field deltas.
#[cold]
fn reorder(w: &mut Writer, scratch: &mut Scratch, base: usize, start: usize) -> Result<()> {
    let fields = &mut scratch.fields[base..];
    fields.sort_unstable_by_key(|field| field.index);
    if fields.windows(2).any(|pair| pair[0].index == pair[1].index) {
        return Err(Error::data("struct field given twice").into());
    }
    let mut out = std::mem::take(&mut scratch.reorder);
    out.clear();
    let mut last = None;
    for field in fields.iter() {
        push_uint(&mut out, field_delta(last, field.index));
        last = Some(field.index);
        out.extend_from_slice(&w.since(field.start)[..field.end - field.start]);
    }
    w.truncate(start);
    w.raw(&out);
    scratch.reorder = out;
    Ok(())
}

impl ser::SerializeStruct for StreamStruct<'_> {
    type Ok = Written;
    type Error = WriteError;

    #[inline]
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        self.field(key, value)
    }

    fn end(self) -> Result<Written> {
        self.finish()
    }
}

impl ser::SerializeStructVariant for StreamStruct<'_> {
    type Ok = Written;
    type Error = WriteError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        self.field(key, value)
    }

    fn end(self) -> Result<Written> {
        self.finish()
    }
}
