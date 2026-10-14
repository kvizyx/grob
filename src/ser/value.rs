//! Serializing into [`Value`]s.

use std::sync::Arc;

use serde::ser::{self, Impossible, Serialize};

use super::infer::tuple_variant;
use crate::external::marshaler;
use crate::{Error, Value};

/// The bytes of an externally encoded value, which must serialize as bytes,
/// a string, or a sequence of integers up to 255 (like `uuid::Uuid` or
/// `[u8; 16]` do).
pub(super) fn external_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, Error> {
    let not_bytes = || Error::data("externally encoded values must serialize as bytes or a string");
    match value.serialize(ValueSerializer)?.value {
        Value::Bytes(bytes) => Ok(bytes),
        Value::String(string) => Ok(string.into_bytes()),
        Value::List(items) => items
            .iter()
            .map(|item| item.as_u64().and_then(|byte| u8::try_from(byte).ok()).ok_or_else(not_bytes))
            .collect(),
        _ => Err(not_bytes()),
    }
}

/// A serialized value, and whether it came from a `u8`: a sequence of them
/// is a `[]byte`.
pub(super) struct Built {
    pub value: Value,
    byte: bool,
    /// An externally encoded zero value, which has bytes all the same.
    zero: bool,
}

impl Built {
    /// Whether structs leave the value out.
    pub fn is_zero(&self) -> bool {
        self.zero || self.value.is_zero()
    }
}

impl From<Value> for Built {
    fn from(value: Value) -> Self {
        Self { value, byte: false, zero: false }
    }
}

/// Serializes a value into a [`Value`], leaving zero fields out of structs.
pub(super) struct ValueSerializer;

impl ser::Serializer for ValueSerializer {
    type Ok = Built;
    type Error = Error;
    type SerializeSeq = ValueSeq;
    type SerializeTuple = ValueSeq;
    type SerializeTupleStruct = ValueSeq;
    type SerializeTupleVariant = Impossible<Built, Error>;
    type SerializeMap = ValueMap;
    type SerializeStruct = ValueStruct;
    type SerializeStructVariant = ValueStruct;

    fn serialize_bool(self, v: bool) -> Result<Built, Error> {
        Ok(Value::Bool(v).into())
    }

    fn serialize_i8(self, v: i8) -> Result<Built, Error> {
        self.serialize_i64(v.into())
    }

    fn serialize_i16(self, v: i16) -> Result<Built, Error> {
        self.serialize_i64(v.into())
    }

    fn serialize_i32(self, v: i32) -> Result<Built, Error> {
        self.serialize_i64(v.into())
    }

    fn serialize_i64(self, v: i64) -> Result<Built, Error> {
        Ok(Value::Int(v).into())
    }

    fn serialize_i128(self, v: i128) -> Result<Built, Error> {
        self.serialize_i64(i64::try_from(v).map_err(|_| Error::data(format!("{v} overflows int64")))?)
    }

    fn serialize_u8(self, v: u8) -> Result<Built, Error> {
        Ok(Built { value: Value::Uint(v.into()), byte: true, zero: false })
    }

    fn serialize_u16(self, v: u16) -> Result<Built, Error> {
        self.serialize_u64(v.into())
    }

    fn serialize_u32(self, v: u32) -> Result<Built, Error> {
        self.serialize_u64(v.into())
    }

    fn serialize_u64(self, v: u64) -> Result<Built, Error> {
        Ok(Value::Uint(v).into())
    }

    fn serialize_u128(self, v: u128) -> Result<Built, Error> {
        self.serialize_u64(u64::try_from(v).map_err(|_| Error::data(format!("{v} overflows uint64")))?)
    }

    fn serialize_f32(self, v: f32) -> Result<Built, Error> {
        self.serialize_f64(v.into())
    }

    fn serialize_f64(self, v: f64) -> Result<Built, Error> {
        Ok(Value::Float(v).into())
    }

    fn serialize_char(self, v: char) -> Result<Built, Error> {
        Ok(Value::String(v.to_string()).into())
    }

