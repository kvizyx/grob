//! Go types that encode themselves (`gob.GobEncoder`,
//! `encoding.BinaryMarshaler`): gob sends their values as opaque bytes, and
//! the receiver must know the type implements the interface.

use std::marker::PhantomData;

use serde::de::{self, Visitor};

/// serde newtype names marking externally encoded values. They start with a
/// NUL byte so no real Rust type has them.
pub(crate) const BINARY_MARSHALER: &str = "\0grob::BinaryMarshaler";
pub(crate) const GOB_ENCODER: &str = "\0grob::GobEncoder";

/// Serializes a field as a Go `encoding.BinaryMarshaler`.
///
/// Use it with `#[serde(with = "grob::binary_marshaler")]` on fields whose Go
/// type implements `MarshalBinary`, such as `uuid.UUID`. The field must
/// serialize as bytes (like `uuid::Uuid` and `[u8; N]` do) or as a string,
/// which is sent as its bytes. When the Go type also implements
/// `GobEncoder`, Go prefers that: use [`gob_encoder`](crate::gob_encoder).
///
/// Go leaves zero values of such types out of structs, so a missing field
/// deserializes as `T::default()`.
///
/// # Examples
///
/// ```
/// #[derive(serde::Serialize, serde::Deserialize)]
/// struct Request {
///     #[serde(rename = "ID", with = "grob::binary_marshaler")]
///     id: [u8; 16], // uuid.UUID
/// }
/// ```
pub mod binary_marshaler {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Serializes `value` as a `BinaryMarshaler`'s bytes.
    ///
    /// # Errors
    ///
    /// When `value` does not serialize as bytes or a string.
    pub fn serialize<T: Serialize + ?Sized, S: Serializer>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_struct(super::BINARY_MARSHALER, value)
    }

    /// Deserializes a `BinaryMarshaler`'s bytes, or `T::default()` when the
    /// field was left out.
    ///
    /// # Errors
    ///
    /// When `T` cannot be deserialized from the bytes.
    pub fn deserialize<'de, T: Deserialize<'de> + Default, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<T, D::Error> {
        deserializer.deserialize_newtype_struct(super::BINARY_MARSHALER, super::External(std::marker::PhantomData))
    }
}

/// Serializes a field as a Go `gob.GobEncoder`.
///
/// Use it with `#[serde(with = "grob::gob_encoder")]` on fields whose Go type
/// implements `GobEncode`, such as `time.Time`. Otherwise it works like
/// [`binary_marshaler`](crate::binary_marshaler).
///
/// # Examples
///
/// ```
/// #[derive(serde::Serialize, serde::Deserialize)]
/// struct Event {
///     #[serde(rename = "Version", with = "grob::gob_encoder")]
///     version: String, // a Go type whose GobEncode writes text like "1.2"
/// }
/// ```
pub mod gob_encoder {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Serializes `value` as a `GobEncoder`'s bytes.
    ///
    /// # Errors
    ///
    /// When `value` does not serialize as bytes or a string.
    pub fn serialize<T: Serialize + ?Sized, S: Serializer>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_struct(super::GOB_ENCODER, value)
    }

    /// Deserializes a `GobEncoder`'s bytes, or `T::default()` when the field
    /// was left out.
    ///
    /// # Errors
    ///
    /// When `T` cannot be deserialized from the bytes.
    pub fn deserialize<'de, T: Deserialize<'de> + Default, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<T, D::Error> {
        deserializer.deserialize_newtype_struct(super::GOB_ENCODER, super::External(std::marker::PhantomData))
    }
}

/// Deserializes an externally encoded value: the value itself, or its
/// default when gob left it out (signalled with `visit_none`).
struct External<T>(PhantomData<T>);

impl<'de, T: de::Deserialize<'de> + Default> Visitor<'de> for External<T> {
    type Value = T;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("an externally encoded value")
    }

    fn visit_none<E: de::Error>(self) -> Result<T, E> {
        Ok(T::default())
    }

    fn visit_newtype_struct<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<T, D::Error> {
        T::deserialize(deserializer)
    }
}
