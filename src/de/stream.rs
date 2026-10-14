//! Deserializing straight from a gob stream, without building [`Value`]s.
//!
//! It mirrors the [`Value`] deserializer case by case, so a type decodes the
//! same either way. Every value handed to a visitor is consumed exactly once,
//! even when the visitor ignores it, so reading never loses its place.
//!
//! [`Value`]: crate::Value

use std::sync::Arc;

use serde::de::value::{BorrowedStrDeserializer, SeqDeserializer, StrDeserializer, U64Deserializer};
use serde::de::{self, Deserialize, DeserializeSeed, Unexpected, Visitor};

use super::zero::{Zero, ZeroFill};
use crate::decode::{Bytes, Session};
use crate::types::{Entry, Type};
use crate::{Error, TypeId};

/// Deserializes the top-level value of type `id` the session is at.
pub(crate) fn deserialize_top<'a, T: Deserialize<'a>>(session: &mut Session<'_, 'a>, id: TypeId) -> Result<T, Error> {
    session.singleton_delta(id)?;
    guarded(session, Pos::Start(id), 0, |de| T::deserialize(de))
}

/// Where a deserializer stands in its value.
enum Pos {
    /// At the start of a value of type `id`; for interfaces, at the header.
    Start(TypeId),
    /// Past an interface header, at the concrete value of type `id`.
    Interface(String, TypeId),
    /// At a nil interface, already read.
    Nil,
}

/// Runs `read` with a deserializer for the value at `pos`, then skips the
/// value if `read` never looked at it.
fn guarded<'d, 'a, T>(
    session: &mut Session<'d, 'a>,
    pos: Pos,
    depth: usize,
    read: impl FnOnce(Deserializer<'_, 'd, 'a>) -> Result<T, Error>,
) -> Result<T, Error> {
    let reads = session.reads;
    let unread = match &pos {
        Pos::Start(id) => Some((*id, depth)),
        Pos::Interface(_, id) => Some((*id, depth + 1)),
        Pos::Nil => None,
    };
    let value = read(Deserializer { session: &mut *session, pos, depth })?;
    if session.reads == reads
        && let Some((id, depth)) = unread
    {
        session.skip_value(id, depth)?;
    }
    Ok(value)
}

/// Deserializes one value from a gob stream.
pub(crate) struct Deserializer<'s, 'd, 'a> {
    session: &'s mut Session<'d, 'a>,
    pos: Pos,
    depth: usize,
}

impl<'s, 'd, 'a> Deserializer<'s, 'd, 'a> {
    /// Marks the value as read, checks the nesting and reads an interface's
    /// header.
    #[inline]
    fn begin(&mut self) -> Result<(), Error> {
        self.session.reads += 1;
        self.session.check_depth(self.depth)?;
        if let Pos::Start(TypeId::INTERFACE) = self.pos {
            self.pos = match self.session.interface_header()? {
                Some((name, id)) => Pos::Interface(name, id),
                None => Pos::Nil,
            };
        }
        Ok(())
    }

    /// Reads past the value.
    #[inline]
    fn skip(self) -> Result<(), Error> {
        match self.pos {
            Pos::Start(id) => self.session.skip_value(id, self.depth),
            Pos::Interface(_, id) => self.session.skip_value(id, self.depth + 1),
            Pos::Nil => Ok(()),
        }
    }

