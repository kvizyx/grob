//! Decoding into [`Value`]s.

use std::sync::Arc;

use super::Session;
use crate::types::Type;
use crate::{Error, Field, TypeId, Value};

/// Decodes the top-level value of type `id` the session is at.
pub(super) fn decode_top(session: &mut Session<'_, '_>, id: TypeId) -> Result<Value, Error> {
    session.singleton_delta(id)?;
    session.decode_value(id, 0)
}

impl Session<'_, '_> {
    /// Decodes a value of type `id` at nesting level `depth`.
    ///
    /// Recursion passes through here for every level, so the work for each
    /// kind of type lives in separate functions to keep this frame small.
    pub fn decode_value(&mut self, id: TypeId, depth: usize) -> Result<Value, Error> {
        self.check_depth(depth)?;
        if id.is_basic() {
            return if id == TypeId::INTERFACE { self.decode_interface(depth) } else { self.decode_basic(id) };
        }
        let entry = self.entry(id)?;
        match &entry.ty {
            Type::Struct { fields, .. } => self.decode_struct(fields, &entry.field_names, depth),
            Type::Array { elem, len, .. } => {
                let count = self.array_count(*len)?;
                self.decode_list(*elem, count, depth)
            }
            Type::Slice { elem, .. } => {
                let count = self.input.count()?;
                self.decode_list(*elem, count, depth)
            }
            Type::Map { key, elem, .. } => self.decode_map(*key, *elem, depth),
            Type::GobEncoder { .. } | Type::BinaryMarshaler { .. } | Type::TextMarshaler { .. } => {
                self.decode_basic(TypeId::BYTES)
            }
        }
    }

    #[inline(never)]
    fn decode_basic(&mut self, id: TypeId) -> Result<Value, Error> {
        let input = &mut self.input;
        Ok(match id {
            TypeId::BOOL => Value::Bool(input.bool()?),
            TypeId::INT => Value::Int(input.int()?),
            TypeId::UINT => Value::Uint(input.uint()?),
            TypeId::FLOAT => Value::Float(input.float()?),
            TypeId::COMPLEX => Value::Complex(input.float()?, input.float()?),
            TypeId::STRING => Value::String(input.string()?.into_owned()),
            _ => Value::Bytes(input.bytes()?.to_vec()),
        })
    }

    #[inline(never)]
    fn decode_struct(&mut self, fields: &[Field], names: &[Arc<str>], depth: usize) -> Result<Value, Error> {
        let mut values = Vec::with_capacity(fields.len());
        let mut last = None;
        while let Some(index) = self.next_field(&mut last, fields.len())? {
            let value = self.decode_value(fields[index].id, depth + 1).map_err(|e| e.in_field(&names[index]))?;
            values.push((Arc::clone(&names[index]), value));
        }
        Ok(Value::Struct(values))
    }

    #[inline(never)]
    fn decode_list(&mut self, elem: TypeId, count: usize, depth: usize) -> Result<Value, Error> {
        // Every element takes at least a byte: don't trust the count further.
        let mut items = Vec::with_capacity(count.min(self.input.remaining()));
        for index in 0..count {
            items.push(self.decode_value(elem, depth + 1).map_err(|e| e.in_element(index))?);
        }
        Ok(Value::List(items))
    }

    #[inline(never)]
    fn decode_map(&mut self, key: TypeId, elem: TypeId, depth: usize) -> Result<Value, Error> {
        let count = self.input.count()?;
        let mut entries = Vec::with_capacity(count.min(self.input.remaining()));
        for index in 0..count {
            let entry =
                self.decode_value(key, depth + 1).and_then(|key| Ok((key, self.decode_value(elem, depth + 1)?)));
            entries.push(entry.map_err(|e| e.in_entry(index))?);
        }
        Ok(Value::Map(entries))
    }

    #[inline(never)]
    fn decode_interface(&mut self, depth: usize) -> Result<Value, Error> {
        let Some((name, id)) = self.interface_header()? else {
            return Ok(Value::Nil);
        };
        let value = self.decode_value(id, depth + 1)?;
        Ok(Value::Interface { name, value: Box::new(value) })
    }
}
