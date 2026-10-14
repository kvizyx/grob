//! Inferring the gob types of a value.

use serde::ser::{self, Impossible, Serialize};

use super::schema::Schema;
use super::shape::Shape;
use crate::Error;
use crate::external::{Marshaler, marshaler};

/// What inference learned about one value.
pub(super) struct Inferred {
    pub shape: Shape,
    /// Zero values are left out of structs, and so out of their definitions.
    pub zero: bool,
    /// `None`, which gob can only send as an omitted struct field.
    pub nil: bool,
}

impl Inferred {
    fn new(shape: Shape, zero: bool) -> Self {
        Self { shape, zero, nil: false }
    }
}

/// Serializes a value into its [`Inferred`] type, collecting struct fields
/// and interface types into a [`Schema`].
pub(super) struct Infer<'s>(pub &'s mut Schema);

impl<'s> ser::Serializer for Infer<'s> {
    type Ok = Inferred;
    type Error = Error;
    type SerializeSeq = InferSeq<'s>;
    type SerializeTuple = InferSeq<'s>;
    type SerializeTupleStruct = InferSeq<'s>;
    type SerializeTupleVariant = Impossible<Inferred, Error>;
    type SerializeMap = InferMap<'s>;
    type SerializeStruct = InferStruct<'s>;
    type SerializeStructVariant = InferStruct<'s>;

    fn serialize_bool(self, v: bool) -> Result<Inferred, Error> {
        Ok(Inferred::new(Shape::Bool, !v))
    }

    fn serialize_i8(self, v: i8) -> Result<Inferred, Error> {
        self.serialize_i64(v.into())
    }

    fn serialize_i16(self, v: i16) -> Result<Inferred, Error> {
        self.serialize_i64(v.into())
    }

    fn serialize_i32(self, v: i32) -> Result<Inferred, Error> {
        self.serialize_i64(v.into())
    }

    fn serialize_i64(self, v: i64) -> Result<Inferred, Error> {
        Ok(Inferred::new(Shape::Int, v == 0))
    }

    fn serialize_i128(self, v: i128) -> Result<Inferred, Error> {
        self.serialize_i64(i64::try_from(v).map_err(|_| Error::data(format!("{v} overflows int64")))?)
    }

    fn serialize_u8(self, v: u8) -> Result<Inferred, Error> {
        Ok(Inferred::new(Shape::Byte, v == 0))
    }

    fn serialize_u16(self, v: u16) -> Result<Inferred, Error> {
        self.serialize_u64(v.into())
    }

    fn serialize_u32(self, v: u32) -> Result<Inferred, Error> {
        self.serialize_u64(v.into())
    }

    fn serialize_u64(self, v: u64) -> Result<Inferred, Error> {
        Ok(Inferred::new(Shape::Uint, v == 0))
    }

    fn serialize_u128(self, v: u128) -> Result<Inferred, Error> {
        self.serialize_u64(u64::try_from(v).map_err(|_| Error::data(format!("{v} overflows uint64")))?)
    }

    fn serialize_f32(self, v: f32) -> Result<Inferred, Error> {
        self.serialize_f64(v.into())
    }

    fn serialize_f64(self, v: f64) -> Result<Inferred, Error> {
        Ok(Inferred::new(Shape::Float, v == 0.0))
    }

    fn serialize_char(self, _: char) -> Result<Inferred, Error> {
        Ok(Inferred::new(Shape::String, false))
    }

