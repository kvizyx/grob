//! The types inferred for one value, and how an encoder turns them into
//! definitions on its stream.
//!
//! Struct types are identified by their serde name. All values of a struct
//! within one serialized value share one definition, holding the union of
//! their non-zero fields; that also makes recursive types work. Across a
//! stream, a definition is reused while it covers a new value's fields, and
//! replaced by a wider one otherwise.

use std::collections::{BTreeSet, HashMap};
use std::hash::{BuildHasherDefault, Hasher};

use super::shape::Shape;
use crate::{Encoder, Error, Field, Type, TypeId};

/// A struct's fields by name, in order of first appearance.
pub(super) type Fields = Vec<(&'static str, Shape)>;

fn merge_field(into: &mut Fields, name: &'static str, shape: Shape) -> Result<(), Error> {
    if let Some((_, existing)) = into.iter_mut().find(|(n, _)| *n == name) {
        return existing.merge(shape);
    }
    into.push((name, shape));
    Ok(())
}

fn covers(defined: &Fields, fields: &Fields) -> bool {
    fields.iter().all(|(name, shape)| defined.iter().any(|(n, s)| n == name && s.covers(shape)))
}

/// A hasher for names from the Rust program itself (serde's `&'static str`
/// names): much cheaper than the standard library's hasher, but not resistant
/// to crafted collisions, so never for keys from the input.
#[derive(Default, Clone, Copy)]
struct NameHasher(u64);

impl Hasher for NameHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for chunk in chunks {
            self.write_u64(u64::from_le_bytes(*chunk));
        }
        let mut tail = [0; 8];
        tail[..rest.len()].copy_from_slice(rest);
        self.write_u64(u64::from_le_bytes(tail));
    }

    fn write_u64(&mut self, i: u64) {
        // The 64-bit golden ratio, as in Fibonacci hashing.
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

/// The types found while serializing one value.
#[derive(Default)]
pub(super) struct Schema {
    /// Struct names and the union of their non-zero fields.
    structs: Vec<(&'static str, Fields)>,
    index: HashMap<&'static str, usize, BuildHasherDefault<NameHasher>>,
    /// Interface concrete type names, with the merged shape of their values.
    interfaces: Vec<(&'static str, Shape)>,
}

impl Schema {
    /// Where the fields of struct `name` are collected.
    #[inline]
    pub fn struct_index(&mut self, name: &'static str) -> usize {
        *self.index.entry(name).or_insert_with(|| {
            self.structs.push((name, Vec::new()));
            self.structs.len() - 1
        })
    }

    #[inline]
    pub fn add_field(&mut self, index: usize, field: &'static str, shape: Shape) -> Result<(), Error> {
        let (_, fields) = &mut self.structs[index];
        merge_field(fields, field, shape)
    }

    pub fn add_interface(&mut self, name: &'static str, shape: Shape) -> Result<(), Error> {
        if let Some((_, existing)) = self.interfaces.iter_mut().find(|(n, _)| *n == name) {
            return existing.merge(shape);
        }
        self.interfaces.push((name, shape));
        Ok(())
    }

    fn shapes(&self) -> impl Iterator<Item = &Shape> {
        self.interfaces.iter().chain(self.structs.iter().flat_map(|(_, fields)| fields)).map(|(_, shape)| shape)
    }
}

/// The definitions an encoder sent for serialized values.
#[derive(Clone, Default)]
pub(crate) struct Definitions {
    structs: HashMap<&'static str, (TypeId, Fields)>,
    /// Slices, arrays, maps and external types, which have no name.
    anonymous: HashMap<Type, TypeId>,
    /// The type last used for each Rust type, by `std::any::type_name`.
    pub roots: HashMap<&'static str, TypeId>,
}

/// A struct that needs a definition: its name, whether the stream's current
/// one falls short, and the fields it needs.
struct Planned {
    name: &'static str,
    redefine: bool,
    fields: Fields,
}

impl Encoder {
    /// Sends the definitions a value with `schema` and type `shape` needs,
    /// and returns the value's type id. On error nothing is sent.
    pub(super) fn define_inferred(&mut self, schema: Schema, shape: &Shape) -> Result<TypeId, Error> {
        shape.check_known()?;
        schema.shapes().try_for_each(Shape::check_known)?;
        let plan = self.plan(schema.structs)?;
        self.define_structs(plan);
        for (name, shape) in &schema.interfaces {
            let id = self.resolve(shape);
            if self.registered(name) != Some(id) {
                self.register(*name, id);
            }
        }
        Ok(self.resolve(shape))
    }

    /// Works out which structs need a new definition.
    fn plan(&self, structs: Vec<(&'static str, Fields)>) -> Result<Vec<Planned>, Error> {
        let definitions = &self.serde().structs;
        let mut plan = structs
            .into_iter()
            .map(|(name, fields)| {
                let (redefine, fields) = match definitions.get(name) {
                    Some((_, defined)) if covers(defined, &fields) => (false, defined.clone()),
                    Some((_, defined)) => {
                        let mut union = defined.clone();
                        for (field, shape) in fields {
                            merge_field(&mut union, field, shape)
                                .map_err(|e| e.in_field(field).context(format_args!("struct {name}")))?;
                        }
                        (true, union)
                    }
                    None => (true, fields),
                };
                Ok(Planned { name, redefine, fields })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        // A struct referring to a redefined struct needs redefining too.
        loop {
            let redefined: BTreeSet<&str> = plan.iter().filter(|s| s.redefine).map(|s| s.name).collect();
            let mut changed = false;
            for planned in plan.iter_mut().filter(|s| !s.redefine) {
                if planned.fields.iter().any(|(_, shape)| shape.references(&redefined)) {
                    planned.redefine = true;
                    changed = true;
                }
            }
            if !changed {
                return Ok(plan);
            }
        }
    }

    fn define_structs(&mut self, plan: Vec<Planned>) {
        let redefined: Vec<Planned> = plan.into_iter().filter(|s| s.redefine).collect();
        // Reserve every id first: the definitions may refer to each other.
        let ids: Vec<TypeId> = redefined.iter().map(|_| self.reserve_id()).collect();
        for (planned, &id) in redefined.iter().zip(&ids) {
            self.serde_mut().structs.insert(planned.name, (id, planned.fields.clone()));
        }
        for (planned, id) in redefined.into_iter().zip(ids) {
            let fields = planned.fields.iter().map(|(name, shape)| Field::new(*name, self.resolve(shape))).collect();
            self.define_at(id, Type::Struct { name: planned.name.to_owned(), fields });
        }
    }

    /// The type id for `shape`, defining it if needed. Struct shapes must be
    /// defined already, and the shape must be known.
    fn resolve(&mut self, shape: &Shape) -> TypeId {
        let ty = match shape {
            Shape::Bool => return TypeId::BOOL,
            Shape::Int => return TypeId::INT,
            Shape::Uint | Shape::Byte => return TypeId::UINT,
            Shape::Float => return TypeId::FLOAT,
            Shape::String => return TypeId::STRING,
            Shape::Bytes => return TypeId::BYTES,
            Shape::Interface => return TypeId::INTERFACE,
            Shape::Seq(elem) if **elem == Shape::Byte => return TypeId::BYTES,
            Shape::Struct(name) => {
                return self.serde().structs.get(name).expect("structs are defined before their shapes resolve").0;
            }
            Shape::Unknown => unreachable!("shapes are checked to be known before resolving"),
            Shape::Seq(elem) => Type::Slice { name: String::new(), elem: self.resolve(elem) },
            Shape::Array(elem, len) => Type::Array { name: String::new(), elem: self.resolve(elem), len: *len },
            Shape::Map(key, elem) => {
                Type::Map { name: String::new(), key: self.resolve(key), elem: self.resolve(elem) }
            }
            Shape::GobEncoder => Type::GobEncoder { name: String::new() },
            Shape::BinaryMarshaler => Type::BinaryMarshaler { name: String::new() },
        };
        if let Some(&id) = self.serde().anonymous.get(&ty) {
            return id;
        }
        let id = self.define(ty.clone());
        self.serde_mut().anonymous.insert(ty, id);
        id
    }
}
