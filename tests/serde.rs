//! serde support. tests/go has the same orders as `sample_orders` and checks
//! them against what each side decodes.

mod common;

use std::collections::BTreeMap;

use common::{fixture, go};
use grob::{Decoder, Encoder, Value};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(rename_all = "PascalCase")]
struct Point {
    x: i64,
    y: i64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(rename_all = "PascalCase")]
struct Item {
    #[serde(rename = "SKU")]
    sku: String,
    count: u16,
    price: f32,
}

/// A Go `any` holding one of the types the Go side registered.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
enum Extra {
    #[serde(rename = "main.Point")]
    Point(Point),
    #[serde(rename = "int")]
    Int(i64),
}

/// Go string constants.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
enum Status {
    #[serde(rename = "shipped")]
    Shipped,
    #[serde(rename = "pending")]
    Pending,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
#[serde(rename_all = "PascalCase")]
struct Order {
    #[serde(rename = "ID", with = "grob::binary_marshaler")]
    id: Uuid,
    customer: String,
    items: Vec<Item>,
    tags: Vec<String>,
    quantities: BTreeMap<String, u32>,
    total: f64,
    paid: bool,
    note: Option<String>,
    shipping: Option<Point>,
    position: [i64; 2],
    payload: Vec<u8>,
    extra: Option<Extra>,
    parent: Option<Box<Order>>,
    #[serde(with = "grob::gob_encoder")]
    version: String,
    status: Option<Status>,
}

/// The same orders as sampleOrders in tests/go/main.go.
fn sample_orders() -> Vec<Order> {
    let mut id = [0u8; 16];
    id[0] = 1;
    id[15] = 9;
    vec![
        Order {
            id: Uuid::from_bytes(id),
            customer: "ann".into(),
            items: vec![
                Item { sku: "a-1".into(), count: 2, price: 9.5 },
                Item { sku: "b-2".into(), ..Item::default() },
            ],
            tags: vec!["gift".into(), String::new()],
            quantities: BTreeMap::from([("a-1".into(), 2), ("b-2".into(), 0)]),
            total: 19.0,
            paid: true,
            note: Some("leave at door".into()),
            shipping: Some(Point { x: 3, y: 4 }),
            position: [-1, 0],
            payload: vec![0, 1],
            extra: Some(Extra::Point(Point { x: 5, y: 6 })),
            parent: Some(Box::new(Order {
                customer: "ann's mom".into(),
                items: vec![Item { count: 1, ..Item::default() }],
                version: "1.0".into(),
                ..Order::default()
            })),
            version: "2.3".into(),
            status: Some(Status::Shipped),
        },
        Order { customer: "bob".into(), extra: Some(Extra::Int(42)), ..Order::default() },
    ]
}

#[test]
fn deserializes_orders_go_wrote() {
    let stream = fixture("orders.gob");
    let mut reader = stream.as_slice();
    let mut decoder = Decoder::new();
    for expected in sample_orders() {
        assert_eq!(decoder.deserialize_from::<Order>(&mut reader).unwrap(), Some(expected));
    }
    assert_eq!(decoder.deserialize_from::<Order>(&mut reader).unwrap(), None);

    // The same through values.
    let orders: Vec<Order> =
        Decoder::new().decode_all(&stream).unwrap().into_iter().map(|v| grob::from_value(v).unwrap()).collect();
    assert_eq!(orders, sample_orders());
}

#[test]
fn go_deserializes_our_orders() {
    let mut encoder = Encoder::new();
    for order in sample_orders() {
        encoder.serialize(&order).unwrap();
    }
    if let Some(output) = go("verify-orders", &encoder.finish()) {
        assert_eq!(output.trim(), "ok");
    }
}

#[test]
fn orders_round_trip() {
    for order in sample_orders().into_iter().chain([Order::default()]) {
        let bytes = grob::to_vec(&order).unwrap();
        assert_eq!(grob::from_slice::<Order>(&bytes).unwrap(), order);
    }
}

fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(value: &T) {
    let bytes = grob::to_vec(value).unwrap();
    assert_eq!(&grob::from_slice::<T>(&bytes).unwrap(), value);
}

#[test]
fn top_level_values_of_every_kind_round_trip() {
    round_trip(&42i32);
    round_trip(&0u64);
    round_trip(&-1.5f64);
    round_trip(&true);
    round_trip(&"text".to_owned());
    round_trip(&'x');
    round_trip(&vec![1u8, 2, 3]);
    round_trip(&[Point { x: 1, y: 2 }, Point::default()]);
    round_trip(&BTreeMap::from([(1i64, vec!["a".to_owned()]), (2, vec![])]));
    round_trip(&vec![vec![1i64], vec![], vec![2, 3]]);
    round_trip(&Some(Point { x: 0, y: 7 }));
    round_trip(&Extra::Int(0));
    round_trip(&vec![Status::Pending, Status::Shipped]);
    round_trip(&());
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Default)]
#[serde(rename_all = "PascalCase")]
struct Node {
    value: i64,
    next: Option<Box<Node>>,
}

