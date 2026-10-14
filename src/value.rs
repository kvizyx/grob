use std::sync::Arc;

/// A dynamically typed gob value.
///
/// [`Decoder::decode`](crate::Decoder::decode) produces these, and
/// [`Encoder::encode`](crate::Encoder::encode) writes them against a type
/// definition.
///
/// Gob leaves zero values (0, false, empty strings and collections, nil) out
/// of structs. Decoded structs therefore only hold the fields that were sent,
/// and a missing field means its zero value. When encoding, zero fields are
/// left out the same way.
///
/// # Examples
///
/// ```
/// use grob::Value;
///
/// let point = Value::Struct(vec![("X".into(), 1.into()), ("Y".into(), 2.into())]);
/// assert_eq!(point.get("Y").and_then(Value::as_i64), Some(2));
/// assert_eq!(point.get("Z"), None);
/// ```
#[derive(Clone, PartialEq, Debug)]
pub enum Value {
    /// A `bool`.
    Bool(bool),
    /// Any signed integer.
    Int(i64),
    /// Any unsigned integer.
    Uint(u64),
    /// A `float32` or `float64`.
    Float(f64),
    /// A `complex64` or `complex128`: real and imaginary parts.
    Complex(f64, f64),
    /// A string. Go strings may hold any bytes; invalid UTF-8 is replaced
    /// when decoding. Encode [`Value::Bytes`] as a string to send exact bytes.
    String(String),
    /// A `[]byte`, or the bytes of a Gob/Binary/Text marshaled value.
    Bytes(Vec<u8>),
    /// An array or slice.
    List(Vec<Value>),
    /// A struct: its fields by name. Names are shared between decoded values
    /// of the same type, which saves an allocation per field.
    Struct(Vec<(Arc<str>, Value)>),
    /// A map, as key-value pairs.
    Map(Vec<(Value, Value)>),
    /// A non-nil interface value.
    Interface {
        /// The name its concrete type was registered under with
        /// `gob.Register`, e.g. `"main.Point"` or `"int"`.
        name: String,
        /// The concrete value.
        value: Box<Value>,
    },
    /// A nil interface value. As a struct field, also a field left out.
    Nil,
}

impl Value {
    /// The struct field `name`, if this is a struct and the field is present.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Struct(fields) => fields.iter().find(|(n, _)| **n == *name).map(|(_, value)| value),
            _ => None,
        }
    }

    /// The boolean, if this is one.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The integer, if this is an integer that fits in an `i64`.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Uint(u) => i64::try_from(*u).ok(),
            _ => None,
        }
    }

    /// The integer, if this is a non-negative integer.
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Uint(u) => Some(*u),
            Value::Int(i) => u64::try_from(*i).ok(),
            _ => None,
        }
    }

    /// The float, if this is one.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// The text, if this is a string or bytes holding valid UTF-8.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            Value::Bytes(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    /// The bytes, if this is bytes or a string.
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            Value::String(s) => Some(s.as_bytes()),
            _ => None,
        }
    }

    /// The elements, if this is an array or slice.
    #[must_use]
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(items) => Some(items),
            _ => None,
        }
    }

    /// Whether gob leaves this value out of structs.
    pub(crate) fn is_zero(&self) -> bool {
        match self {
            Value::Nil => true,
            Value::Bool(b) => !b,
            Value::Int(i) => *i == 0,
            Value::Uint(u) => *u == 0,
            Value::Float(f) => *f == 0.0,
            Value::Complex(re, im) => *re == 0.0 && *im == 0.0,
            Value::String(s) => s.is_empty(),
            Value::Bytes(b) => b.is_empty(),
            Value::List(items) => items.is_empty(),
            Value::Map(entries) => entries.is_empty(),
            Value::Struct(_) | Value::Interface { .. } => false,
        }
    }

    /// What kind of value this is, for error messages.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Uint(_) => "uint",
            Value::Float(_) => "float",
            Value::Complex(..) => "complex",
            Value::String(_) => "string",
            Value::Bytes(_) => "bytes",
            Value::List(_) => "list",
            Value::Struct(_) => "struct",
            Value::Map(_) => "map",
            Value::Interface { .. } => "interface",
            Value::Nil => "nil",
        }
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<i8> for Value {
    fn from(value: i8) -> Self {
        Value::Int(value.into())
    }
}

impl From<i16> for Value {
    fn from(value: i16) -> Self {
        Value::Int(value.into())
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Value::Int(value.into())
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<u8> for Value {
    fn from(value: u8) -> Self {
        Value::Uint(value.into())
    }
}

impl From<u16> for Value {
    fn from(value: u16) -> Self {
        Value::Uint(value.into())
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Value::Uint(value.into())
    }
}

impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Value::Uint(value)
    }
}

impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Value::Float(value.into())
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Float(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::String(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::String(value)
    }
}

impl From<&[u8]> for Value {
    fn from(value: &[u8]) -> Self {
        Value::Bytes(value.to_vec())
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Value::Bytes(value)
    }
}

impl From<Vec<Value>> for Value {
    fn from(value: Vec<Value>) -> Self {
        Value::List(value)
    }
}
