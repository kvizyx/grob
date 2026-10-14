//! Deserializing from [`Value`]s.

use std::sync::Arc;

use serde::de::value::{SeqDeserializer, StrDeserializer, U64Deserializer};
use serde::de::{self, DeserializeSeed, IntoDeserializer, Unexpected, Visitor};

use super::zero::{Zero, ZeroFill};
use crate::{Error, Value};

impl IntoDeserializer<'_, Error> for Value {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self {
        self
    }
}

fn unexpected(value: &Value) -> Unexpected<'_> {
    match value {
        Value::Bool(b) => Unexpected::Bool(*b),
        Value::Int(i) => Unexpected::Signed(*i),
        Value::Uint(u) => Unexpected::Unsigned(*u),
        Value::Float(f) => Unexpected::Float(*f),
        Value::String(s) => Unexpected::Str(s),
        Value::Bytes(b) => Unexpected::Bytes(b),
        Value::Complex(..) | Value::List(_) => Unexpected::Seq,
        Value::Struct(_) | Value::Map(_) => Unexpected::Map,
        Value::Interface { .. } => Unexpected::Other("interface value"),
        Value::Nil => Unexpected::Option,
    }
}

fn byte_seq(bytes: &[u8]) -> SeqDeserializer<impl Iterator<Item = u8>, Error> {
    SeqDeserializer::new(bytes.iter().copied())
}