fn list(len: i64) -> Node {
    (1..len).rev().fold(Node { value: len, next: None }, |next, value| Node { value, next: Some(Box::new(next)) })
}

#[test]
fn recursive_types_share_one_definition() {
    let long = list(100);
    let bytes = grob::to_vec(&long).unwrap();
    assert_eq!(grob::from_slice::<Node>(&bytes).unwrap(), long);
    // Each extra node adds its value, a few bytes, but no definition.
    let short = grob::to_vec(&list(2)).unwrap();
    assert!(bytes.len() - short.len() < 98 * 6, "{} vs {} bytes", bytes.len(), short.len());
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Default)]
#[serde(rename_all = "PascalCase")]
struct Wide {
    a: i64,
    b: String,
    inner: Option<Node>,
}

#[test]
fn streams_reuse_definitions_until_a_value_needs_more_fields() {
    let mut encoder = Encoder::new();
    let mut decoder = Decoder::new();
    let mut send = |value: &Wide| {
        encoder.serialize(value).unwrap();
        let bytes = encoder.take();
        assert_eq!(&decoder.deserialize::<Wide>(&bytes).unwrap(), value);
        bytes.len()
    };

    let first = send(&Wide { a: 1, b: "x".into(), inner: None });
    // The same fields: just the value.
    let same = send(&Wide { a: 2, b: "y".into(), inner: None });
    assert!(same < first / 2, "{same} vs {first} bytes");
    // Fewer fields: the first definition covers them.
    let fewer = send(&Wide { a: 3, ..Wide::default() });
    assert!(fewer < same);
    // A new field: a wider definition comes first.
    let wider = send(&Wide { a: 4, b: "z".into(), inner: Some(Node { value: 5, next: None }) });
    assert!(wider > first);
    // The old fields again: the wider definition covers them.
    assert_eq!(send(&Wide { a: 6, b: "w".into(), inner: None }), same);
}

#[test]
fn missing_fields_deserialize_as_zero_values() {
    #[derive(Deserialize, Debug, PartialEq, Default)]
    struct Everything {
        int: i32,
        uint: u8,
        float: f32,
        flag: bool,
        text: String,
        bytes: Vec<u8>,
        list: Vec<String>,
        map: BTreeMap<String, i64>,
        array: [u16; 3],
        nested: Point,
        tuple: (i64, String),
        maybe: Option<Point>,
        unit: (),
    }
    assert_eq!(grob::from_value::<Everything>(Value::Struct(vec![])).unwrap(), Everything::default());
    assert_eq!(grob::from_slice::<Everything>(&grob::to_vec(&()).unwrap()).unwrap(), Everything::default());
}

#[test]
fn enums_come_from_interface_names_strings_and_indexes() {
    assert_eq!(grob::from_value::<Status>("pending".into()).unwrap(), Status::Pending);
    // Go enums are usually iota constants: the index selects the variant.
    assert_eq!(grob::from_value::<Status>(Value::Int(1)).unwrap(), Status::Pending);
    let point =
        Value::Interface { name: "main.Point".into(), value: Box::new(Value::Struct(vec![("X".into(), 1i64.into())])) };
    assert_eq!(grob::from_value::<Extra>(point).unwrap(), Extra::Point(Point { x: 1, y: 0 }));
    let other = Value::Interface { name: "main.Other".into(), value: Box::new(1i64.into()) };
    assert!(grob::from_value::<Extra>(other).unwrap_err().is_data());
}

#[test]
fn to_value_leaves_zero_fields_out() {
    let value = grob::to_value(&Point { x: 0, y: 3 }).unwrap();
    assert_eq!(value, Value::Struct(vec![("Y".into(), 3i64.into())]));
}

