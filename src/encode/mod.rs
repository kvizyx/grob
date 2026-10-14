//! Writing gob streams.

mod value;

#[cfg(feature = "serde")]
pub(crate) use value::field_delta;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::types::{Type, TypeTable, WIRE_TYPE};
use crate::wire::{Writer, push_uint};
use crate::{Error, Field, TypeId, Value};

/// Concrete types Go registers for interface values out of the box (type.go
/// `registerBasics`), with the basic type ids they map to.
const BASIC_NAMES: &[(&str, TypeId)] = &[
    ("bool", TypeId::BOOL),
    ("int", TypeId::INT),
    ("int8", TypeId::INT),
    ("int16", TypeId::INT),
    ("int32", TypeId::INT),
    ("int64", TypeId::INT),
    ("uint", TypeId::UINT),
    ("uint8", TypeId::UINT),
    ("uint16", TypeId::UINT),
    ("uint32", TypeId::UINT),
    ("uint64", TypeId::UINT),
    ("uintptr", TypeId::UINT),
    ("float32", TypeId::FLOAT),
    ("float64", TypeId::FLOAT),
    ("complex64", TypeId::COMPLEX),
    ("complex128", TypeId::COMPLEX),
    ("string", TypeId::STRING),
    ("[]uint8", TypeId::BYTES),
];

/// Slice types Go registers out of the box, with their element types. They
/// need a definition, which is sent the first time one is used.
const BASIC_SLICES: &[(&str, TypeId)] = &[
    ("[]bool", TypeId::BOOL),
    ("[]int", TypeId::INT),
    ("[]int8", TypeId::INT),
    ("[]int16", TypeId::INT),
    ("[]int32", TypeId::INT),
    ("[]int64", TypeId::INT),
    ("[]uint", TypeId::UINT),
    ("[]uint16", TypeId::UINT),
    ("[]uint32", TypeId::UINT),
    ("[]uint64", TypeId::UINT),
    ("[]uintptr", TypeId::UINT),
    ("[]float32", TypeId::FLOAT),
    ("[]float64", TypeId::FLOAT),
    ("[]complex64", TypeId::COMPLEX),
    ("[]complex128", TypeId::COMPLEX),
    ("[]string", TypeId::STRING),
];

/// Writes gob streams, like Go's `gob.Encoder`.
///
/// A stream carries type definitions ahead of the values that use them.
/// Define the types of [`Value`]s before encoding them; later values reuse
/// the definitions. With the `serde` feature,
/// [`serialize`](Self::serialize) works out the definitions on its own.
///
/// The encoder collects the stream in memory. Send it whole with
/// [`finish`](Self::finish), or piece by piece with [`take`](Self::take) to
/// talk to a long-lived Go decoder, for instance over a connection.
///
/// # Examples
///
/// ```
/// use grob::{Encoder, TypeId, Value};
///
/// let mut encoder = Encoder::new();
/// let point = encoder.define_struct("Point", &[("X", TypeId::INT), ("Y", TypeId::INT)]);
/// encoder.encode(point, &Value::Struct(vec![("X".into(), 1.into()), ("Y".into(), 2.into())]))?;
/// let stream = encoder.finish();
///
/// let decoded = grob::decode(&stream)?;
/// assert_eq!(decoded.get("Y"), Some(&Value::Int(2)));
/// # Ok::<(), grob::Error>(())
/// ```
#[derive(Default)]
pub struct Encoder {
    /// What the stream has defined. Shared with the snapshots of fresh
    /// streams, and copied when it changes.
    state: Arc<State>,
    stream: Vec<u8>,
    /// Reused for building each message.
    scratch: Vec<u8>,
    /// Where the last value message starts in `stream`, after the
    /// definitions sent for it.
    #[cfg(feature = "serde")]
    value_at: usize,
    #[cfg(feature = "serde")]
    serde_scratch: crate::ser::Scratch,
    /// For each Rust type serialized at the start of a stream, what the
    /// stream defined for it.
    #[cfg(feature = "serde")]
    fresh: HashMap<&'static str, Fresh>,
}

/// The types a stream defined.
#[derive(Clone)]
pub(crate) struct State {
    next_id: TypeId,
    types: TypeTable,
    /// Interface concrete type names (`gob.Register`) to type ids, on top of
    /// [`BASIC_NAMES`].
    names: HashMap<String, TypeId>,
    #[cfg(feature = "serde")]
    serde: crate::ser::Definitions,
}

impl Default for State {
    fn default() -> Self {
        Self {
            next_id: TypeId::FIRST_USER,
            types: TypeTable::default(),
            names: HashMap::new(),
            #[cfg(feature = "serde")]
            serde: crate::ser::Definitions::default(),
        }
    }
}

/// What a fresh stream defined for a value: the encoder's state after, and
/// the definition messages sent.
#[cfg(feature = "serde")]
pub(crate) struct Fresh {
    state: Arc<State>,
    definitions: Box<[u8]>,
}

