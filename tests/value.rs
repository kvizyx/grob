//! Encoding and decoding [`Value`]s against declared types.

mod common;

use common::{iface, point};
use grob::{Decoder, Encoder, Type, TypeId, Value};

fn decode_all(encoder: Encoder) -> Vec<Value> {
    Decoder::new().decode_all(&encoder.finish()).unwrap()
}

#[test]
fn type_definitions_match_the_bytes_go_sends() {
    // The first message of Go's encoding of
    // struct Holder { A int; I, J any; B string }.
    let mut expected = vec![0x2b, 0x7f, 0x03, 0x01, 0x01, 0x06];
    expected.extend(b"Holder");
    expected.extend([0x01, 0xff, 0x80, 0x00, 0x01, 0x04]);
    for (name, id) in [(b'A', 0x04), (b'I', 0x10), (b'J', 0x10), (b'B', 0x0c)] {
        expected.extend([0x01, 0x01, name, 0x01, id, 0x00]);
    }
    expected.extend([0x00, 0x00]);

    let mut encoder = Encoder::new();
    let fields = [("A", TypeId::INT), ("I", TypeId::INTERFACE), ("J", TypeId::INTERFACE), ("B", TypeId::STRING)];
    encoder.define_struct("Holder", &fields);
    assert_eq!(encoder.finish(), expected);
}

#[test]
fn zero_fields_are_left_out_and_fields_may_come_in_any_order() {
    let mut encoder = Encoder::new();
    let ints = encoder.define_slice("[]int", TypeId::INT);
    let ty = encoder.define_struct(
        "T",
        &[("A", TypeId::INT), ("B", TypeId::FLOAT), ("C", ints), ("D", TypeId::STRING), ("E", TypeId::INTERFACE)],
    );
    let value = Value::Struct(vec![
        ("E".into(), Value::Nil),
        ("D".into(), "".into()),
        ("C".into(), Value::List(vec![])),
        ("B".into(), (-0.0f64).into()),
        ("A".into(), 0i64.into()),
    ]);
    encoder.encode(ty, &value).unwrap();
    assert_eq!(decode_all(encoder), [Value::Struct(vec![])]);
}

#[test]
fn every_kind_of_value_round_trips() {
    let mut encoder = Encoder::new();
    let point_type = encoder.define_struct("Point", &[("X", TypeId::INT), ("Y", TypeId::INT)]);
    encoder.register("main.Point", point_type);
    let points = encoder.define_array("[2]Point", point_type, 2);
    let map = encoder.define_map("map[string]any", TypeId::STRING, TypeId::INTERFACE);
    let uuid = encoder.define(Type::BinaryMarshaler { name: "UUID".into() });
    let node = encoder.next_type_id();
    encoder.define_struct("Node", &[("Value", TypeId::INT), ("Next", node)]);
    let ty = encoder.define_struct(
        "All",
        &[
            ("U", TypeId::UINT),
            ("C", TypeId::COMPLEX),
            ("B", TypeId::BYTES),
            ("A", points),
            ("M", map),
            ("ID", uuid),
            ("L", node),
        ],
    );

    let list = Value::Struct(vec![
        ("Value".into(), 1i64.into()),
        ("Next".into(), Value::Struct(vec![("Value".into(), 2i64.into())])),
    ]);
    let value = Value::Struct(vec![
        ("U".into(), 300u32.into()),
        ("C".into(), Value::Complex(1.5, -2.0)),
        ("B".into(), vec![0u8, 1].into()),
        ("A".into(), Value::List(vec![point(1, 2), point(0, 0)])),
        (
            "M".into(),
            Value::Map(vec![
                ("point".into(), iface("main.Point", point(3, 4))),
                ("ints".into(), iface("[]int", Value::List(vec![1i64.into(), 0i64.into()]))),
                ("str".into(), iface("string", "".into())),
                ("nil".into(), Value::Nil),
            ]),
        ),
        ("ID".into(), vec![7u8; 16].into()),
        ("L".into(), list),
    ]);
    encoder.encode(ty, &value).unwrap();
    encoder.encode(TypeId::INT, &(-5i64).into()).unwrap();
    encoder.encode(point_type, &point(0, 0)).unwrap();

    // The second array element's zero fields are left out on the wire.
    let mut expected = value;
    if let Value::Struct(fields) = &mut expected
        && let Value::List(points) = &mut fields[3].1
    {
        points[1] = Value::Struct(vec![]);
    }
    assert_eq!(decode_all(encoder), [expected, Value::Int(-5), Value::Struct(vec![])]);
}