    /// The concrete value of an interface.
    #[inline]
    fn concrete(self, id: TypeId) -> Deserializer<'s, 'd, 'a> {
        Deserializer { session: self.session, pos: Pos::Start(id), depth: self.depth + 1 }
    }

    /// Hands the value to `read`, skipping it if `read` does not look at it.
    fn hand_over<T>(self, read: impl FnOnce(Deserializer<'_, 'd, 'a>) -> Result<T, Error>) -> Result<T, Error> {
        guarded(self.session, self.pos, self.depth, read)
    }

    fn unexpected(&self) -> Unexpected<'static> {
        match self.pos {
            Pos::Nil => Unexpected::Option,
            Pos::Interface(..) => Unexpected::Other("interface value"),
            Pos::Start(TypeId::BOOL) => Unexpected::Other("bool"),
            Pos::Start(TypeId::INT | TypeId::UINT) => Unexpected::Other("integer"),
            Pos::Start(TypeId::FLOAT) => Unexpected::Other("float"),
            Pos::Start(TypeId::STRING) => Unexpected::Other("string"),
            Pos::Start(TypeId::BYTES) => Unexpected::Other("bytes"),
            Pos::Start(id) => {
                if self.session.entry(id).is_ok_and(|entry| matches!(entry.ty, Type::Struct { .. } | Type::Map { .. }))
                {
                    Unexpected::Map
                } else {
                    Unexpected::Seq
                }
            }
        }
    }

    /// Whether values of type `id` are byte strings on the wire.
    #[inline]
    fn is_bytes(&self, id: TypeId) -> bool {
        matches!(id, TypeId::BYTES | TypeId::STRING)
            || (!id.is_basic() && self.session.entry(id).is_ok_and(|entry| entry.ty.is_external()))
    }

    /// `deserialize_any`, once [`begin`](Self::begin) ran.
    fn any<V: Visitor<'a>>(self, visitor: V) -> Result<V::Value, Error> {
        let id = match self.pos {
            Pos::Nil => return visitor.visit_none(),
            Pos::Interface(_, id) => return de::Deserializer::deserialize_any(self.concrete(id), visitor),
            Pos::Start(id) => id,
        };
        let input = &mut self.session.input;
        match id {
            TypeId::BOOL => visitor.visit_bool(input.bool()?),
            TypeId::INT => visitor.visit_i64(input.int()?),
            TypeId::UINT => visitor.visit_u64(input.uint()?),
            TypeId::FLOAT => visitor.visit_f64(input.float()?),
            TypeId::COMPLEX => {
                let parts = [input.float()?, input.float()?];
                visitor.visit_seq(SeqDeserializer::<_, Error>::new(parts.into_iter()))
            }
            TypeId::STRING => visit_string(input.bytes_ref()?, visitor),
            TypeId::BYTES => visit_bytes(input.bytes_ref()?, visitor),
            _ => {
                let entry = self.session.entry(id)?;
                match &entry.ty {
                    Type::Struct { .. } => self.visit_fields(Source::Struct { entry, last: None }, &[], visitor),
                    Type::Array { elem, len, .. } => {
                        let count = self.session.array_count(*len)?;
                        self.visit_seq(*elem, count, visitor)
                    }
                    Type::Slice { elem, .. } => {
                        let count = self.session.input.count()?;
                        self.visit_seq(*elem, count, visitor)
                    }
                    Type::Map { key, elem, .. } => {
                        let count = self.session.input.count()?;
                        let mut access = MapAccess {
                            session: self.session,
                            key: *key,
                            elem: *elem,
                            next: 0,
                            remaining: count,
                            value_pending: false,
                            depth: self.depth + 1,
                        };
                        let value = visitor.visit_map(&mut access)?;
                        access.finish()?;
                        Ok(value)
                    }
                    Type::GobEncoder { .. } | Type::BinaryMarshaler { .. } | Type::TextMarshaler { .. } => {
                        visit_bytes(self.session.input.bytes_ref()?, visitor)
                    }
                }
            }
        }
    }

    fn visit_seq<V: Visitor<'a>>(self, elem: TypeId, count: usize, visitor: V) -> Result<V::Value, Error> {
        let mut access = SeqAccess { session: self.session, elem, next: 0, remaining: count, depth: self.depth + 1 };
        let value = visitor.visit_seq(&mut access)?;
        access.finish()?;
        Ok(value)
    }

    fn visit_fields<V: Visitor<'a>>(
        self,
        source: Source,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        let mut access = StructAccess {
            session: self.session,
            source,
            done: false,
            fill: ZeroFill::new(fields),
            pending: None,
            depth: self.depth + 1,
        };
        let value = visitor.visit_map(&mut access)?;
        access.finish()?;
        Ok(value)
    }

    /// Reads an integer directly when the wire has one.
    #[inline]
    fn integer<V: Visitor<'a>>(mut self, visitor: V) -> Result<V::Value, Error> {
        match self.pos {
            Pos::Start(TypeId::INT) => {
                self.begin()?;
                visitor.visit_i64(self.session.input.int()?)
            }
            Pos::Start(TypeId::UINT) => {
                self.begin()?;
                visitor.visit_u64(self.session.input.uint()?)
            }
            _ => de::Deserializer::deserialize_any(self, visitor),
        }
    }

    /// Reads a float directly when the wire has one.
    #[inline]
    fn float<V: Visitor<'a>>(mut self, visitor: V) -> Result<V::Value, Error> {
        if let Pos::Start(TypeId::FLOAT) = self.pos {
            self.begin()?;
            return visitor.visit_f64(self.session.input.float()?);
        }
        de::Deserializer::deserialize_any(self, visitor)
    }
}

