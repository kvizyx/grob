//! Zero values for struct fields gob left out.

use serde::de::value::SeqDeserializer;
use serde::de::{self, Visitor};

use crate::Error;
use crate::external::{BINARY_MARSHALER, GOB_ENCODER};

/// Tracks which declared fields of a Rust struct have been seen, so the
/// others can be given their zero values afterwards.
pub(super) struct ZeroFill {
    fields: &'static [&'static str],
    missing: Missing,
    /// Where to start looking for the next field: fields usually arrive in
    /// declaration order.
    guess: usize,
    /// The next field to consider giving a zero value.
    next: usize,
}

/// One bit per field, without allocating for the usual struct sizes.
enum Missing {
    Mask(u128),
    List(Vec<bool>),
}

impl ZeroFill {
    pub fn new(fields: &'static [&'static str]) -> Self {
        let missing = match fields.len() {
            128 => Missing::Mask(u128::MAX),
            count @ 0..128 => Missing::Mask((1 << count) - 1),
            count => Missing::List(vec![true; count]),
        };
        Self { fields, missing, guess: 0, next: 0 }
    }

    /// Marks the field named `name` as present.
    pub fn seen(&mut self, name: &str) {
        let (earlier, later) = self.fields.split_at(self.guess);
        let Some(index) = later
            .iter()
            .position(|field| *field == name)
            .map(|i| i + earlier.len())
            .or_else(|| earlier.iter().position(|field| *field == name))
        else {
            return;
        };
        self.guess = index + 1;
        match &mut self.missing {
            Missing::Mask(mask) => *mask &= !(1 << index),
            Missing::List(list) => list[index] = false,
        }
    }

    /// The next field that was not seen.
    pub fn next_missing(&mut self) -> Option<&'static str> {
        while self.next < self.fields.len() {
            let index = self.next;
            self.next += 1;
            let missing = match &self.missing {
                Missing::Mask(mask) => mask & (1 << index) != 0,
                Missing::List(list) => list[index],
            };
            if missing {
                return Some(self.fields[index]);
            }
        }
        None
    }
}

/// The zero value of whatever type asks for it.
#[derive(Clone, Copy)]
pub(super) struct Zero;

impl<'de> de::Deserializer<'de> for Zero {
    type Error = Error;

    /// Without a type to go by, nil.
    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_none()
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_bool(false)
    }

    fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_i64(0)
    }

    fn deserialize_i16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_i64(0)
    }

    fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_i64(0)
    }

    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_i64(0)
    }

    fn deserialize_i128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_i64(0)
    }

    fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_u64(visitor)
    }

    fn deserialize_u16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_u64(visitor)
    }

    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_u64(visitor)
    }

    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_u64(0)
    }

    fn deserialize_u128<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_u64(visitor)
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_f64(0.0)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_f64(0.0)
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_char('\0')
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_str("")
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_str("")
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_bytes(&[])
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_bytes(&[])
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_none()
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _: &'static str, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, visitor: V) -> Result<V::Value, Error> {
        match name {
            // Externally encoded values default to `T::default()`.
            BINARY_MARSHALER | GOB_ENCODER => visitor.visit_none(),
            _ => visitor.visit_newtype_struct(self),
        }
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_seq(SeqDeserializer::<_, Error>::new(std::iter::empty::<Zero>()))
    }

    /// Fixed-size arrays are full of zeros.
    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_seq(SeqDeserializer::<_, Error>::new(std::iter::repeat_n(Zero, len)))
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_map(de::value::MapDeserializer::<_, Error>::new(std::iter::empty::<(Zero, Zero)>()))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_map(ZeroStruct(ZeroFill::new(fields)))
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        _: &'static [&'static str],
        _: V,
    ) -> Result<V::Value, Error> {
        Err(Error::data(format!("missing value for enum {name}; use Option for a nil interface")))
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

impl de::IntoDeserializer<'_, Error> for Zero {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self {
        self
    }
}

/// A struct whose fields are all zero.
struct ZeroStruct(ZeroFill);

impl<'de> de::MapAccess<'de> for ZeroStruct {
    type Error = Error;

    fn next_key_seed<K: de::DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Error> {
        self.0
            .next_missing()
            .map(|field| seed.deserialize(de::value::BorrowedStrDeserializer::<Error>::new(field)))
            .transpose()
    }

    fn next_value_seed<V: de::DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        seed.deserialize(Zero)
    }
}
