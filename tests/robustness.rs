//! Decoding untrusted input: malformed streams are errors, never panics,
//! nesting is bounded, and both deserializers agree.

mod common;

use common::{FIXTURES, decode_fixture, fixture, iface, structure};
use grob::{Decoder, Encoder, TypeId, Value};
use serde::Deserialize;
use serde::de::{self, IgnoredAny, MapAccess, SeqAccess};

/// Whatever `deserialize_any` reports, to compare deserializers with.
#[derive(Debug, PartialEq)]
enum Any {
    None,
    Unit,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Some(Box<Any>),
    Seq(Vec<Any>),
    Map(Vec<(Any, Any)>),
}

struct AnyVisitor;

impl<'de> de::Visitor<'de> for AnyVisitor {
    type Value = Any;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("anything")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Any, E> {
        Ok(Any::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Any, E> {
        Ok(Any::Int(v))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Any, E> {
        Ok(Any::Uint(v))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Any, E> {
        Ok(Any::Float(v))
    }

    fn visit_str<E>(self, v: &str) -> Result<Any, E> {
        Ok(Any::Str(v.to_owned()))
    }

    fn visit_bytes<E>(self, v: &[u8]) -> Result<Any, E> {
        Ok(Any::Bytes(v.to_vec()))
    }

    fn visit_none<E>(self) -> Result<Any, E> {
        Ok(Any::None)
    }

    fn visit_some<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Any, D::Error> {
        Ok(Any::Some(Box::new(Any::deserialize(deserializer)?)))
    }

    fn visit_unit<E>(self) -> Result<Any, E> {
        Ok(Any::Unit)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Any, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Any::Seq(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Any, A::Error> {
        let mut entries = Vec::new();
        while let Some(entry) = map.next_entry()? {
            entries.push(entry);
        }
        Ok(Any::Map(entries))
    }
}

impl<'de> Deserialize<'de> for Any {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(AnyVisitor)
    }
}

/// Every truncation of every fixture, and every fixture with each byte
/// flipped a few ways.
fn damaged_fixtures() -> impl Iterator<Item = Vec<u8>> {
    FIXTURES.iter().flat_map(|name| {
        let bytes = fixture(name);
        (0..bytes.len()).flat_map(move |i| {
            let truncated = bytes[..i].to_vec();
            let flipped = [0x01, 0x80, 0xff].map(|flip| {
                let mut corrupted = bytes.clone();
                corrupted[i] ^= flip;
                corrupted
            });
            std::iter::once(truncated).chain(flipped)
        })
    })
}

#[derive(Deserialize)]
#[expect(dead_code, reason = "only deserialized")]
struct Order {
    #[serde(rename = "Customer")]
    customer: String,
    #[serde(rename = "Items")]
    items: Vec<(String, u16, f32)>,
    #[serde(rename = "Extra")]
    extra: Option<Any>,
}

#[test]
fn damaged_streams_are_errors_not_panics() {
    for stream in damaged_fixtures() {
        // Results don't matter, only that every call returns.
        let _values = Decoder::new().decode_all(&stream);
        let mut decoder = Decoder::new();
        let mut reader = stream.as_slice();
        while let Ok(Some(_)) = decoder.decode_from(&mut reader) {}
        let mut decoder = Decoder::new();
        let mut reader = stream.as_slice();
        while let Ok(Some(_)) = decoder.deserialize_from::<Any>(&mut reader) {}
        let _orders = grob::from_slice::<Vec<Order>>(&stream);
        let _order = grob::from_slice::<Order>(&stream);
    }
}

#[test]
fn both_deserializers_agree_on_every_fixture() {
    for name in FIXTURES {
        let bytes = fixture(name);
        let mut decoder = Decoder::new();
        let mut reader = bytes.as_slice();
        for value in decode_fixture(name) {
            let direct: Any = decoder.deserialize_from(&mut reader).unwrap().unwrap();
            assert_eq!(direct, grob::from_value::<Any>(value).unwrap(), "{name}");
        }
        assert!(decoder.deserialize_from::<Any>(&mut reader).unwrap().is_none());
    }
}

#[test]
fn values_a_visitor_does_not_read_are_skipped() {
    #[derive(Deserialize, Debug, PartialEq)]
    struct Customer {
        #[serde(rename = "Customer")]
        customer: String,
    }
    let bytes = fixture("orders.gob");
    let mut decoder = Decoder::new();
    let mut reader = bytes.as_slice();
    for expected in ["ann", "bob"] {
        let order: Customer = decoder.deserialize_from(&mut reader).unwrap().unwrap();
        assert_eq!(order.customer, expected);
    }
    assert!(decoder.deserialize_from::<Customer>(&mut reader).unwrap().is_none());

    let bytes = fixture("interfaces.gob");
    let mut decoder = Decoder::new();
    let mut reader = bytes.as_slice();
    let mut count = 0;
    while decoder.deserialize_from::<IgnoredAny>(&mut reader).unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, 4);
}

/// Runs `f` on a thread with a 2 MiB stack, the default for spawned threads.
fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new().stack_size(2 << 20).spawn(f).unwrap().join().unwrap()
}

#[test]
fn the_default_depth_limit_fits_a_small_stack() {
    let mut encoder = Encoder::new();
    let deep = encoder.define_struct("Deep", &[("Inner", TypeId::INTERFACE)]);
    encoder.register("main.Deep", deep);
    // Each level is a struct field plus an interface: two levels of nesting.
    let mut value = structure(vec![]);
    for _ in 0..255 {
        value = structure(vec![("Inner", iface("main.Deep", value))]);
    }
    encoder.encode(deep, &value).unwrap();
    let stream = encoder.finish();

    let stream_copy = stream.clone();
    assert_eq!(on_small_stack(move || Decoder::new().decode(&stream_copy).unwrap()), value);
    on_small_stack(move || grob::from_slice::<Any>(&stream).unwrap());
}

#[test]
fn nesting_beyond_the_limit_is_an_error() {
    let list = fixture("list.gob"); // 300 levels
    assert!(Decoder::new().with_max_depth(100).decode(&list).unwrap_err().is_limit());
    assert!(Decoder::new().with_max_depth(100).deserialize::<Any>(&list).unwrap_err().is_limit());

    let mut encoder = Encoder::new();
    let node = encoder.next_type_id();
    encoder.define_struct("Node", &[("Next", node)]);
    let mut value = Value::Struct(vec![]);
    for _ in 0..600 {
        value = structure(vec![("Next", value)]);
    }
    encoder.encode(node, &value).unwrap();
    let stream = encoder.finish();
    let stream_copy = stream.clone();
    assert!(on_small_stack(move || Decoder::new().decode(&stream_copy).unwrap_err()).is_limit());
    assert!(on_small_stack(move || grob::from_slice::<IgnoredAny>(&stream).unwrap_err()).is_limit());
    // Dropping a 600-level value recurses as well; keep it off small stacks.
    drop(value);
}