#[test]
fn values_that_do_not_fit_their_type_are_data_errors_and_write_nothing() {
    let mut other = Encoder::new();
    let undefined = (0..5).map(|_| other.define_slice("", TypeId::INT)).last().unwrap();

    let mut encoder = Encoder::new();
    let pair = encoder.define_array("[2]int", TypeId::INT, 2);
    let ty = encoder.define_struct("T", &[("A", TypeId::INT)]);
    let anys = encoder.define_slice("[]any", TypeId::INTERFACE);
    let before = encoder.take();
    let unregistered = iface("main.X", 1i64.into());
    let failures = [
        encoder.encode(ty, &Value::Struct(vec![("A".into(), "x".into())])),
        encoder.encode(ty, &Value::Struct(vec![("A".into(), 0u8.into()), ("A".into(), 2i64.into())])),
        encoder.encode(ty, &Value::Struct(vec![("Z".into(), 1i64.into())])),
        encoder.encode(pair, &Value::List(vec![1i64.into()])),
        encoder.encode(TypeId::INT, &Value::Nil),
        encoder.encode(TypeId::UINT, &(-1i64).into()),
        encoder.encode(undefined, &1i64.into()),
        encoder.encode(anys, &Value::List(vec![unregistered])),
    ];
    for (i, result) in failures.into_iter().enumerate() {
        assert!(result.unwrap_err().is_data(), "case {i}");
    }
    assert_ne!(before.len(), 0);
    assert!(encoder.take().is_empty(), "failed values leave the stream untouched");
}

#[test]
fn malformed_input_is_an_error_of_the_right_kind() {
    let decode = |bytes: &[u8]| Decoder::new().decode(bytes).unwrap_err();
    assert!(decode(&[]).is_eof());
    assert!(decode(&[0x05, 0x01]).is_eof(), "message longer than the input");
    assert!(decode(&[0xf0]).is_syntax(), "invalid length prefix");
    assert!(decode(&[0x00]).is_syntax(), "empty message");
    assert!(decode(&[0x02, 0x03, 0x00]).is_syntax(), "definition with a reserved id");
    assert!(decode(&[0x04, 0x10, 0x00, 0x7f, 0x01]).is_eof(), "interface name longer than the message");
    assert!(decode(&[0x03, 0xff, 0x82, 0x00]).is_syntax(), "value of an unknown type");
}

#[test]
fn reading_stops_cleanly_at_the_end_of_a_stream() {
    assert!(Decoder::new().decode_from(&[][..]).unwrap().is_none());
    assert!(Decoder::new().decode_from(&[0x05, 0x04, 0x00][..]).unwrap_err().is_eof());
}

#[test]
fn decodes_singletons_go_sends() {
    // complex(1.5, -2), as Go encodes it.
    let value = grob::decode(&[0x07, 0x0e, 0x00, 0xfe, 0xf8, 0x3f, 0xff, 0xc0]).unwrap();
    assert_eq!(value, Value::Complex(1.5, -2.0));
}

#[test]
fn deep_nesting_is_a_limit_error() {
    let mut encoder = Encoder::new();
    let node = encoder.next_type_id();
    encoder.define_struct("Node", &[("Next", node)]);
    let mut value = Value::Struct(vec![]);
    for _ in 0..20 {
        value = Value::Struct(vec![("Next".into(), value)]);
    }
    encoder.encode(node, &value).unwrap();
    let stream = encoder.finish();
    assert!(Decoder::new().with_max_depth(10).decode(&stream).unwrap_err().is_limit());
    assert_eq!(Decoder::new().decode(&stream).unwrap(), value);
}

/// The public types can be shared with and moved to other threads, as async
/// runtimes need.
const _: () = {
    const fn shareable<T: Send + Sync>() {}
    shareable::<Encoder>();
    shareable::<Decoder>();
    shareable::<Value>();
    shareable::<grob::Error>();
};

#[test]
fn encoding_errors_tell_where_in_the_value() {
    let mut encoder = Encoder::new();
    let point_type = encoder.define_struct("Point", &[("X", TypeId::INT), ("Y", TypeId::INT)]);
    let points = encoder.define_slice("[]Point", point_type);
    let ty = encoder.define_struct("Shape", &[("Points", points)]);
    let value =
        Value::Struct(vec![("Points".into(), Value::List(vec![point(1, 2), point(3, 4), iface("x", 1i64.into())]))]);
    let error = encoder.encode(ty, &value).unwrap_err();
    assert_eq!(error.path().as_deref(), Some("Points[2]"));
    assert_eq!(error.offset(), None);
}

#[test]
fn decoding_errors_tell_where_in_the_stream() {
    let mut encoder = Encoder::new();
    encoder.encode(TypeId::STRING, &"first".into()).unwrap();
    let mut stream = encoder.finish();
    let first = stream.len();
    // A second message: a value of type 65, which was never defined. The
    // decoder notices after the type id (2 bytes) and the singleton delta.
    stream.extend([0x03, 0xff, 0x82, 0x00]);

    let mut decoder = Decoder::new();
    let mut reader = stream.as_slice();
    assert!(decoder.decode_from(&mut reader).unwrap().is_some());
    let error = decoder.decode_from(&mut reader).unwrap_err();
    assert!(error.is_syntax());
    assert_eq!(error.offset(), Some(first as u64 + 4));
    assert!(error.to_string().ends_with(&format!("(byte {})", first + 4)), "{error}");
}