    fn serialize_str(self, v: &str) -> Result<Inferred, Error> {
        Ok(Inferred::new(Shape::String, v.is_empty()))
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<Inferred, Error> {
        Ok(Inferred::new(Shape::Bytes, v.is_empty()))
    }

    fn serialize_none(self) -> Result<Inferred, Error> {
        Ok(Inferred { shape: Shape::Unknown, zero: true, nil: true })
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Inferred, Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Inferred, Error> {
        self.serialize_unit_struct("")
    }

    fn serialize_unit_struct(self, name: &'static str) -> Result<Inferred, Error> {
        self.0.struct_index(name);
        Ok(Inferred::new(Shape::Struct(name), false))
    }

    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> Result<Inferred, Error> {
        self.serialize_str(variant)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, name: &'static str, value: &T) -> Result<Inferred, Error> {
        let (shape, zero) = match marshaler(name) {
            Some((Marshaler::Binary, zero)) => (Shape::BinaryMarshaler, zero),
            Some((Marshaler::Gob, zero)) => (Shape::GobEncoder, zero),
            None => return value.serialize(self),
        };
        // Whether the bytes are valid shows when they are written.
        let inner = value.serialize(Infer(&mut *self.0))?;
        Ok(Inferred::new(shape, zero || inner.zero))
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Inferred, Error> {
        let inner = value.serialize(Infer(&mut *self.0))?;
        if inner.nil {
            return Err(Error::data(format!("cannot encode a nil pointer inside interface value {variant}")));
        }
        self.0.add_interface(variant, inner.shape)?;
        Ok(Inferred::new(Shape::Interface, false))
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<InferSeq<'s>, Error> {
        Ok(InferSeq { schema: self.0, elem: Shape::Unknown, count: 0, array: None })
    }

    fn serialize_tuple(self, len: usize) -> Result<InferSeq<'s>, Error> {
        Ok(InferSeq { schema: self.0, elem: Shape::Unknown, count: 0, array: Some(len) })
    }

    fn serialize_tuple_struct(self, _: &'static str, len: usize) -> Result<InferSeq<'s>, Error> {
        self.serialize_tuple(len)
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Impossible<Inferred, Error>, Error> {
        Err(tuple_variant(variant))
    }

    fn serialize_map(self, _: Option<usize>) -> Result<InferMap<'s>, Error> {
        Ok(InferMap { schema: self.0, key: Shape::Unknown, elem: Shape::Unknown, count: 0 })
    }

    fn serialize_struct(self, name: &'static str, _: usize) -> Result<InferStruct<'s>, Error> {
        let index = self.0.struct_index(name);
        Ok(InferStruct { schema: self.0, name, index, variant: false })
    }

    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<InferStruct<'s>, Error> {
        let index = self.0.struct_index(variant);
        Ok(InferStruct { schema: self.0, name: variant, index, variant: true })
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

pub(super) fn tuple_variant(variant: &str) -> Error {
    Error::data(format!("tuple variant {variant} has no gob equivalent"))
}

/// Infers the type of an element, which gob always sends, so it may not be
/// nil.
fn element<T: Serialize + ?Sized>(schema: &mut Schema, value: &T, what: &str) -> Result<Shape, Error> {
    let inner = value.serialize(Infer(schema))?;
    if inner.nil {
        return Err(Error::data(format!("cannot encode a nil {what}")));
    }
    Ok(inner.shape)
}

/// A sequence, tuple or array: its elements share one type.
pub(super) struct InferSeq<'s> {
    schema: &'s mut Schema,
    elem: Shape,
    count: usize,
    /// Tuples and fixed-size arrays are Go arrays.
    array: Option<usize>,
}

impl InferSeq<'_> {
    fn push<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        let index = self.count;
        self.count += 1;
        element(self.schema, value, "element").and_then(|shape| self.elem.merge(shape)).map_err(|e| e.in_element(index))
    }

    fn finish(self) -> Inferred {
        let elem = Box::new(self.elem);
        let shape = match self.array {
            Some(len) => Shape::Array(elem, len),
            None => Shape::Seq(elem),
        };
        Inferred::new(shape, self.count == 0)
    }
}

impl ser::SerializeSeq for InferSeq<'_> {
    type Ok = Inferred;
    type Error = Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }

    fn end(self) -> Result<Inferred, Error> {
        Ok(self.finish())
    }
}

impl ser::SerializeTuple for InferSeq<'_> {
    type Ok = Inferred;
    type Error = Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }

    fn end(self) -> Result<Inferred, Error> {
        Ok(self.finish())
    }
}

impl ser::SerializeTupleStruct for InferSeq<'_> {
    type Ok = Inferred;
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.push(value)
    }

    fn end(self) -> Result<Inferred, Error> {
        Ok(self.finish())
    }
}

/// A map: its keys share one type, and its values another.
pub(super) struct InferMap<'s> {
    schema: &'s mut Schema,
    key: Shape,
    elem: Shape,
    count: usize,
}

impl ser::SerializeMap for InferMap<'_> {
    type Ok = Inferred;
    type Error = Error;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Error> {
        self.count += 1;
        element(self.schema, key, "map key")
            .and_then(|shape| self.key.merge(shape))
            .map_err(|e| e.in_entry(self.count - 1))
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        element(self.schema, value, "map value")
            .and_then(|shape| self.elem.merge(shape))
            .map_err(|e| e.in_entry(self.count - 1))
    }

    fn end(self) -> Result<Inferred, Error> {
        Ok(Inferred::new(Shape::Map(Box::new(self.key), Box::new(self.elem)), self.count == 0))
    }
}

/// A struct: its non-zero fields go into the schema.
pub(super) struct InferStruct<'s> {
    schema: &'s mut Schema,
    name: &'static str,
    index: usize,
    /// An enum struct variant, which is an interface value.
    variant: bool,
}

impl InferStruct<'_> {
    fn field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), Error> {
        let inner = value.serialize(Infer(&mut *self.schema)).map_err(|e| e.in_field(key))?;
        if inner.zero {
            return Ok(());
        }
        self.schema.add_field(self.index, key, inner.shape).map_err(|e| e.in_field(key))
    }

    fn finish(self) -> Result<Inferred, Error> {
        let shape = Shape::Struct(self.name);
        if !self.variant {
            return Ok(Inferred::new(shape, false));
        }
        self.schema.add_interface(self.name, shape)?;
        Ok(Inferred::new(Shape::Interface, false))
    }
}

impl ser::SerializeStruct for InferStruct<'_> {
    type Ok = Inferred;
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), Error> {
        self.field(key, value)
    }

    fn end(self) -> Result<Inferred, Error> {
        self.finish()
    }
}

impl ser::SerializeStructVariant for InferStruct<'_> {
    type Ok = Inferred;
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), Error> {
        self.field(key, value)
    }

    fn end(self) -> Result<Inferred, Error> {
        self.finish()
    }
}