/// Visits a gob string, borrowed from the stream when possible. Invalid
/// UTF-8 is replaced, in a string of its own.
fn visit_string<'a, V: Visitor<'a>>(bytes: Bytes<'a, '_>, visitor: V) -> Result<V::Value, Error> {
    match bytes {
        Bytes::Stream(bytes) => match std::str::from_utf8(bytes) {
            Ok(string) => visitor.visit_borrowed_str(string),
            Err(_) => visitor.visit_string(String::from_utf8_lossy(bytes).into_owned()),
        },
        Bytes::Message(bytes) => match std::str::from_utf8(bytes) {
            Ok(string) => visitor.visit_str(string),
            Err(_) => visitor.visit_string(String::from_utf8_lossy(bytes).into_owned()),
        },
    }
}

/// Visits gob bytes, borrowed from the stream when possible.
fn visit_bytes<'a, V: Visitor<'a>>(bytes: Bytes<'a, '_>, visitor: V) -> Result<V::Value, Error> {
    match bytes {
        Bytes::Stream(bytes) => visitor.visit_borrowed_bytes(bytes),
        Bytes::Message(bytes) => visitor.visit_bytes(bytes),
    }
}

impl<'de> de::Deserializer<'de> for Deserializer<'_, '_, 'de> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(mut self, visitor: V) -> Result<V::Value, Error> {
        self.begin()?;
        self.any(visitor)
    }

    fn deserialize_bool<V: Visitor<'de>>(mut self, visitor: V) -> Result<V::Value, Error> {
        if let Pos::Start(TypeId::BOOL) = self.pos {
            self.begin()?;
            return visitor.visit_bool(self.session.input.bool()?);
        }
        self.deserialize_any(visitor)
    }

    fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.integer(visitor)
    }

    fn deserialize_i16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.integer(visitor)
    }

    fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.integer(visitor)
    }

    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.integer(visitor)
    }

    fn deserialize_i128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.integer(visitor)
    }

    fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.integer(visitor)
    }

    fn deserialize_u16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.integer(visitor)
    }

    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.integer(visitor)
    }

    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.integer(visitor)
    }

    fn deserialize_u128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.integer(visitor)
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.float(visitor)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.float(visitor)
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_str<V: Visitor<'de>>(mut self, visitor: V) -> Result<V::Value, Error> {
        if let Pos::Start(TypeId::STRING) = self.pos {
            self.begin()?;
            return visit_string(self.session.input.bytes_ref()?, visitor);
        }
        self.deserialize_any(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(mut self, visitor: V) -> Result<V::Value, Error> {
        self.begin()?;
        match self.pos {
            Pos::Start(TypeId::STRING) => visit_bytes(self.session.input.bytes_ref()?, visitor),
            _ => self.any(visitor),
        }
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(mut self, visitor: V) -> Result<V::Value, Error> {
        self.begin()?;
        match self.pos {
            Pos::Nil => visitor.visit_none(),
            _ => self.hand_over(|de| visitor.visit_some(de)),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(mut self, visitor: V) -> Result<V::Value, Error> {
        self.begin()?;
        self.skip()?;
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _: &'static str, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(mut self, _: &'static str, visitor: V) -> Result<V::Value, Error> {
        self.begin()?;
        self.hand_over(|de| visitor.visit_newtype_struct(de))
    }

    /// Byte strings also deserialize as sequences of bytes.
    fn deserialize_seq<V: Visitor<'de>>(mut self, visitor: V) -> Result<V::Value, Error> {
        self.begin()?;
        match self.pos {
            Pos::Start(id) if self.is_bytes(id) => {
                let bytes = self.session.input.bytes()?;
                visitor.visit_seq(SeqDeserializer::<_, Error>::new(bytes.iter().copied()))
            }
            _ => self.any(visitor),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _: usize, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    /// Structs, and maps with string keys, fill a Rust struct; declared
    /// fields that are missing get their zero value.
    fn deserialize_struct<V: Visitor<'de>>(
        mut self,
        _: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.begin()?;
        if let Pos::Start(id) = self.pos
            && !id.is_basic()
        {
            let entry = self.session.entry(id)?;
            match &entry.ty {
                Type::Struct { .. } => return self.visit_fields(Source::Struct { entry, last: None }, fields, visitor),
                Type::Map { key, elem, .. } => {
                    let (key, elem) = (*key, *elem);
                    let count = self.session.input.count()?;
                    if key != TypeId::STRING && count > 0 {
                        return Err(de::Error::invalid_type(Unexpected::Map, &"a map with string keys"));
                    }
                    return self.visit_fields(Source::Map { elem, remaining: count }, fields, visitor);
                }
                _ => {}
            }
        }
        Err(de::Error::invalid_type(self.unexpected(), &visitor))
    }

    /// Interface values select the variant by their registered name, strings
    /// by variant name and integers by variant index.
    fn deserialize_enum<V: Visitor<'de>>(
        mut self,
        _: &'static str,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.begin()?;
        let (variant, payload) = match self.pos {
            Pos::Interface(name, id) => (Variant::Name(name), Some((id, self.depth + 1))),
            Pos::Start(TypeId::STRING) => (Variant::Name(self.session.input.string()?.into_owned()), None),
            Pos::Start(TypeId::INT) => {
                let index = self.session.input.int()?;
                let index = u64::try_from(index).map_err(|_| Error::data(format!("invalid variant index {index}")))?;
                (Variant::Index(index), None)
            }
            Pos::Start(TypeId::UINT) => (Variant::Index(self.session.input.uint()?), None),
            _ => return Err(de::Error::invalid_type(self.unexpected(), &visitor)),
        };
        visitor.visit_enum(Enum { session: self.session, variant, payload })
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(mut self, visitor: V) -> Result<V::Value, Error> {
        self.begin()?;
        self.skip()?;
        visitor.visit_unit()
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

struct SeqAccess<'s, 'd, 'a> {
    session: &'s mut Session<'d, 'a>,
    elem: TypeId,
    next: usize,
    remaining: usize,
    depth: usize,
}

impl SeqAccess<'_, '_, '_> {
    /// Skips the elements the visitor did not read.
    fn finish(self) -> Result<(), Error> {
        (0..self.remaining).try_for_each(|_| self.session.skip_value(self.elem, self.depth))
    }
}

impl<'de> de::SeqAccess<'de> for SeqAccess<'_, '_, 'de> {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Error> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        let index = self.next;
        self.next += 1;
        guarded(self.session, Pos::Start(self.elem), self.depth, |de| seed.deserialize(de))
            .map(Some)
            .map_err(|e| e.in_element(index))
    }

    fn size_hint(&self) -> Option<usize> {
        // Each element takes at least a byte: don't promise more than that.
        Some(self.remaining.min(self.session.input.remaining()))
    }
}

struct MapAccess<'s, 'd, 'a> {
    session: &'s mut Session<'d, 'a>,
    key: TypeId,
    elem: TypeId,
    next: usize,
    remaining: usize,
    value_pending: bool,
    depth: usize,
}

