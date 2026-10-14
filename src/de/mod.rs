//! serde deserialization, straight from gob streams or from [`Value`]s.
//!
//! Gob leaves zero values out of structs, so struct fields missing from the
//! input deserialize as their zero value: 0, false, "", empty collections,
//! `None`, and structs of zeros. No `#[serde(default)]` is needed.
//!
//! Interface values deserialize into enums: the name the concrete type was
//! registered under selects the variant. Strings select unit variants by
//! name, and integers by index, matching Go's string and `iota` constants.

mod stream;
mod value;
mod zero;

use std::io::Read;

use serde::de::{self, Deserialize, DeserializeOwned};

use crate::decode::{ReadFrames, SliceFrames};
use crate::{Decoder, Error, Value};

impl de::Error for Error {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Error::data(msg.to_string())
    }
}

/// Deserializes a `T` from a [`Value`].
///
/// # Errors
///
/// When `value` does not fit `T`.
///
/// # Examples
///
/// ```
/// let value = grob::Value::Struct(vec![("X".into(), 3.into())]);
///
/// #[derive(serde::Deserialize, Debug, PartialEq)]
/// struct Point {
///     #[serde(rename = "X")]
///     x: i64,
///     #[serde(rename = "Y")]
///     y: i64, // left out: zero
/// }
///
/// assert_eq!(grob::from_value::<Point>(value)?, Point { x: 3, y: 0 });
/// # Ok::<(), grob::Error>(())
/// ```
pub fn from_value<T: DeserializeOwned>(value: Value) -> Result<T, Error> {
    T::deserialize(value)
}

/// Deserializes the first value of a self-contained gob stream.
///
/// A shorthand for [`Decoder::deserialize`] with a new decoder. `T` may
/// borrow strings and bytes from `bytes`.
///
/// # Errors
///
/// When `bytes` holds no value, is cut short or is not valid gob data, when
/// the value nests too deeply, or when it does not fit `T`.
pub fn from_slice<'de, T: Deserialize<'de>>(bytes: &'de [u8]) -> Result<T, Error> {
    Decoder::new().deserialize(bytes)
}

impl Decoder {
    /// Deserializes the first value in `bytes` into a `T`.
    ///
    /// It reads straight from the stream, without building [`Value`]s. Type
    /// definitions before the value are remembered for later calls; anything
    /// after the value is ignored.
    ///
    /// `T` may borrow strings and bytes from `bytes`, such as `&str`,
    /// `&[u8]` (with `serde_bytes`) or `Cow<str>` with `#[serde(borrow)]`
    /// fields. A string with invalid UTF-8 cannot be borrowed: it is
    /// replaced in a string of its own, which a `&str` field rejects.
    ///
    /// # Errors
    ///
    /// When `bytes` holds no value, is cut short or is not valid gob data,
    /// when the value nests too deeply, or when it does not fit `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// #[derive(serde::Serialize, serde::Deserialize)]
    /// struct Message<'a> {
    ///     #[serde(rename = "Text")]
    ///     text: &'a str,
    /// }
    ///
    /// let bytes = grob::to_vec(&Message { text: "hello" })?;
    /// let message: Message = grob::Decoder::new().deserialize(&bytes)?;
    /// assert_eq!(message.text, "hello"); // borrowed from `bytes`
    /// # Ok::<(), grob::Error>(())
    /// ```
    pub fn deserialize<'de, T: Deserialize<'de>>(&mut self, bytes: &'de [u8]) -> Result<T, Error> {
        self.read(&mut SliceFrames(bytes), stream::deserialize_top)?.ok_or_else(Error::eof)
    }

    /// Reads the next value from `reader` into a `T`, or returns `None` at
    /// the end of the stream.
    ///
    /// Like [`decode_from`](Self::decode_from), it reads exactly the messages
    /// that make up the value.
    ///
    /// # Errors
    ///
    /// When reading fails, the stream ends in the middle of a value, the data
    /// is not valid gob, the value nests too deeply, or it does not fit `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut encoder = grob::Encoder::new();
    /// encoder.serialize(&1u32)?;
    /// encoder.serialize(&2u32)?;
    /// let stream = encoder.finish();
    ///
    /// let mut decoder = grob::Decoder::new();
    /// let mut reader = stream.as_slice();
    /// assert_eq!(decoder.deserialize_from::<u32>(&mut reader)?, Some(1));
    /// assert_eq!(decoder.deserialize_from::<u32>(&mut reader)?, Some(2));
    /// assert_eq!(decoder.deserialize_from::<u32>(&mut reader)?, None);
    /// # Ok::<(), grob::Error>(())
    /// ```
    pub fn deserialize_from<T: DeserializeOwned>(&mut self, reader: impl Read) -> Result<Option<T>, Error> {
        self.read(&mut ReadFrames(reader), stream::deserialize_top)
    }
}
