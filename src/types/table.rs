//! The types one stream knows.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::wire::BOOTSTRAP;
use super::{Type, TypeId};

/// A type definition plus what encoding and decoding need at hand.
#[derive(Debug)]
pub(crate) struct Entry {
    pub ty: Type,
    /// Struct field names, shared by every decoded value of the type.
    pub field_names: Box<[Arc<str>]>,
}

impl Entry {
    pub fn new(ty: Type) -> Self {
        let field_names = match &ty {
            Type::Struct { fields, .. } => fields.iter().map(|field| Arc::from(field.name.as_str())).collect(),
            _ => Box::default(),
        };
        Self { ty, field_names }
    }

    #[inline]
    pub fn is_struct(&self) -> bool {
        matches!(self.ty, Type::Struct { .. })
    }
}

/// gob's bootstrap types plus the types a stream defined.
///
/// The peer picks the ids, so this is a B-tree rather than a fast hash map:
/// as quick for the handful of types a stream has, without a worst case a
/// peer could exploit.
#[derive(Clone, Debug, Default)]
pub(crate) struct TypeTable {
    defined: BTreeMap<TypeId, Arc<Entry>>,
}

impl TypeTable {
    #[inline]
    pub fn get(&self, id: TypeId) -> Option<&Arc<Entry>> {
        if id < TypeId::FIRST_USER { BOOTSTRAP.get(&id) } else { self.defined.get(&id) }
    }

    pub fn contains(&self, id: TypeId) -> bool {
        self.get(id).is_some()
    }

    /// Adds a definition under a free user id.
    pub fn insert(&mut self, id: TypeId, ty: Type) {
        debug_assert!(id >= TypeId::FIRST_USER, "type {id} is in the reserved range");
        self.defined.insert(id, Arc::new(Entry::new(ty)));
    }

    #[inline]
    pub fn is_struct(&self, id: TypeId) -> bool {
        self.get(id).is_some_and(|entry| entry.is_struct())
    }
}