impl MapAccess<'_, '_, '_> {
    /// Skips the entries the visitor did not read.
    fn finish(self) -> Result<(), Error> {
        if self.value_pending {
            self.session.skip_value(self.elem, self.depth)?;
        }
        (0..self.remaining).try_for_each(|_| {
            self.session.skip_value(self.key, self.depth)?;
            self.session.skip_value(self.elem, self.depth)
        })
    }
}

impl<'de> de::MapAccess<'de> for MapAccess<'_, '_, 'de> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Error> {
        if self.value_pending {
            self.session.skip_value(self.elem, self.depth)?;
            self.value_pending = false;
        }
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        self.next += 1;
        let key = guarded(self.session, Pos::Start(self.key), self.depth, |de| seed.deserialize(de))
            .map_err(|e| e.in_entry(self.next - 1))?;
        self.value_pending = true;
        Ok(Some(key))
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        if !self.value_pending {
            return Err(Error::data("map value requested before its key"));
        }
        self.value_pending = false;
        guarded(self.session, Pos::Start(self.elem), self.depth, |de| seed.deserialize(de))
            .map_err(|e| e.in_entry(self.next - 1))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining.min(self.session.input.remaining()))
    }
}

/// Where struct fields come from: a gob struct, or a map with string keys.
enum Source {
    Struct { entry: Arc<Entry>, last: Option<usize> },
    Map { elem: TypeId, remaining: usize },
}

/// A field's name: an index into the struct definition's names, or a map key.
enum Name {
    Defined(usize),
    Key(String),
}

fn field_name<'n>(source: &'n Source, name: &'n Name) -> &'n str {
    match (name, source) {
        (Name::Defined(index), Source::Struct { entry, .. }) => &entry.field_names[*index],
        (Name::Key(name), _) => name,
        (Name::Defined(_), Source::Map { .. }) => unreachable!("map fields are keys"),
    }
}

/// The value of the field whose name was just handed out.
enum Pending {
    Wire(TypeId, Name),
    Zero(&'static str),
}

/// A struct's fields: those on the wire, then the declared ones that were
/// left out, as zero values.
struct StructAccess<'s, 'd, 'a> {
    session: &'s mut Session<'d, 'a>,
    source: Source,
    /// The wire has no more fields.
    done: bool,
    fill: ZeroFill,
    pending: Option<Pending>,
    depth: usize,
}