impl<'de> de::Deserializer<'de> for Value {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self {
            Value::Bool(b) => visitor.visit_bool(b),
            Value::Int(i) => visitor.visit_i64(i),
            Value::Uint(u) => visitor.visit_u64(u),
            Value::Float(f) => visitor.visit_f64(f),
            Value::Complex(re, im) => visitor.visit_seq(SeqDeserializer::new([re, im].into_iter())),
            Value::String(s) => visitor.visit_string(s),
            Value::Bytes(b) => visitor.visit_byte_buf(b),
            Value::List(items) => visitor.visit_seq(List { items: items.into_iter(), next: 0 }),
            Value::Map(entries) => visitor.visit_map(Map { entries: entries.into_iter(), next: 0, value: None }),
            Value::Struct(fields) => visitor.visit_map(Struct::new(fields, &[])),
            // Without a type to go by, an interface is its concrete value.
            Value::Interface { value, .. } => value.deserialize_any(visitor),
            Value::Nil => visitor.visit_none(),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_i16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_i128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_u16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_u128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self {
            Value::String(s) => visitor.visit_byte_buf(s.into_bytes()),
            value => value.deserialize_any(visitor),
        }
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self {
            Value::Nil => visitor.visit_none(),
            value => visitor.visit_some(value),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _: &'static str, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _: &'static str, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    /// Byte strings also deserialize as sequences of bytes.
    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self {
            Value::Bytes(bytes) => visitor.visit_seq(byte_seq(&bytes)),
            Value::String(s) => visitor.visit_seq(byte_seq(s.as_bytes())),
            value => value.deserialize_any(visitor),
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
        self,
        _: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self {
            Value::Struct(present) => visitor.visit_map(Struct::new(present, fields)),
            Value::Map(entries) => {
                let present = entries
                    .into_iter()
                    .map(|(key, value)| match key {
                        Value::String(key) => Ok((Arc::from(key), value)),
                        other => Err(de::Error::invalid_type(unexpected(&other), &"a string key")),
                    })
                    .collect::<Result<_, Error>>()?;
                visitor.visit_map(Struct::new(present, fields))
            }
            value => Err(de::Error::invalid_type(unexpected(&value), &visitor)),
        }
    }

    /// Interface values select the variant by their registered name, strings
    /// by variant name and integers by variant index.
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self {
            Value::Interface { name, value } => {
                visitor.visit_enum(Enum { variant: Variant::Name(name), value: Some(*value) })
            }
            Value::String(name) => visitor.visit_enum(Enum { variant: Variant::Name(name), value: None }),
            Value::Int(index) => {
                let index = u64::try_from(index).map_err(|_| Error::data(format!("invalid variant index {index}")))?;
                visitor.visit_enum(Enum { variant: Variant::Index(index), value: None })
            }
            Value::Uint(index) => visitor.visit_enum(Enum { variant: Variant::Index(index), value: None }),
            value => Err(de::Error::invalid_type(unexpected(&value), &visitor)),
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_any(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

/// The elements of a list.
struct List {
    items: std::vec::IntoIter<Value>,
    next: usize,
}

impl<'de> de::SeqAccess<'de> for List {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Error> {
        let Some(item) = self.items.next() else {
            return Ok(None);
        };
        self.next += 1;
        seed.deserialize(item).map(Some).map_err(|e| e.in_element(self.next - 1))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.items.len())
    }
}

/// The entries of a map.
struct Map {
    entries: std::vec::IntoIter<(Value, Value)>,
    next: usize,
    value: Option<Value>,
}

impl<'de> de::MapAccess<'de> for Map {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Error> {
        let Some((key, value)) = self.entries.next() else {
            return Ok(None);
        };
        self.next += 1;
        self.value = Some(value);
        seed.deserialize(key).map(Some).map_err(|e| e.in_entry(self.next - 1))
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        let value = self.value.take().ok_or_else(|| Error::data("map value requested before its key"))?;
        seed.deserialize(value).map_err(|e| e.in_entry(self.next - 1))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.entries.len())
    }
}

/// A struct's fields: those present, then the declared ones that were left
/// out, as zero values.
struct Struct {
    present: std::vec::IntoIter<(Arc<str>, Value)>,
    fill: ZeroFill,
    pending: Option<Pending>,
}

/// The value of the field whose name was just handed out.
enum Pending {
    Present(Arc<str>, Value),
    Zero(&'static str),
}

impl Struct {
    fn new(present: Vec<(Arc<str>, Value)>, fields: &'static [&'static str]) -> Self {
        Self { present: present.into_iter(), fill: ZeroFill::new(fields), pending: None }
    }
}

impl<'de> de::MapAccess<'de> for Struct {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Error> {
        if let Some((name, value)) = self.present.next() {
            self.fill.seen(&name);
            let key = seed.deserialize(StrDeserializer::<Error>::new(&name)).map(Some);
            self.pending = Some(Pending::Present(name, value));
            return key;
        }
        let Some(field) = self.fill.next_missing() else {
            return Ok(None);
        };
        self.pending = Some(Pending::Zero(field));
        seed.deserialize(de::value::BorrowedStrDeserializer::<Error>::new(field)).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        match self.pending.take() {
            Some(Pending::Present(name, value)) => seed.deserialize(value).map_err(|e| e.in_field(&name)),
            Some(Pending::Zero(field)) => seed.deserialize(Zero).map_err(|e| e.in_field(field)),
            None => Err(Error::data("struct value requested before its field")),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.present.len())
    }
}

enum Variant {
    Name(String),
    Index(u64),
}

struct Enum {
    variant: Variant,
    value: Option<Value>,
}

impl<'de> de::EnumAccess<'de> for Enum {
    type Error = Error;
    type Variant = Payload;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Payload), Error> {
        let variant = match self.variant {
            Variant::Name(name) => seed.deserialize(StrDeserializer::<Error>::new(&name))?,
            Variant::Index(index) => seed.deserialize(U64Deserializer::<Error>::new(index))?,
        };
        Ok((variant, Payload(self.value)))
    }
}

/// The concrete value of an interface; `None` for unit variants.
struct Payload(Option<Value>);

impl Payload {
    fn value(self) -> Result<Value, Error> {
        self.0.ok_or_else(|| Error::data("expected an interface value, found a unit variant"))
    }
}

impl<'de> de::VariantAccess<'de> for Payload {
    type Error = Error;

    fn unit_variant(self) -> Result<(), Error> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Error> {
        seed.deserialize(self.value()?)
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, Error> {
        de::Deserializer::deserialize_tuple(self.value()?, len, visitor)
    }

    fn struct_variant<V: Visitor<'de>>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value, Error> {
        de::Deserializer::deserialize_struct(self.value()?, "", fields, visitor)
    }
}
