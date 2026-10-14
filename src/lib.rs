//! Go's [`encoding/gob`](https://pkg.go.dev/encoding/gob) format, for
//! exchanging data with Go programs.
//!
//! Gob streams are self-describing: type definitions travel ahead of the
//! values that use them, and every Go type has a gob equivalent. This crate
//! reads and writes such streams in two ways:
//!
//! * With [serde](https://serde.rs) (the default `serde` feature): Rust types
//!   map to Go types, and gob type definitions are worked out from the
//!   values. Use [`to_vec`] and [`from_slice`], or [`Encoder::serialize`] and
//!   [`Decoder::deserialize_from`] for streams. To send the exact types the
//!   Go side expects, declare them and use [`Encoder::serialize_as`].
//!   Strings and byte slices can borrow from the input.
//! * Dynamically: [`Value`]s encoded against [`Type`] definitions you
//!   declare with an [`Encoder`], and decoded by a [`Decoder`] that
//!   understands any gob stream.
//!
//! # How Go types map to gob
//!
//! Go matches struct fields by name: fields the receiver does not know are
//! skipped, and fields that were not sent keep their zero value. Gob relies
//! on this by leaving zero fields (0, false, empty strings and collections,
//! nil) out entirely. Type names are informational only, and pointers are
//! flattened: `*T` travels as `T`.
//!
//! Interface values (Go's `any` and other interface types) carry the name
//! their concrete type was registered under with `gob.Register`, such as
//! `"main.Point"`. Go registers its basic types, like `"int"` or
//! `"[]string"`, out of the box.
//!
//! # Errors
//!
//! [`Error`]s tell what went wrong, where in the value
//! ([`Error::path`], such as `Items[2].Name`), and for decoding, at which
//! byte of the stream ([`Error::offset`]).
//!
//! # Examples
//!
//! With serde:
//!
//! ```
//! # #[cfg(feature = "serde")] {
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize, Debug, PartialEq)]
//! #[serde(rename_all = "PascalCase")] // Go's exported field names
//! struct Point {
//!     x: i64,
//!     y: i64,
//! }
//!
//! let bytes = grob::to_vec(&Point { x: 1, y: 2 })?;
//! assert_eq!(grob::from_slice::<Point>(&bytes)?, Point { x: 1, y: 2 });
//! # }
//! # Ok::<(), grob::Error>(())
//! ```
//!
//! With values and declared types:
//!
//! ```
//! use grob::{Encoder, TypeId, Value};
//!
//! let mut encoder = Encoder::new();
//! let point = encoder.define_struct("Point", &[("X", TypeId::INT), ("Y", TypeId::INT)]);
//! let request = encoder.define_struct("Request", &[("ID", TypeId::STRING), ("At", point), ("Any", TypeId::INTERFACE)]);
//! encoder.register("main.Point", point);
//!
//! let value = Value::Struct(vec![
//!     ("ID".into(), "abc".into()),
//!     ("At".into(), Value::Struct(vec![("X".into(), 1.into()), ("Y".into(), 2.into())])),
//!     ("Any".into(), Value::Interface { name: "int".into(), value: Box::new(42.into()) }),
//! ]);
//! encoder.encode(request, &value)?;
//! let bytes = encoder.finish();
//!
//! assert_eq!(grob::decode(&bytes)?, value);
//! # Ok::<(), grob::Error>(())
//! ```

#[cfg(feature = "serde")]
mod de;
mod decode;
mod encode;
mod error;
#[cfg(feature = "serde")]
mod external;
#[cfg(feature = "serde")]
mod ser;
mod types;
mod value;
mod wire;

#[cfg(feature = "serde")]
#[doc(inline)]
pub use de::{from_slice, from_value};
#[doc(inline)]
pub use decode::Decoder;
#[doc(inline)]
pub use encode::Encoder;
#[doc(inline)]
pub use error::Error;
#[cfg(feature = "serde")]
#[doc(inline)]
pub use external::{binary_marshaler, gob_encoder};
#[cfg(feature = "serde")]
#[doc(inline)]
pub use ser::{to_value, to_vec};
#[doc(inline)]
pub use types::{Field, Type, TypeId};
#[doc(inline)]
pub use value::Value;

/// Decodes the first value of a self-contained gob stream.
///
/// A shorthand for [`Decoder::decode`] with a new decoder.
///
/// # Errors
///
/// When `bytes` holds no value, is cut short or is not valid gob data, or
/// when the value nests too deeply.
pub fn decode(bytes: &[u8]) -> Result<Value, Error> {
    Decoder::new().decode(bytes)
}