impl StructAccess<'_, '_, '_> {
    /// The next field on the wire: its type and name.
    fn next_wire_field(&mut self) -> Result<Option<(TypeId, Name)>, Error> {
        match &mut self.source {
            Source::Struct { entry, last } => {
                let Type::Struct { fields, .. } = &entry.ty else { unreachable!("struct source holds a struct type") };
                let index = self.session.next_field(last, fields.len())?;
                Ok(index.map(|index| (fields[index].id, Name::Defined(index))))
            }
            Source::Map { elem, remaining } => {
                if *remaining == 0 {
                    return Ok(None);
                }
                *remaining -= 1;
                let name = self.session.input.string()?.into_owned();
                Ok(Some((*elem, Name::Key(name))))
            }
        }
    }

    /// Skips the fields the visitor did not read.
    fn finish(mut self) -> Result<(), Error> {
        if let Some(Pending::Wire(id, _)) = self.pending.take() {
            self.session.skip_value(id, self.depth)?;
        }
        while !self.done {
            match self.next_wire_field()? {
                Some((id, _)) => self.session.skip_value(id, self.depth)?,
                None => self.done = true,
            }
        }
        Ok(())
    }
}

impl<'de> de::MapAccess<'de> for StructAccess<'_, '_, 'de> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Error> {
        if let Some(Pending::Wire(id, _)) = self.pending.take() {
            self.session.skip_value(id, self.depth)?;
        }
        if !self.done {
            match self.next_wire_field()? {
                Some((id, name)) => {
                    let field = field_name(&self.source, &name);
                    self.fill.seen(field);
                    let key = seed.deserialize(StrDeserializer::<Error>::new(field));
                    self.pending = Some(Pending::Wire(id, name));
                    return key.map(Some);
                }
                None => self.done = true,
            }
        }
        let Some(field) = self.fill.next_missing() else {
            return Ok(None);
        };
        self.pending = Some(Pending::Zero(field));
        seed.deserialize(BorrowedStrDeserializer::<Error>::new(field)).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        match self.pending.take() {
            Some(Pending::Wire(id, name)) => {
                guarded(self.session, Pos::Start(id), self.depth, |de| seed.deserialize(de))
                    .map_err(|e| e.in_field(field_name(&self.source, &name)))
            }
            Some(Pending::Zero(field)) => seed.deserialize(Zero).map_err(|e| e.in_field(field)),
            None => Err(Error::data("struct value requested before its field")),
        }
    }
}

enum Variant {
    Name(String),
    Index(u64),
}

struct Enum<'s, 'd, 'a> {
    session: &'s mut Session<'d, 'a>,
    variant: Variant,
    /// The interface's concrete value: its type and nesting level.
    payload: Option<(TypeId, usize)>,
}

impl<'de, 's, 'd> de::EnumAccess<'de> for Enum<'s, 'd, 'de> {
    type Error = Error;
    type Variant = Payload<'s, 'd, 'de>;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant), Error> {
        let variant = match self.variant {
            Variant::Name(name) => seed.deserialize(StrDeserializer::<Error>::new(&name))?,
            Variant::Index(index) => seed.deserialize(U64Deserializer::<Error>::new(index))?,
        };
        Ok((variant, Payload { session: self.session, payload: self.payload }))
    }
}

struct Payload<'s, 'd, 'a> {
    session: &'s mut Session<'d, 'a>,
    payload: Option<(TypeId, usize)>,
}

impl<'d, 'a> Payload<'_, 'd, 'a> {
    fn read<T>(self, read: impl FnOnce(Deserializer<'_, 'd, 'a>) -> Result<T, Error>) -> Result<T, Error> {
        let (id, depth) =
            self.payload.ok_or_else(|| Error::data("expected an interface value, found a unit variant"))?;
        guarded(self.session, Pos::Start(id), depth, read)
    }
}

impl<'de> de::VariantAccess<'de> for Payload<'_, '_, 'de> {
    type Error = Error;

    fn unit_variant(self) -> Result<(), Error> {
        match self.payload {
            Some((id, depth)) => self.session.skip_value(id, depth),
            None => Ok(()),
        }
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Error> {
        self.read(|de| seed.deserialize(de))
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, Error> {
        self.read(|de| de::Deserializer::deserialize_tuple(de, len, visitor))
    }

    fn struct_variant<V: Visitor<'de>>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value, Error> {
        self.read(|de| de::Deserializer::deserialize_struct(de, "", fields, visitor))
    }
}
