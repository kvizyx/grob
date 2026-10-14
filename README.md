# grob

[![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-orange?logo=rust)](Cargo.toml)

Go's [`encoding/gob`](https://pkg.go.dev/encoding/gob) format implementation in Rust.

- Serde support (the default `serde` feature): Rust types map to Go
  types, and gob type definitions are worked out from the values.
- Declared types: send exactly the types the Go side expects, or work
  with dynamic `Value`s without serde.
- Every gob type: bools, integers, floats, complex numbers, strings, byte
  slices, arrays, slices, maps, structs including recursive ones, interface
  values, and types that encode themselves (`GobEncoder`, `BinaryMarshaler`).
- Streams of many values, read from any `std::io::Read`, or a stream per
  message.
- Zero-copy: strings and byte slices can borrow from the input.
- Safe on untrusted input, with errors that say where they happened.

## Usage

### Types mapping

> [!NOTE]
> Go's implementation ignores `TextMarshaler`, so Go types that implement it travel as plain structs.

| Rust                                        | Go                                                                    |
|---------------------------------------------|-----------------------------------------------------------------------|
| `bool`                                      | `bool`                                                                |
| `i8`, `i16`, `i32`, `i64`                   | `int`, `int8`, `int16`, `int32`, `int64`                              |
| `u8`, `u16`, `u32`, `u64`                   | `uint`, `uint8`, `uint16`, `uint32`, `uint64`                         |
| `f32`, `f64`                                | `float32`, `float64`                                                  |
| `String`, `&str`, `char`                    | `string`                                                              |
| `Vec<u8>`                                   | `[]byte`                                                              |
| `Vec<T>` and other sequences                | `[]T`                                                                 |
| `[T; N]`, tuples                            | `[N]T`                                                                |
| `HashMap<K, V>`, `BTreeMap<K, V>`           | `map[K]V`                                                             |
| Structs                                     | Structs, matched by field name                                        |
| `Option<T>`                                 | `*T`                                                                  |
| Enum newtype and struct variants            | Interface values (`any`), the variant named as Go registered the type |
| Enum unit variants                          | `string`                                                              |
| `#[serde(with = "grob::binary_marshaler")]` | Types implementing `encoding.BinaryMarshaler`, like `uuid.UUID`       |
| `#[serde(with = "grob::gob_encoder")]`      | Types implementing `gob.GobEncoder`                                   |

Like Go, zero fields (0, false, empty strings and collections, `None`) are
left out of structs, and come back as zero values. When types are worked out
from values, integers must match the Go side's signedness: `int` needs a
signed Rust integer and `uint` an unsigned one. `#[serde(flatten)]` and tuple
enum variants have no gob equivalent.

### With Serde

```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "PascalCase")] // Go's exported field names
struct Order {
    #[serde(rename = "ID", with = "grob::binary_marshaler")]
    id: uuid::Uuid,       // uuid.UUID
    items: Vec<Item>,     // []Item
    note: Option<String>, // *string
    extra: Option<Extra>, // any
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "PascalCase")]
struct Item {
    name: String,
    count: u32,
}

/// Interface values, selected by the name Go registered their type under.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
enum Extra {
    #[serde(rename = "main.Item")]
    Item(Item),
    #[serde(rename = "int")]
    Int(i64),
}

let order = Order {
    id: uuid::Uuid::new_v4(),
    items: vec![Item { name: "tea".into(), count: 2 }],
    note: None,
    extra: Some(Extra::Int(7)),
};
let bytes = grob::to_vec(&order)?;
assert_eq!(grob::from_slice::<Order>(&bytes)?, order);
```

### Declared types

When the Go side expects particular types, declare them and serialize
against them. Nothing is worked out from the values then: empty collections
need no element type, integers convert when they fit, and type names are
the ones you give.

```rust
use grob::{Encoder, TypeId};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Request {
    #[serde(rename = "ID")]
    id: i32, // Sent as Go's uint
    tags: Vec<String>,
}

let mut encoder = Encoder::new();
let tags = encoder.define_slice("[]string", TypeId::STRING);
let request = encoder.define_struct("Request", &[("ID", TypeId::UINT), ("Tags", tags)]);
encoder.serialize_as(request, &Request { id: 7, tags: vec![] })?;
let bytes = encoder.finish();
```

### Borrowing from the input

Strings and byte slices deserialize without copying when the target borrows
them: `&str`, `&[u8]`, or `Cow<str>` with `#[serde(borrow)]`.

```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Message<'a> {
    text: &'a str,
    payload: &'a [u8],
}

let bytes = grob::to_vec(&Message { text: "hello", payload: b"\x01\x02" })?;
let message: Message = grob::from_slice(&bytes)?; // Points into `bytes`
assert_eq!(message.text, "hello");
```

### Streams

A stream carries type definitions once, ahead of the first value that needs
them. Encode values one after another, and decode them from a byte slice or
any `std::io::Read`, such as a connection:

```rust
use grob::{Decoder, Encoder};

let mut encoder = Encoder::new();
for word in ["one", "two", "three"] {
    encoder.serialize(word)?;
    // encoder.take() hands out what is ready to send
}
let stream = encoder.finish();

let mut decoder = Decoder::new();
let mut reader = stream.as_slice();
while let Some(word) = decoder.deserialize_from::<String>(&mut reader)? {
    println!("{word}");
}
```

When every message is a stream of its own, as when Go code creates a
`gob.Encoder` per message, call `Encoder::reset` between them. The encoder
remembers what each Rust type needed, so it does not work out the types
again:

```rust
let mut encoder = grob::Encoder::new();
for word in ["one", "two", "three"] {
    encoder.reset();
    encoder.serialize(word)?;
    let message = encoder.take(); // Decodable on its own
}
```

### Without Serde

Values are dynamic `Value`s, encoded against declared types; the decoder
understands any gob stream.

```rust
use grob::{Decoder, Encoder, TypeId, Value};

let mut encoder = Encoder::new();
let request = encoder.define_struct("Request", &[("ID", TypeId::STRING), ("Count", TypeId::INT)]);
encoder.encode(request, &Value::Struct(vec![("ID".into(), "abc".into()), ("Count".into(), 3i64.into())]))?;
let bytes = encoder.finish();

let value = Decoder::new().decode(&bytes)?;
assert_eq!(value.get("ID").and_then(Value::as_str), Some("abc"));
```

### Errors

Errors tell what went wrong, where in the value, and for decoding, at which
byte of the stream:

```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct Order<C> {
    items: Vec<Item<C>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct Item<C> {
    count: C,
}

// A count too big for the receiver's u8.
let bytes = grob::to_vec(&Order { items: vec![Item { count: 300u32 }] })?;
let error = grob::from_slice::<Order<u8>>(&bytes).unwrap_err();
assert_eq!(error.path().as_deref(), Some("Items[0].Count"));
println!("{error}"); // What went wrong, then "at Items[0].Count (byte N)"
```

## Untrusted input

Decoding is safe on input from untrusted peers (malformed data is an error, not panic). Lengths read from the input are checked before anything is
allocated for them, and values nest at most 512 levels deep by default
(`Decoder::with_max_depth`), which keeps decoding within a 2 MiB thread stack.

## License

This project is licensed under the [MIT](LICENSE) license.
