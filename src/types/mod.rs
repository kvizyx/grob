//! Type ids and type definitions.

mod table;
mod wire;

use std::fmt;

pub(crate) use table::{Entry, TypeTable};
pub(crate) use wire::WIRE_TYPE;

/// Identifies a type within one gob stream.
///
/// Basic types have fixed ids, available as associated constants. Every
/// other type is defined by the stream that uses it: [`Encoder`](crate::Encoder)
/// hands out ids for the types it defines, and a [`Decoder`](crate::Decoder)
/// learns the ids its peer defined. Ids from one stream mean nothing in
/// another.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TypeId(i32);

impl TypeId {
    /// Go's `bool`.
    pub const BOOL: Self = Self(1);
    /// Every Go signed integer type: `int`, `int8`, ..., `int64`.
    pub const INT: Self = Self(2);
    /// Every Go unsigned integer type: `uint`, `uint8`, ..., `uint64`, `uintptr`.
    pub const UINT: Self = Self(3);
    /// Go's `float32` and `float64`.
    pub const FLOAT: Self = Self(4);
    /// Go's `[]byte`.
    pub const BYTES: Self = Self(5);
    /// Go's `string`.
    pub const STRING: Self = Self(6);
    /// Go's `complex64` and `complex128`.
    pub const COMPLEX: Self = Self(7);
    /// Go's interface types, such as `any`.
    pub const INTERFACE: Self = Self(8);

    /// User-defined types get ids from here on; lower ids are reserved by
    /// gob for its basic and bootstrap types (encoding/gob type.go).
    pub(crate) const FIRST_USER: Self = Self(64);

    pub(crate) const fn new(id: i32) -> Self {
        Self(id)
    }

    /// The id as it appears on the wire.
    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }

    pub(crate) fn is_basic(self) -> bool {
        (Self::BOOL.0..=Self::INTERFACE.0).contains(&self.0)
    }

    pub(crate) fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl fmt::Display for TypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl From<TypeId> for i32 {
    fn from(id: TypeId) -> Self {
        id.0
    }
}

/// A struct field: its name and type.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Field {
    /// The field name. Go matches fields by name, so it must be the exported
    /// Go field name, e.g. `"ID"`.
    pub name: String,
    /// The field's type.
    pub id: TypeId,
}

impl Field {
    /// Creates a field named `name` of type `id`.
    pub fn new(name: impl Into<String>, id: TypeId) -> Self {
        Self { name: name.into(), id }
    }
}

/// A type definition, for types other than the basic ones.
///
/// Basic types (bools, integers, floats, complex numbers, strings, `[]byte`
/// and interfaces) need no definition; they are referenced by the constants
/// on [`TypeId`]. Go pointers are flattened: `*T` is described as `T`.
/// Names are informational only: Go ignores type names and matches struct
/// fields by name.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Type {
    /// A fixed-size array, Go's `[len]elem`.
    Array {
        /// The Go type name, e.g. `"[4]int"`.
        name: String,
        /// The element type.
        elem: TypeId,
        /// The number of elements.
        len: usize,
    },
    /// A slice, Go's `[]elem`.
    Slice {
        /// The Go type name, e.g. `"[]string"`.
        name: String,
        /// The element type.
        elem: TypeId,
    },
    /// A struct with its exported fields, in order.
    Struct {
        /// The Go type name, e.g. `"Point"`.
        name: String,
        /// The fields, in the order they are numbered on the wire.
        fields: Vec<Field>,
    },
    /// A map, Go's `map[key]elem`.
    Map {
        /// The Go type name, e.g. `"map[string]int"`.
        name: String,
        /// The key type.
        key: TypeId,
        /// The value type.
        elem: TypeId,
    },
    /// A type implementing `gob.GobEncoder`; its values are opaque bytes.
    GobEncoder {
        /// The Go type name, e.g. `"time.Time"`.
        name: String,
    },
    /// A type implementing `encoding.BinaryMarshaler`; its values are opaque
    /// bytes.
    BinaryMarshaler {
        /// The Go type name, e.g. `"uuid.UUID"`.
        name: String,
    },
    /// A type implementing `encoding.TextMarshaler`; its values are opaque
    /// bytes.
    ///
    /// Part of the wire format, but Go's encoding/gob has its support
    /// disabled: Go never sends it and its decoder rejects it, sending such
    /// types as plain structs instead.
    TextMarshaler {
        /// The Go type name.
        name: String,
    },
}

impl Type {
    /// The Go type name.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Type::Array { name, .. }
            | Type::Slice { name, .. }
            | Type::Struct { name, .. }
            | Type::Map { name, .. }
            | Type::GobEncoder { name }
            | Type::BinaryMarshaler { name }
            | Type::TextMarshaler { name } => name,
        }
    }

    /// Whether values are opaque bytes produced by the Go type itself.
    pub(crate) fn is_external(&self) -> bool {
        matches!(self, Type::GobEncoder { .. } | Type::BinaryMarshaler { .. } | Type::TextMarshaler { .. })
    }
}