#[test]
fn values_without_a_gob_equivalent_are_data_errors() {
    #[derive(Serialize)]
    enum Tuple {
        Pair(i64, i64),
    }

    let unsupported = [
        // Empty collections that must be sent: nothing tells their type.
        grob::to_vec(&Vec::<i64>::new()),
        grob::to_vec(&vec![Vec::<i64>::new()]),
        // Gob collections hold elements of one type.
        grob::to_vec(&(1i64, "a")),
        // Gob cannot send nil elements or nil at the top level.
        grob::to_vec(&vec![Some(1i64), None]),
        grob::to_vec(&None::<i64>),
        grob::to_vec(&Tuple::Pair(1, 2)),
    ];
    for (i, result) in unsupported.into_iter().enumerate() {
        assert!(result.unwrap_err().is_data(), "case {i}");
    }
}

#[test]
fn a_failed_value_leaves_the_stream_usable() {
    let mut encoder = Encoder::new();
    encoder.serialize(&vec![Vec::<i64>::new()]).unwrap_err();
    assert!(encoder.take().is_empty());
    encoder.serialize(&Point { x: 1, y: 2 }).unwrap();
    assert_eq!(grob::from_slice::<Point>(&encoder.finish()).unwrap(), Point { x: 1, y: 2 });
}

#[test]
fn deserialization_errors_tell_where_in_the_value_and_stream() {
    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "PascalCase")]
    #[expect(dead_code, reason = "only deserialized")]
    struct Wrong {
        items: Vec<WrongItem>,
    }
    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "PascalCase")]
    #[expect(dead_code, reason = "only deserialized")]
    struct WrongItem {
        count: String,
    }

    let stream = fixture("orders.gob");
    let error = grob::from_slice::<Wrong>(&stream).unwrap_err();
    assert!(error.is_data());
    assert_eq!(error.path().as_deref(), Some("Items[0].Count"));
    assert!(error.offset().is_some_and(|offset| offset < stream.len() as u64));
    // Values give the same path.
    let value = Decoder::new().decode(&stream).unwrap();
    assert_eq!(grob::from_value::<Wrong>(value).unwrap_err().path().as_deref(), Some("Items[0].Count"));
}

#[test]
fn serialization_errors_tell_where_in_the_value() {
    #[derive(Serialize)]
    enum Mixed {
        #[serde(rename = "int")]
        Int(i64),
        Plain,
    }
    #[derive(Serialize)]
    struct Holder {
        values: Vec<Mixed>,
    }
    let error = grob::to_vec(&Holder { values: vec![Mixed::Int(1), Mixed::Plain] }).unwrap_err();
    assert!(error.is_data());
    assert_eq!(error.path().as_deref(), Some("values[1]"));
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Borrowed<'a> {
    text: &'a str,
    data: &'a [u8],
    #[serde(borrow)]
    cow: std::borrow::Cow<'a, str>,
    #[serde(borrow)]
    names: Vec<&'a str>,
}

fn points_into(part: &[u8], whole: &[u8]) -> bool {
    whole.as_ptr_range().contains(&part.as_ptr())
}

#[test]
fn strings_and_bytes_borrow_from_the_input() {
    #[derive(Deserialize)]
    struct Holder<'a> {
        #[serde(rename = "B")]
        b: &'a str,
    }

    let original = Borrowed { text: "hello", data: &[1, 2, 3], cow: "cow".into(), names: vec!["a", "bc"] };
    let bytes = grob::to_vec(&original).unwrap();
    let read: Borrowed = grob::from_slice(&bytes).unwrap();
    assert_eq!(read, original);
    assert!(matches!(read.cow, std::borrow::Cow::Borrowed(_)));
    for part in [read.text.as_bytes(), read.data, read.cow.as_bytes(), read.names[1].as_bytes()] {
        assert!(points_into(part, &bytes));
    }

    // Also when Go sends type definitions in the middle of a value.
    let stream = fixture("interfaces.gob");
    let holder: Holder = grob::from_slice(&stream).unwrap();
    assert_eq!(holder.b, "z");
    assert!(points_into(holder.b.as_bytes(), &stream));
}

