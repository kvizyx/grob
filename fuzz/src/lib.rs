//! Shared pieces of the fuzz targets.

use std::fmt;
use std::io::{self, Read};

use serde::Deserialize;
use serde::de::{self, MapAccess, SeqAccess};

/// Whatever `deserialize_any` reports, to compare the two deserializers.
/// Floats are kept as bits, so NaN compares equal to itself.
#[derive(Debug, PartialEq)]
pub enum Any {
    None,
    Unit,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(u64),
    Str(String),
    Bytes(Vec<u8>),
    Some(Box<Any>),
    Seq(Vec<Any>),
    Map(Vec<(Any, Any)>),
}

struct AnyVisitor;

impl<'de> de::Visitor<'de> for AnyVisitor {
    type Value = Any;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("anything")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Any, E> {
        Ok(Any::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Any, E> {
        Ok(Any::Int(v))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Any, E> {
        Ok(Any::Uint(v))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Any, E> {
        Ok(Any::Float(v.to_bits()))
    }

    fn visit_str<E>(self, v: &str) -> Result<Any, E> {
        Ok(Any::Str(v.to_owned()))
    }

    fn visit_bytes<E>(self, v: &[u8]) -> Result<Any, E> {
        Ok(Any::Bytes(v.to_vec()))
    }

    fn visit_none<E>(self) -> Result<Any, E> {
        Ok(Any::None)
    }

    fn visit_some<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Any, D::Error> {
        Ok(Any::Some(Box::new(Any::deserialize(deserializer)?)))
    }

    fn visit_unit<E>(self) -> Result<Any, E> {
        Ok(Any::Unit)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Any, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Any::Seq(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Any, A::Error> {
        let mut entries = Vec::new();
        while let Some(entry) = map.next_entry()? {
            entries.push(entry);
        }
        Ok(Any::Map(entries))
    }
}

impl<'de> Deserialize<'de> for Any {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(AnyVisitor)
    }
}

/// A reader handing out its data in the given chunk sizes, cycled, like a
/// network connection would.
pub struct Chunked<'a> {
    data: &'a [u8],
    sizes: &'a [u8],
    next: usize,
}

impl<'a> Chunked<'a> {
    pub fn new(data: &'a [u8], sizes: &'a [u8]) -> Self {
        Self { data, sizes, next: 0 }
    }
}

impl Read for Chunked<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let size = match self.sizes.get(self.next % self.sizes.len().max(1)) {
            Some(size) => usize::from(*size).max(1),
            None => buf.len(),
        };
        self.next += 1;
        let size = size.min(buf.len()).min(self.data.len());
        buf[..size].copy_from_slice(&self.data[..size]);
        self.data = &self.data[size..];
        Ok(size)
    }
}
