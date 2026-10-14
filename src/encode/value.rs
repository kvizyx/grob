//! Encoding [`Value`]s against type definitions.

use std::sync::Arc;

use super::Encoder;
use crate::types::Type;
use crate::wire::Writer;
use crate::{Error, Field, TypeId, Value};

fn mismatch(value: &Value, ty: impl std::fmt::Display) -> Error {
    Error::data(format!("cannot encode {} as {ty}", value.kind()))
}

impl Encoder {
    /// Writes a top-level or interface value. Values that are not structs
    /// are preceded by a zero field delta, and zero values are sent.
    pub(super) fn write_top(&mut self, w: &mut Writer, id: TypeId, value: &Value) -> Result<(), Error> {
        if *value == Value::Nil {
            return Err(Error::data("cannot encode a nil value"));
        }
        if !self.state.types.is_struct(id) {
            w.uint(0);
        }
        self.write_value(w, id, value)
    }

    pub(super) fn write_value(&mut self, w: &mut Writer, id: TypeId, value: &Value) -> Result<(), Error> {
        if id.is_basic() {
            return self.write_basic(w, id, value);
        }
        let entry =
            Arc::clone(self.state.types.get(id).ok_or_else(|| Error::data(format!("type {id} is not defined")))?);
        match (&entry.ty, value) {
            (Type::Struct { name, fields }, Value::Struct(values)) => self.write_struct(w, name, fields, values),
            (Type::Array { elem, len, .. }, Value::List(items)) => {
                if items.len() != *len {
                    let name = entry.ty.name();
                    return Err(Error::data(format!("array {name} holds {len} elements, not {}", items.len())));
                }
                self.write_list(w, *elem, items)
            }
            (Type::Slice { elem, .. }, Value::List(items)) => self.write_list(w, *elem, items),
            (Type::Map { key, elem, .. }, Value::Map(entries)) => {
                w.uint(entries.len() as u64);
                for (index, (k, v)) in entries.iter().enumerate() {
                    self.write_element(w, *key, k)
                        .and_then(|()| self.write_element(w, *elem, v))
                        .map_err(|e| e.in_entry(index))?;
                }
                Ok(())
            }
            (ty, Value::Bytes(bytes)) if ty.is_external() => {
                w.bytes(bytes);
                Ok(())
            }
            (ty, value) => Err(mismatch(value, ty.name())),
        }
    }

    fn write_basic(&mut self, w: &mut Writer, id: TypeId, value: &Value) -> Result<(), Error> {
        match (id, value) {
            (TypeId::BOOL, Value::Bool(b)) => w.bool(*b),
            (TypeId::INT, Value::Int(i)) => w.int(*i),
            (TypeId::INT, Value::Uint(u)) => {
                w.int(i64::try_from(*u).map_err(|_| Error::data(format!("{u} overflows int64")))?);
            }
            (TypeId::UINT, Value::Uint(u)) => w.uint(*u),
            (TypeId::UINT, Value::Int(i)) => {
                w.uint(u64::try_from(*i).map_err(|_| Error::data(format!("{i} does not fit a uint")))?);
            }
            (TypeId::FLOAT, Value::Float(f)) => w.float(*f),
            (TypeId::COMPLEX, Value::Complex(re, im)) => {
                w.float(*re);
                w.float(*im);
            }
            (TypeId::BYTES | TypeId::STRING, Value::Bytes(b)) => w.bytes(b),
            (TypeId::BYTES | TypeId::STRING, Value::String(s)) => w.bytes(s.as_bytes()),
            (TypeId::INTERFACE, Value::Nil) => w.uint(0),
            (TypeId::INTERFACE, Value::Interface { name, value }) => self.write_interface(w, name, value)?,
            _ => return Err(mismatch(value, basic_name(id))),
        }
        Ok(())
    }

    fn write_list(&mut self, w: &mut Writer, elem: TypeId, items: &[Value]) -> Result<(), Error> {
        w.uint(items.len() as u64);
        for (index, item) in items.iter().enumerate() {
            self.write_element(w, elem, item).map_err(|e| e.in_element(index))?;
        }
        Ok(())
    }

    /// Writes an array, slice or map element. Elements are always sent, zero
    /// or not; only interfaces may be nil.
    fn write_element(&mut self, w: &mut Writer, id: TypeId, value: &Value) -> Result<(), Error> {
        if *value == Value::Nil && id != TypeId::INTERFACE {
            return Err(Error::data("cannot encode a nil element"));
        }
        self.write_value(w, id, value)
    }

    fn write_struct(
        &mut self,
        w: &mut Writer,
        name: &str,
        fields: &[Field],
        values: &[(Arc<str>, Value)],
    ) -> Result<(), Error> {
        let mut indexed = Vec::with_capacity(values.len());
        for (field, value) in values {
            let index = fields
                .iter()
                .position(|f| *f.name == **field)
                .ok_or_else(|| Error::data(format!("struct {name} has no field {field}")))?;
            indexed.push((index, value));
        }
        // Values may list fields in any order; the wire wants definition order.
        indexed.sort_by_key(|&(index, _)| index);
        if let Some(pair) = indexed.windows(2).find(|pair| pair[0].0 == pair[1].0) {
            return Err(Error::data(format!("field {} given twice for struct {name}", fields[pair[0].0].name)));
        }
        let mut last = None;
        for (index, value) in indexed {
            let id = fields[index].id;
            if self.is_omitted(id, value) {
                continue;
            }
            w.uint(field_delta(last, index));
            last = Some(index);
            self.write_value(w, id, value).map_err(|e| e.in_field(&fields[index].name))?;
        }
        w.uint(0);
        Ok(())
    }

    /// Whether a struct field is left out, like Go leaves out zero basic
    /// values, empty slices and nil pointers, maps and interfaces. Structs
    /// and arrays are always sent.
    /// Values of the wrong kind are never omitted, so they get reported.
    fn is_omitted(&self, id: TypeId, value: &Value) -> bool {
        match (id, value) {
            (_, Value::Nil)
            | (TypeId::BOOL, Value::Bool(_))
            | (TypeId::INT | TypeId::UINT, Value::Int(_) | Value::Uint(_))
            | (TypeId::FLOAT, Value::Float(_))
            | (TypeId::COMPLEX, Value::Complex(..))
            | (TypeId::BYTES | TypeId::STRING, Value::String(_) | Value::Bytes(_)) => value.is_zero(),
            (_, Value::List(items)) => {
                items.is_empty() && self.state.types.get(id).is_some_and(|entry| matches!(entry.ty, Type::Slice { .. }))
            }
            _ => false,
        }
    }

    /// Writes an interface value: the concrete type's name and id, then the
    /// value prefixed by its length.
    fn write_interface(&mut self, w: &mut Writer, name: &str, value: &Value) -> Result<(), Error> {
        if name.is_empty() {
            return Err(Error::data("interface type name must not be empty"));
        }
        let id = self.interface_type(name)?;
        w.bytes(name.as_bytes());
        w.int(id.get().into());
        let start = w.len();
        self.write_top(w, id, value)?;
        w.prefix_length(start);
        Ok(())
    }
}

/// The delta from the field written last to field `index`; fields are
/// numbered from 0 and the first delta counts from -1.
#[inline]
pub(crate) fn field_delta(last: Option<usize>, index: usize) -> u64 {
    (index - last.map_or(0, |last| last + 1) + 1) as u64
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
