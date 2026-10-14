use std::backtrace::{Backtrace, BacktraceStatus};
use std::borrow::Cow;
use std::fmt::{self, Write as _};
use std::io;

/// An error from encoding or decoding gob data.
///
/// Errors fall into a few categories, which the `is_*` methods tell apart:
/// I/O failures, streams that end too early, malformed gob data, values that
/// do not fit a type, and input beyond the decoder's limits.
///
/// Errors also tell where they happened, when known: the [`path`](Self::path)
/// to the value within the encoded or decoded value, and for decoding, the
/// [`offset`](Self::offset) in the stream.
pub struct Error(Box<Inner>);

struct Inner {
    kind: Kind,
    message: Cow<'static, str>,
    /// Where in a value the error happened, innermost step first.
    path: Vec<Step>,
    offset: Option<u64>,
    backtrace: Backtrace,
}

/// One step of a path into a value.
enum Step {
    Field(Box<str>),
    Element(usize),
    Entry(usize),
}

enum Kind {
    Io(io::Error),
    Eof,
    Syntax,
    Data,
    Limit,
}

impl Error {
    #[cold]
    fn new(kind: Kind, message: impl Into<Cow<'static, str>>) -> Self {
        let message = message.into();
        Self(Box::new(Inner { kind, message, path: Vec::new(), offset: None, backtrace: Backtrace::capture() }))
    }

    /// The stream ended in the middle of a message or value.
    #[cold]
    pub(crate) fn eof() -> Self {
        Self::new(Kind::Eof, "unexpected end of stream")
    }

    /// Malformed gob data.
    #[cold]
    pub(crate) fn syntax(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(Kind::Syntax, message)
    }

    /// A value that does not fit its type, or cannot be encoded as gob.
    #[cold]
    pub(crate) fn data(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(Kind::Data, message)
    }

    /// Input beyond the decoder's limits.
    #[cold]
    pub(crate) fn limit(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(Kind::Limit, message)
    }

    /// Prefixes the message with what it is about.
    #[cfg_attr(not(feature = "serde"), expect(dead_code, reason = "only serde inference adds context"))]
    pub(crate) fn context(mut self, context: impl fmt::Display) -> Self {
        self.0.message = format!("{context}: {}", self.0.message).into();
        self
    }

    /// Records that the error happened in struct field `name`.
    #[cold]
    pub(crate) fn in_field(mut self, name: &str) -> Self {
        self.0.path.push(Step::Field(name.into()));
        self
    }

    /// Records that the error happened in element `index` of an array or
    /// slice.
    #[cold]
    pub(crate) fn in_element(mut self, index: usize) -> Self {
        self.0.path.push(Step::Element(index));
        self
    }

    /// Records that the error happened in entry `index` of a map, counting
    /// entries in stream order.
    #[cold]
    pub(crate) fn in_entry(mut self, index: usize) -> Self {
        self.0.path.push(Step::Entry(index));
        self
    }

    /// Records where in the stream the error happened, unless known already.
    #[cold]
    pub(crate) fn at_offset(mut self, offset: u64) -> Self {
        self.0.offset.get_or_insert(offset);
        self
    }

    /// Whether reading from or writing to an I/O object failed.
    #[must_use]
    pub fn is_io(&self) -> bool {
        matches!(self.0.kind, Kind::Io(_))
    }

    /// Whether the stream ended in the middle of a message or value.
    #[must_use]
    pub fn is_eof(&self) -> bool {
        matches!(self.0.kind, Kind::Eof)
    }

    /// Whether the input is not valid gob data.
    #[must_use]
    pub fn is_syntax(&self) -> bool {
        matches!(self.0.kind, Kind::Syntax)
    }

    /// Whether a value does not fit its type or cannot be encoded as gob.
    ///
    /// This covers valid gob data that does not deserialize into the
    /// requested Rust type, Rust values without a gob equivalent, and
    /// [`Value`](crate::Value)s that do not match the type they are encoded as.
    #[must_use]
    pub fn is_data(&self) -> bool {
        matches!(self.0.kind, Kind::Data)
    }

    /// Whether the input exceeds a decoder limit, such as the nesting depth.
    #[must_use]
    pub fn is_limit(&self) -> bool {
        matches!(self.0.kind, Kind::Limit)
    }

    /// Where in the value the error happened, such as `Items[3].SKU`.
    ///
    /// Struct fields are named, array and slice elements are `[index]`, and
    /// map entries are `{index}`, counting entries in stream order. `None`
    /// when the error is not about a nested value.
    #[must_use]
    pub fn path(&self) -> Option<String> {
        if self.0.path.is_empty() {
            return None;
        }
        let mut path = String::new();
        for step in self.0.path.iter().rev() {
            match step {
                Step::Field(name) if path.is_empty() => path.push_str(name),
                Step::Field(name) => {
                    path.push('.');
                    path.push_str(name);
                }
                // Writing to a `String` cannot fail.
                Step::Element(index) => _ = write!(path, "[{index}]"),
                Step::Entry(index) => _ = write!(path, "{{{index}}}"),
            }
        }
        Some(path)
    }

    /// The byte offset in the stream where decoding failed.
    ///
    /// Offsets count from the start of everything the decoder has read since
    /// it was created or reset. `None` for encoding errors.
    #[must_use]
    pub fn offset(&self) -> Option<u64> {
        self.0.offset
    }

    /// The backtrace captured when the error was created.
    ///
    /// It is only captured when enabled through `RUST_BACKTRACE` or
    /// `RUST_LIB_BACKTRACE`, see [`Backtrace`].
    #[must_use]
    pub fn backtrace(&self) -> &Backtrace {
        &self.0.backtrace
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.0.kind {
            Kind::Io(_) => "Io",
            Kind::Eof => "Eof",
            Kind::Syntax => "Syntax",
            Kind::Data => "Data",
            Kind::Limit => "Limit",
        };
        f.debug_struct("Error")
            .field("kind", &kind)
            .field("message", &self.0.message)
            .field("path", &self.path())
            .field("offset", &self.0.offset)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.message)?;
        if let Kind::Io(source) = &self.0.kind {
            write!(f, ": {source}")?;
        }
        if let Some(path) = self.path() {
            write!(f, " at {path}")?;
        }
        if let Some(offset) = self.0.offset {
            write!(f, " (byte {offset})")?;
        }
        if self.0.backtrace.status() == BacktraceStatus::Captured {
            write!(f, "\n\n{}", self.0.backtrace)?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.0.kind {
            Kind::Io(source) => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(source: io::Error) -> Self {
        if source.kind() == io::ErrorKind::UnexpectedEof {
            return Self::eof();
        }
        Self::new(Kind::Io(source), "I/O error")
    }
}
