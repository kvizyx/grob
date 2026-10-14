# grob

Go's [`encoding/gob`](https://pkg.go.dev/encoding/gob) format for Rust:
exchange data with Go programs that speak gob.

With serde (the default `serde` feature), Rust types map to Go types:

```rust
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")] // Go's exported field names
struct Order {
    #[serde(rename = "ID", with = "grob::binary_marshaler")] // uuid.UUID
    id: uuid::Uuid,
    items: Vec<Item>,     // []Item
    note: Option<String>, // *string
    extra: Option<Extra>, // any
}

#[derive(Serialize, Deserialize)]
enum Extra {
    // Interface values, selected by the name Go registered their type under.
    #[serde(rename = "main.Point")]
    Point(Point),
    #[serde(rename = "int")]
    Int(i64),
}

let bytes = grob::to_vec(&order)?;
let order: Order = grob::from_slice(&bytes)?;
```

Gob type definitions are worked out from the values. Like Go, zero fields
are left out, and come back as zero values when deserializing, so empty
collections in structs need no type information. Integers must match the Go
side's signedness (`int` needs a signed Rust integer, `uint` an unsigned
one), and `#[serde(flatten)]` and tuple enum variants have no gob equivalent.

When the Go side expects particular types, declare them and serialize
against them; nothing is inferred then, and integers convert when they fit:

```rust
let mut encoder = grob::Encoder::new();
let tags = encoder.define_slice("", TypeId::STRING);
let request = encoder.define_struct("Request", &[("ID", TypeId::UINT), ("Tags", tags)]);
encoder.serialize_as(request, &Request { id: 7, tags: vec![] })?; // id: i32
```

Strings and byte slices deserialize without copying when the target
borrows them: `&str`, `&[u8]`, or `Cow<str>` with `#[serde(borrow)]`.

To send each value as a stream of its own, as Go code does with a
`gob.Encoder` per message, call `Encoder::reset` between values. The
encoder remembers what each Rust type needed on a new stream, so it does
not work out the types again.

Errors say where they happened: `error.path()` gives the place in the value,
like `Items[2].Name`, and `error.offset()` the byte of the stream.

Without serde, values are dynamic `Value`s and types are declared:

```rust
use grob::{Decoder, Encoder, TypeId, Value};

let mut encoder = Encoder::new();
let request = encoder.define_struct("Request", &[("ID", TypeId::STRING), ("Count", TypeId::INT)]);
encoder.encode(request, &Value::Struct(vec![("ID".into(), "abc".into()), ("Count".into(), 3i64.into())]))?;
let bytes = encoder.finish();

let value = Decoder::new().decode(&bytes)?;
assert_eq!(value.get("ID").and_then(Value::as_str), Some("abc"));
```

Every gob type is supported: bools, integers, floats, complex numbers,
strings, byte slices, arrays, slices, maps, structs including recursive ones,
interface values, and types that encode themselves (`GobEncoder`,
`BinaryMarshaler`). So are streams of many values, and reading them from any
`std::io::Read`. Go's encoding/gob ignores `TextMarshaler`, so Go types that
implement it travel as plain structs.

## Untrusted input

Decoding is safe on input from untrusted peers: malformed data is an error,
never a panic; lengths read from the input are checked before anything is
allocated for them; and values nest at most 512 levels deep by default
(`Decoder::with_max_depth`), which keeps decoding within a 2 MiB thread stack.

## Tests

- `tests/value.rs` and `tests/serde.rs` cover the two APIs.
- `tests/interop.rs` and `tests/serde.rs` check compatibility with Go both
  ways: they decode fixtures written by Go's encoding/gob, and when `go` is
  installed, they have Go decode what this crate encodes. The fixtures come
  from `tests/go` (`cd tests/go && go run . gen ../fixtures`).
- `tests/robustness.rs` feeds truncated and corrupted streams to the decoders
  and checks the nesting limit.

Tests that need Go skip themselves when `go` is not installed, unless
`GROB_REQUIRE_GO` is set, as CI does. CI (`.github/workflows/ci.yml`) also
runs clippy, rustdoc and the Rust 1.88 build with and without serde, and
fuzzes each target for a minute on every push, ten minutes weekly.

## Fuzzing

`fuzz/` holds [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) targets,
which need a nightly toolchain:

- `decode` feeds arbitrary bytes to every decoding entry point, including
  readers that return data in small chunks;
- `differential` checks that decoding through `Value`s and deserializing
  straight from the stream agree on every input;
- `round_trip` serializes arbitrary Rust values as a stream and checks they
  come back the same both ways.

```sh
cargo install cargo-fuzz
mkdir -p fuzz/corpus/decode && cp tests/fixtures/*.gob fuzz/corpus/decode/
cargo +nightly fuzz run decode
```

## Benchmarks

`cargo bench` runs `benches/gob.rs`, and `cd tests/go && go test -bench .`
runs Go's encoding/gob on the same data: 1000 orders, structs with nested
slices, maps, strings, a UUID and an interface field, about 170 KB of gob.

| | grob | Go encoding/gob |
|---|---|---|
| encode, serde (`to_vec`) | 2.0 ms | 1.9 ms |
| encode, `Value` and declared types | 0.9 ms | |
| decode, serde (`from_slice`) | 2.2 ms | 2.4 ms |
| decode to `Value` | 2.4 ms | |
| stream of 1000 messages, serde encode | 1.1 ms | 1.7 ms |
| stream of 1000 messages, serde decode | 2.1 ms | 2.6 ms |
| 1000 streams of one message, serde encode (`reset`) | 1.4 ms | |
| 1000 streams of one message, serde encode (`to_vec`) | 16 ms | |

Encoding a value of a Rust type the stream has seen before writes it
straight away, and so does a stream after `reset`; a fresh `to_vec` first
works out the types. Decoding mostly
allocates the strings, vectors and maps of the result, so a faster global
allocator helps it most.

## Minimum Rust version

Rust 1.88.

## License

This project is licensed under the [MIT](LICENSE) license.
