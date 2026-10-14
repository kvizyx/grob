//! serde serialization. Gob types are inferred from the values themselves.
//!
//! [`Encoder::serialize`] first tries to write a value straight to gob bytes
//! against the definitions its stream already has, which is all it takes
//! once the stream has seen a value of the same Rust type. When those don't
//! cover the value, it infers the value's types, sends the missing
//! definitions, and writes the value then.
//!
//! Gob needs a definition for every type, but serde only shows values, so
//! an empty `Vec<T>` says nothing about `T`. That rarely matters: like Go,
//! zero fields (0, false, empty strings and collections, `None`) are left out
//! of struct values, and here out of their definitions too. Go matches fields
//! by name and gives missing ones their zero value, so the receiver sees the
//! same data. Collection elements are always sent, so their types are merged
//! across elements; only a collection whose elements are all empty
//! collections cannot be typed.

mod infer;
mod schema;
mod shape;
mod stream;
#[cfg(test)]
mod tests;
mod value;

use serde::ser::{self, Serialize};

pub(crate) use schema::Definitions;
pub(crate) use stream::Scratch;

use self::infer::Infer;
use self::schema::Schema;
use self::stream::WriteError;
use self::value::ValueSerializer;
use crate::{Encoder, Error, TypeId, Value};

impl ser::Error for Error {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Error::data(msg.to_string())
    }
}

/// Serializes `value` into a self-contained gob stream.
///
/// A shorthand for [`Encoder::serialize`] with a new encoder.
///
/// # Errors
///
/// When `value` has no gob equivalent, see [`Encoder::serialize`].
pub fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, Error> {
    let mut encoder = Encoder::new();
    encoder.serialize(value)?;
    Ok(encoder.finish())
}

/// Converts `value` into a [`Value`].
///
/// Zero struct fields are left out, as [`Encoder::serialize`] would leave
/// them out of the stream.
///
/// # Errors
///
/// When `value` has no gob equivalent, see [`Encoder::serialize`].
pub fn to_value<T: Serialize + ?Sized>(value: &T) -> Result<Value, Error> {
    Ok(value.serialize(ValueSerializer)?.value)
}