impl fmt::Debug for Encoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Encoder")
            .field("next_type_id", &self.state.next_id)
            .field("buffered_bytes", &self.stream.len())
            .finish_non_exhaustive()
    }
}

impl Encoder {
    /// Creates an encoder for a new stream.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The id the next defined type will get.
    ///
    /// Self-referencing types need it, since their definition refers to
    /// their own id:
    ///
    /// ```
    /// use grob::{Encoder, TypeId};
    ///
    /// let mut encoder = Encoder::new();
    /// // type Node struct { Value int; Next *Node }
    /// let node = encoder.next_type_id();
    /// assert_eq!(encoder.define_struct("Node", &[("Value", TypeId::INT), ("Next", node)]), node);
    /// ```
    #[must_use]
    pub fn next_type_id(&self) -> TypeId {
        self.state.next_id
    }

    /// Sends the definition of `ty` and returns its id.
    ///
    /// Types it refers to must be defined before values using it are
    /// encoded, not necessarily before it.
    ///
    /// # Panics
    ///
    /// If `ty` is an array longer than `i64::MAX` elements, which gob cannot
    /// describe.
    pub fn define(&mut self, ty: Type) -> TypeId {
        let id = self.reserve_id();
        self.define_at(id, ty);
        id
    }

    /// Sends the definition of a struct named `name` with `fields`, given as
    /// field names and types, and returns its id.
    pub fn define_struct(&mut self, name: &str, fields: &[(&str, TypeId)]) -> TypeId {
        let fields = fields.iter().map(|&(name, id)| Field::new(name, id)).collect();
        self.define(Type::Struct { name: name.to_owned(), fields })
    }

    /// Sends the definition of a slice of `elem` named `name` and returns its id.
    pub fn define_slice(&mut self, name: &str, elem: TypeId) -> TypeId {
        self.define(Type::Slice { name: name.to_owned(), elem })
    }

    /// Sends the definition of an array of `len` `elem`s named `name` and
    /// returns its id.
    ///
    /// # Panics
    ///
    /// If `len` exceeds `i64::MAX`, which gob cannot describe.
    pub fn define_array(&mut self, name: &str, elem: TypeId, len: usize) -> TypeId {
        self.define(Type::Array { name: name.to_owned(), elem, len })
    }

    /// Sends the definition of a map from `key` to `elem` named `name` and
    /// returns its id.
    pub fn define_map(&mut self, name: &str, key: TypeId, elem: TypeId) -> TypeId {
        self.define(Type::Map { name: name.to_owned(), key, elem })
    }

    /// Makes `id` the concrete type of interface values named `name`, like
    /// `gob.Register`.
    ///
    /// The name must be the one the Go side registered, e.g. `"main.Point"`
    /// or `"*main.Point"`. Go's basic types, such as `"int"`, `"string"` or
    /// `"[]string"`, are registered already.
    pub fn register(&mut self, name: impl Into<String>, id: TypeId) {
        Arc::make_mut(&mut self.state).names.insert(name.into(), id);
    }

    /// Appends `value`, encoded as type `id`.
    ///
    /// Struct fields are matched by name and may come in any order; zero
    /// fields are left out like Go does.
    ///
    /// # Errors
    ///
    /// When `value` does not match type `id`, `id` is not defined, an
    /// interface value names an unregistered type, or the value is nil where
    /// gob cannot send nil. Nothing is appended then.
    pub fn encode(&mut self, id: TypeId, value: &Value) -> Result<(), Error> {
        self.message(id, |encoder, w| encoder.write_top(w, id, value))
    }