#[test]
fn strings_with_invalid_utf8_cannot_be_borrowed() {
    #[derive(Deserialize, Debug)]
    #[expect(dead_code, reason = "only deserialized")]
    struct Text<'a> {
        #[serde(rename = "Text")]
        text: &'a str,
    }
    #[derive(Deserialize, Debug)]
    struct Owned {
        #[serde(rename = "Text")]
        text: String,
    }

    let mut encoder = Encoder::new();
    let ty = encoder.define_struct("T", &[("Text", grob::TypeId::STRING)]);
    encoder.encode(ty, &Value::Struct(vec![("Text".into(), Value::Bytes(vec![0xff, b'a']))])).unwrap();
    let bytes = encoder.finish();

    assert!(grob::from_slice::<Text>(&bytes).unwrap_err().is_data());
    // An owned string gets the text with the invalid byte replaced.
    assert_eq!(grob::from_slice::<Owned>(&bytes).unwrap().text, "\u{fffd}a");
}

#[test]
fn values_serialize_as_declared_types() {
    #[derive(Serialize)]
    struct Local {
        #[serde(rename = "ID")]
        id: i32,
        #[serde(rename = "Size")]
        size: u64,
        #[serde(rename = "Tags")]
        tags: Vec<String>,
        #[serde(rename = "Pair")]
        pair: Vec<i16>,
        #[serde(rename = "Raw")]
        raw: String,
        #[serde(rename = "Unused")]
        unused: Option<bool>,
    }

    let mut encoder = Encoder::new();
    let tags = encoder.define_slice("", grob::TypeId::STRING);
    let pair = encoder.define_array("", grob::TypeId::INT, 2);
    let remote = encoder.define_struct(
        "Remote",
        &[
            ("ID", grob::TypeId::UINT),
            ("Size", grob::TypeId::INT),
            ("Tags", tags),
            ("Pair", pair),
            ("Raw", grob::TypeId::BYTES),
            ("Other", grob::TypeId::FLOAT),
        ],
    );
    let local = Local { id: 7, size: 9, tags: vec![], pair: vec![-1, 2], raw: "hi".into(), unused: None };
    encoder.serialize_as(remote, &local).unwrap();
    // Empty collections need no inference, even on their own.
    encoder.serialize_as(tags, &Vec::<String>::new()).unwrap();
    encoder.serialize_as(grob::TypeId::UINT, &5i8).unwrap();

    let values = Decoder::new().decode_all(&encoder.finish()).unwrap();
    let expected = Value::Struct(vec![
        ("ID".into(), Value::Uint(7)),
        ("Size".into(), Value::Int(9)),
        ("Pair".into(), Value::List(vec![Value::Int(-1), Value::Int(2)])),
        ("Raw".into(), Value::Bytes(b"hi".to_vec())),
    ]);
    assert_eq!(values, [expected, Value::List(vec![]), Value::Uint(5)]);
}

#[test]
fn values_that_do_not_fit_declared_types_are_errors() {
    #[derive(Serialize)]
    struct Local {
        #[serde(rename = "Count")]
        count: i64,
        #[serde(rename = "Extra")]
        extra: String,
    }

    let mut encoder = Encoder::new();
    let remote = encoder.define_struct("Remote", &[("Count", grob::TypeId::UINT)]);
    let list = encoder.define_slice("", remote);

    let error = encoder
        .serialize_as(list, &[Local { count: 1, extra: String::new() }, Local { count: -1, extra: String::new() }]);
    let error = error.unwrap_err();
    assert!(error.is_data());
    assert_eq!(error.path().as_deref(), Some("[1].Count"));
    assert!(error.to_string().contains("integer -1 as uint"), "{error}");

    let error = encoder.serialize_as(remote, &Local { count: 1, extra: "x".into() }).unwrap_err();
    assert_eq!(error.path().as_deref(), Some("Extra"));
    assert!(error.to_string().contains("no field Extra"), "{error}");

    let error = encoder.serialize_as(remote, &"text").unwrap_err();
    assert!(error.to_string().contains("a string as Remote"), "{error}");

    assert!(encoder.serialize_as(encoder.next_type_id(), &1).unwrap_err().is_data());
    assert!(encoder.serialize_as(grob::TypeId::INTERFACE, &1).unwrap_err().is_data());

    // Nothing reached the stream but the value that fits.
    encoder.serialize_as(remote, &Local { count: 3, extra: String::new() }).unwrap();
    let values = Decoder::new().decode_all(&encoder.finish()).unwrap();
    assert_eq!(values, [Value::Struct(vec![("Count".into(), Value::Uint(3))])]);
}