impl Encoder {
    /// Serializes `value` and appends it to the stream, sending the type
    /// definitions it needs first.
    ///
    /// Rust types map to Go types as follows:
    ///
    /// * Structs are Go structs, matched by field name: use
    ///   `#[serde(rename)]` or `#[serde(rename_all = "PascalCase")]` to match
    ///   Go's exported names.
    /// * Signed integers are Go's `int` types and unsigned integers its
    ///   `uint` types; Go rejects one for the other.
    /// * `Option<T>` is a pointer: `None` leaves the field out.
    /// * `Vec<T>` and other sequences are slices; `Vec<u8>` is a `[]byte`.
    ///   Tuples and arrays like `[T; N]` are Go arrays, so their elements
    ///   must share a type.
    /// * Maps are maps.
    /// * Newtype and struct enum variants are interface values whose
    ///   concrete type is registered under the variant's name, so rename
    ///   variants to the names the Go side registered, e.g. `"main.Point"`
    ///   or `"int"`. Unit variants are strings.
    /// * Fields marked with [`binary_marshaler`](crate::binary_marshaler) or
    ///   [`gob_encoder`](crate::gob_encoder) are Go types that encode
    ///   themselves.
    ///
    /// Definitions are reused by later values that fit them; values of a
    /// Rust type the stream has seen before are written without working out
    /// types again.
    ///
    /// # Errors
    ///
    /// When the value has no gob equivalent: it is `None` where gob cannot
    /// send nil (at the top level, or as an element), it holds values of
    /// different types where gob needs one type (elements of a sequence),
    /// it holds a tuple enum variant or a collection whose element type
    /// cannot be told (see the module docs), or an integer does not fit 64
    /// bits. Nothing is appended then.
    ///
    /// # Examples
    ///
    /// ```
    /// use serde::Serialize;
    ///
    /// #[derive(Serialize)]
    /// #[serde(rename_all = "PascalCase")]
    /// struct Message {
    ///     text: String,
    ///     tags: Vec<String>,
    /// }
    ///
    /// let mut encoder = grob::Encoder::new();
    /// encoder.serialize(&Message { text: "hi".into(), tags: vec![] })?;
    /// // The second message reuses the definitions sent with the first.
    /// encoder.serialize(&Message { text: "there".into(), tags: vec!["x".into()] })?;
    /// # Ok::<(), grob::Error>(())
    /// ```
    pub fn serialize<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        let rust_type = std::any::type_name::<T>();
        if !self.is_fresh() {
            return self.serialize_typed(rust_type, value);
        }
        // A new stream: start from what a new stream defined for a value of
        // this Rust type before.
        let start = self.stream_len();
        let restored = self.restore_fresh(rust_type);
        match self.serialize_typed(rust_type, value) {
            Ok(()) => {
                self.remember_fresh(rust_type, start, restored);
                Ok(())
            }
            Err(error) => {
                self.rewind_fresh(start);
                Err(error)
            }
        }
    }

    /// Serializes `value`, of type `rust_type`.
    fn serialize_typed<T: Serialize + ?Sized>(&mut self, rust_type: &'static str, value: &T) -> Result<(), Error> {
        // Usually the stream already has every type the value needs: the
        // ones a previous value of the same Rust type got.
        if let Some(&root) = self.serde().roots.get(rust_type) {
            match self.write((root, false), value) {
                Ok(()) => {
                    #[cfg(test)]
                    tests::count(&tests::DIRECT);
                    return Ok(());
                }
                Err(WriteError::Failed(error)) => return Err(error),
                Err(WriteError::Mismatch) => {}
            }
        }
        #[cfg(test)]
        tests::count(&tests::INFERRED);

        let mut schema = Schema::default();
        let inferred = value.serialize(Infer(&mut schema))?;
        if inferred.nil {
            return Err(Error::data("cannot encode a nil value"));
        }
        let root = self.define_inferred(schema, &inferred.shape)?;
        self.serde_mut().roots.insert(rust_type, root);
        match self.write((root, false), value) {
            Ok(()) => Ok(()),
            Err(WriteError::Failed(error)) => Err(error),
            // The definitions cover the value, so this only happens when
            // serializing it twice gives different results. Build it instead.
            Err(WriteError::Mismatch) => {
                #[cfg(test)]
                tests::count(&tests::BUILT);
                let value = value.serialize(ValueSerializer)?.value;
                self.encode(root, &value)
            }
        }
    }

    /// Serializes `value` as type `id`, which the stream already defines,
    /// and appends it to the stream.
    ///
    /// Use this when the Go side expects particular types: declare them
    /// with [`define`](Self::define) and the like, then serialize values
    /// against them. Nothing is inferred, so empty collections and
    /// differently signed integers are no problem. The value must fit the
    /// declared type as Go would decode it:
    ///
    /// * structs match by field names, whatever their Rust name; fields the
    ///   type lacks must be zero, and the type's other fields may be
    ///   missing;
    /// * integers fit any integer type that holds their value, so a
    ///   non-negative `i64` can be sent as a `uint`;
    /// * sequences fill slices, and arrays of their length;
    /// * strings, byte sequences and `[u8; N]` fill `[]byte` and types that
    ///   encode themselves;
    /// * enum variants are interface values of types
    ///   [`register`](Self::register)ed under the variant's name.
    ///
    /// # Errors
    ///
    /// When `id` is not defined, or `value` does not fit it; the error's
    /// [`path`](Error::path) tells where. Nothing is appended then.
    ///
    /// # Examples
    ///
    /// ```
    /// use grob::{Encoder, TypeId};
    /// use serde::Serialize;
    ///
    /// #[derive(Serialize)]
    /// struct Request {
    ///     #[serde(rename = "ID")]
    ///     id: i32,
    ///     #[serde(rename = "Tags")]
    ///     tags: Vec<String>,
    /// }
    ///
    /// let mut encoder = Encoder::new();
    /// let tags = encoder.define_slice("", TypeId::STRING);
    /// let request = encoder.define_struct("Request", &[("ID", TypeId::UINT), ("Tags", tags)]);
    /// encoder.serialize_as(request, &Request { id: 7, tags: vec![] })?;
    ///
    /// let value = grob::decode(&encoder.finish())?;
    /// assert_eq!(value.get("ID"), Some(&grob::Value::Uint(7)));
    /// # Ok::<(), grob::Error>(())
    /// ```
    pub fn serialize_as<T: Serialize + ?Sized>(&mut self, id: TypeId, value: &T) -> Result<(), Error> {
        if id == TypeId::INTERFACE || (!id.is_basic() && self.types().get(id).is_none()) {
            return Err(Error::data(format!("cannot serialize as type {id}, which is not defined")));
        }
        match self.write((id, true), value) {
            Ok(()) => Ok(()),
            Err(WriteError::Failed(error)) => Err(error),
            // Declared types report what does not fit as errors.
            Err(WriteError::Mismatch) => Err(Error::data("value does not fit the declared type")),
        }
    }

    /// Writes a value message for `value` against the stream's definitions.
    /// On error nothing is written.
    fn write<T: Serialize + ?Sized>(&mut self, root: (TypeId, bool), value: &T) -> Result<(), WriteError> {
        let mut scratch = std::mem::take(self.serde_scratch());
        let result = self.message(root.0, |encoder, w| stream::write_top(&*encoder, w, &mut scratch, root, value));
        *self.serde_scratch() = scratch;
        result
    }
}