    /// Returns the bytes produced so far and empties the buffer.
    ///
    /// Defined types stay defined, so later values must go to the same
    /// decoder as the earlier ones.
    pub fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.stream)
    }

    /// Returns all bytes produced.
    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        self.stream
    }

    /// Starts a new stream, for a new decoder: forgets the types defined
    /// and registered, so later values send their definitions again.
    ///
    /// Bytes not yet [`take`](Self::take)n stay in the buffer, so take them
    /// first, unless the receiver knows where one stream ends.
    ///
    /// Use this for sending each value as a stream of its own, as Go code
    /// does when it creates a `gob.Encoder` per message. With the `serde`
    /// feature, the encoder remembers what each Rust type needed at the
    /// start of a stream, so serializing a value of the same type on a new
    /// stream does not work out its types again.
    ///
    /// # Examples
    ///
    /// ```
    /// use grob::{Encoder, TypeId, Value};
    ///
    /// let mut encoder = Encoder::new();
    /// let list = encoder.define_slice("", TypeId::INT);
    /// encoder.encode(list, &Value::List(vec![1.into()]))?;
    /// let first = encoder.take();
    ///
    /// encoder.reset();
    /// // The new stream needs the definition again.
    /// let list = encoder.define_slice("", TypeId::INT);
    /// encoder.encode(list, &Value::List(vec![2.into()]))?;
    /// let second = encoder.take();
    ///
    /// assert_eq!(grob::decode(&second)?, Value::List(vec![2.into()]));
    /// # Ok::<(), grob::Error>(())
    /// ```
    pub fn reset(&mut self) {
        self.state = Arc::default();
    }

    #[cfg(feature = "serde")]
    #[inline]
    pub(crate) fn types(&self) -> &TypeTable {
        &self.state.types
    }

    /// What was defined for serialized values.
    #[cfg(feature = "serde")]
    pub(crate) fn serde(&self) -> &crate::ser::Definitions {
        &self.state.serde
    }

    #[cfg(feature = "serde")]
    pub(crate) fn serde_mut(&mut self) -> &mut crate::ser::Definitions {
        &mut Arc::make_mut(&mut self.state).serde
    }

    #[cfg(feature = "serde")]
    pub(crate) fn serde_scratch(&mut self) -> &mut crate::ser::Scratch {
        &mut self.serde_scratch
    }

    /// Whether the stream has no types defined or registered yet.
    #[cfg(feature = "serde")]
    #[inline]
    pub(crate) fn is_fresh(&self) -> bool {
        self.state.next_id == TypeId::FIRST_USER && self.state.names.is_empty()
    }

    #[cfg(feature = "serde")]
    #[inline]
    pub(crate) fn stream_len(&self) -> usize {
        self.stream.len()
    }

    /// Makes a fresh stream define what it did for `rust_type` before, if
    /// anything.
    #[cfg(feature = "serde")]
    pub(crate) fn restore_fresh(&mut self, rust_type: &str) {
        if let Some(fresh) = self.fresh.get(rust_type) {
            self.state = Arc::clone(&fresh.state);
            self.stream.extend_from_slice(&fresh.definitions);
        }
    }

    /// Remembers what the stream, fresh at `start`, defined for the value
    /// just written of `rust_type`.
    #[cfg(feature = "serde")]
    pub(crate) fn remember_fresh(&mut self, rust_type: &'static str, start: usize) {
        // Any change to the state copies it, so an unchanged state is still
        // the one remembered. Not every change sends definitions: an
        // interface name may be registered for a type already defined.
        if self.fresh.get(rust_type).is_some_and(|fresh| Arc::ptr_eq(&fresh.state, &self.state)) {
            return;
        }
        let definitions = self.stream[start..self.value_at].into();
        self.fresh.insert(rust_type, Fresh { state: Arc::clone(&self.state), definitions });
    }

    /// Takes back everything since a fresh stream's `start`.
    #[cfg(feature = "serde")]
    pub(crate) fn rewind_fresh(&mut self, start: usize) {
        self.stream.truncate(start);
        self.reset();
    }

    /// Appends a value message of type `id` whose body `build` writes. On
    /// error nothing is appended.
    pub(crate) fn message<T, E>(
        &mut self,
        id: TypeId,
        build: impl FnOnce(&mut Self, &mut Writer) -> Result<T, E>,
    ) -> Result<T, E> {
        self.message_with_id(id.get().into(), build)
    }

    fn message_with_id<T, E>(
        &mut self,
        id: i64,
        build: impl FnOnce(&mut Self, &mut Writer) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut w = Writer::from_vec(std::mem::take(&mut self.scratch));
        w.int(id);
        let result = build(self, &mut w);
        let body = w.into_inner();
        if result.is_ok() {
            #[cfg(feature = "serde")]
            if id > 0 {
                self.value_at = self.stream.len();
            }
            push_uint(&mut self.stream, body.len() as u64);
            self.stream.extend_from_slice(&body);
        }
        self.scratch = body;
        result
    }

    pub(crate) fn reserve_id(&mut self) -> TypeId {
        let state = Arc::make_mut(&mut self.state);
        let id = state.next_id;
        state.next_id = id.next();
        id
    }

    /// Defines a type under an id from [`reserve_id`](Self::reserve_id).
    ///
    /// Go only needs a type defined before a value uses it, so ids may be
    /// defined in any order.
    pub(crate) fn define_at(&mut self, id: TypeId, ty: Type) {
        let definition = ty.to_wire(id);
        self.message_with_id(-i64::from(id.get()), |encoder, w| encoder.write_value(w, WIRE_TYPE, &definition))
            .expect("type definitions are well-formed values of gob's wireType");
        Arc::make_mut(&mut self.state).types.insert(id, ty);
    }

    /// The type registered for interface values named `name`, if any.
    #[inline]
    pub(crate) fn registered(&self, name: &str) -> Option<TypeId> {
        self.state.names.get(name).copied().or_else(|| BASIC_NAMES.iter().find(|(n, _)| *n == name).map(|&(_, id)| id))
    }

    /// The concrete type for interface values named `name`, defining Go's
    /// predefined slice types on first use.
    fn interface_type(&mut self, name: &str) -> Result<TypeId, Error> {
        if let Some(id) = self.registered(name) {
            return Ok(id);
        }
        let Some(&(_, elem)) = BASIC_SLICES.iter().find(|(n, _)| *n == name) else {
            return Err(Error::data(format!("type not registered for interface: {name}")));
        };
        // Sent ahead of the value message being built.
        let id = self.define_slice(name, elem);
        self.register(name, id);
        Ok(id)
    }
}