    fn serialize_str(self, v: &str) -> Result<Built, Error> {
        Ok(Value::String(v.to_owned()).into())
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<Built, Error> {
        Ok(Value::Bytes(v.to_vec()).into())
    }

    fn serialize_none(self) -> Result<Built, Error> {
        Ok(Value::Nil.into())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Built, Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Built, Error> {
        Ok(Value::Struct(Vec::new()).into())
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<Built, Error> {
        self.serialize_unit()
    }

    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> Result<Built, Error> {
        self.serialize_str(variant)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, name: &'static str, value: &T) -> Result<Built, Error> {
        match marshaler(name) {
            Some((_, zero)) => Ok(Built { value: Value::Bytes(external_bytes(value)?), byte: false, zero }),
            None => value.serialize(self),
        }
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Built, Error> {
        let value = value.serialize(self)?.value;
        if value == Value::Nil {
            return Err(Error::data(format!("cannot encode a nil pointer inside interface value {variant}")));
        }
        Ok(Value::Interface { name: variant.to_owned(), value: Box::new(value) }.into())
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<ValueSeq, Error> {
        Ok(ValueSeq { items: Vec::with_capacity(len.unwrap_or(0)), bytes: true, array: false })
    }

    fn serialize_tuple(self, len: usize) -> Result<ValueSeq, Error> {
        Ok(ValueSeq { items: Vec::with_capacity(len), bytes: false, array: true })
    }

    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<ValueSeq, Error> {
        self.serialize_tuple(len)
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Impossible<Built, Error>, Error> {
        Err(tuple_variant(variant))
    }

    fn serialize_map(self, len: Option<usize>) -> Result<ValueMap, Error> {
        Ok(ValueMap { entries: Vec::with_capacity(len.unwrap_or(0)), key: None })
    }

    fn serialize_struct(self, _: &'static str, len: usize) -> Result<ValueStruct, Error> {
        Ok(ValueStruct { fields: Vec::with_capacity(len), variant: None })
    }

    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<ValueStruct, Error> {
        Ok(ValueStruct { fields: Vec::with_capacity(len), variant: Some(variant) })
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

/// Serializes an element, which gob always sends, so it may not be nil.
fn element<T: Serialize + ?Sized>(value: &T, what: &str) -> Result<Built, Error> {
    let built = value.serialize(ValueSerializer)?;
    if built.value == Value::Nil {
        return Err(Error::data(format!("cannot encode a nil {what}")));
    }
    Ok(built)
}

pub(super) struct ValueSeq {
    items: Vec<Value>,
    /// Whether every element so far was a `u8`.
    bytes: bool,
    array: bool,
}

impl ValueSeq {
    fn push<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        let built = element(value, "element").map_err(|e| e.in_element(self.items.len()))?;
        self.bytes &= built.byte;
        self.items.push(built.value);
        Ok(())
    }

    fn finish(self) -> Built {
        if self.array || !self.bytes || self.items.is_empty() {
            return Value::List(self.items).into();
        }
        let bytes = self.items.iter().filter_map(Value::as_u64).filter_map(|byte| u8::try_from(byte).ok()).collect();
        Value::Bytes(bytes).into()
    }
}

impl ser::SerializeSeq for ValueSeq {
    type Ok = Built;
    type Error = Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }

    fn end(self) -> Result<Built, Error> {
        Ok(self.finish())
    }
}

impl ser::SerializeTuple for ValueSeq {
    type Ok = Built;
    type Error = Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }

    fn end(self) -> Result<Built, Error> {
        Ok(self.finish())
    }
}

impl ser::SerializeTupleStruct for ValueSeq {
    type Ok = Built;
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }

    fn end(self) -> Result<Built, Error> {
        Ok(self.finish())
    }
}

pub(super) struct ValueMap {
    entries: Vec<(Value, Value)>,
    key: Option<Value>,
}

impl ser::SerializeMap for ValueMap {
    type Ok = Built;
    type Error = Error;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Error> {
        self.key = Some(element(key, "map key").map_err(|e| e.in_entry(self.entries.len()))?.value);
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        let key = self.key.take().ok_or_else(|| Error::data("map value without a key"))?;
        let value = element(value, "map value").map_err(|e| e.in_entry(self.entries.len()))?;
        self.entries.push((key, value.value));
        Ok(())
    }

    fn end(self) -> Result<Built, Error> {
        Ok(Value::Map(self.entries).into())
    }
}

pub(super) struct ValueStruct {
    fields: Vec<(Arc<str>, Value)>,
    /// For enum struct variants, which are interface values.
    variant: Option<&'static str>,
}

impl ValueStruct {
    fn field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), Error> {
        let built = value.serialize(ValueSerializer).map_err(|e| e.in_field(key))?;
        if !built.is_zero() {
            self.fields.push((key.into(), built.value));
        }
        Ok(())
    }

    fn finish(self) -> Built {
        let value = Value::Struct(self.fields);
        match self.variant {
            Some(name) => Value::Interface { name: name.to_owned(), value: Box::new(value) }.into(),
            None => value.into(),
        }
    }
}

impl ser::SerializeStruct for ValueStruct {
    type Ok = Built;
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), Error> {
        self.field(key, value)
    }

    fn end(self) -> Result<Built, Error> {
        Ok(self.finish())
    }
}

impl ser::SerializeStructVariant for ValueStruct {
    type Ok = Built;
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), Error> {
        self.field(key, value)
    }

    fn end(self) -> Result<Built, Error> {
        Ok(self.finish())
    }
}
