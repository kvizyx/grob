//! Values against Go's encoding/gob: decoding streams Go wrote (the
//! fixtures), and Go decoding streams written here (skipped without Go).

mod common;

use common::{decode_fixture, fixture, go, iface, point, structure};
use grob::{Decoder, Encoder, Type, TypeId, Value};

fn string(value: &Value, field: &str) -> String {
    value.get(field).and_then(Value::as_str).unwrap_or_default().to_owned()
}

#[test]
fn decodes_go_primitives() {
    let [value] = decode_fixture("primitives.gob").try_into().unwrap();
    assert_eq!(value.get("Bool"), Some(&Value::Bool(true)));
    assert_eq!(value.get("Int"), Some(&Value::Int(1 << 40)));
    assert_eq!(value.get("Neg"), Some(&Value::Int(-129)));
    assert_eq!(value.get("Uint"), Some(&Value::Uint(255)));
    assert_eq!(value.get("Float"), Some(&Value::Float(-17.5)));
    assert_eq!(string(&value, "String"), "héllo");
    assert_eq!(value.get("Bytes"), Some(&Value::Bytes(vec![0, 1, 2, 0xff])));
    assert_eq!(value.get("Zero"), None, "zero fields are not sent");
}

#[test]
fn decodes_go_nested_types() {
    let [value] = decode_fixture("nested.gob").try_into().unwrap();
    let mut uuid = vec![0u8; 16];
    uuid[..2].copy_from_slice(&[0x12, 0x34]);
    uuid[15] = 0xff;
    assert_eq!(value.get("ID"), Some(&Value::Bytes(uuid)));
    assert_eq!(string(value.get("Inner").unwrap(), "Name"), "inner");
    assert_eq!(value.get("Tags"), Some(&Value::List(vec!["a".into(), "b".into()])));
    assert_eq!(value.get("Counts"), Some(&Value::Map(vec![("x".into(), Value::Int(1))])));
    assert_eq!(value.get("Fixed"), Some(&Value::List(vec![Value::Int(-1), Value::Int(0), Value::Int(1)])));
    assert_eq!(string(&value, "Ptr"), "pointed");
}

#[test]
fn decodes_go_top_level_non_structs() {
    assert_eq!(decode_fixture("singleton.gob"), [Value::List(vec![1i64.into(), (-2i64).into(), 3i64.into()])]);
}

#[test]
fn decodes_go_interfaces_including_definitions_sent_mid_value() {
    let expected = [
        structure(vec![
            ("A", 1i64.into()),
            ("I", iface("main.Point", point(3, 4))),
            ("J", iface("int", 7i64.into())),
            ("B", "z".into()),
        ]),
        structure(vec![("Inner", iface("main.Deep", structure(vec![("Inner", iface("main.Point", point(1, 2)))])))]),
        structure(vec![
            (
                "I",
                iface(
                    "[]interface {}",
                    Value::List(vec![
                        iface("main.Point", point(5, 6)),
                        iface("string", "s".into()),
                        Value::Nil,
                        iface("[]int", Value::List(vec![1i64.into(), 2i64.into()])),
                    ]),
                ),
            ),
            ("J", iface("complex128", Value::Complex(1.0, 2.0))),
        ]),
        structure(vec![("A", 2i64.into())]),
    ];
    assert_eq!(decode_fixture("interfaces.gob"), expected);

    // The same stream read one value at a time.
    let bytes = fixture("interfaces.gob");
    let mut reader = bytes.as_slice();
    let mut decoder = Decoder::new();
    for value in &expected {
        assert_eq!(decoder.decode_from(&mut reader).unwrap().as_ref(), Some(value));
    }
    assert!(decoder.decode_from(&mut reader).unwrap().is_none());
}

#[test]
fn decodes_a_stream_of_go_values() {
    assert_eq!(
        decode_fixture("stream.gob"),
        [
            point(1, 2),
            structure(vec![]),
            Value::Complex(1.5, -2.0),
            "str".into(),
            Value::Map(vec![("a".into(), point(1, 2))]),
            Value::List(vec![true.into(), false.into()]),
            Value::Uint(200),
            Value::Float(0.5),
            Value::List(vec![]),
        ]
    );
}

#[test]
fn decodes_go_recursive_types() {
    let [list] = decode_fixture("list.gob").try_into().unwrap();
    let values: Vec<i64> = std::iter::successors(Some(&list), |node| node.get("Next"))
        .map(|node| node.get("Value").and_then(Value::as_i64).unwrap())
        .collect();
    assert_eq!(values, (1..=300).collect::<Vec<_>>());
}

#[test]
fn decodes_go_external_encodings_and_zero_structs() {
    let mut uuid = vec![0u8; 16];
    uuid[0] = 1;
    uuid[15] = 2;
    assert_eq!(
        decode_fixture("external.gob"),
        [structure(vec![
            ("ID", uuid.into()),
            // Go ignores TextMarshaler: Color travels as a struct.
            ("Color", structure(vec![("R", 255u8.into()), ("G", 128u8.into())])),
            ("Version", b"1.2".to_vec().into()),
            ("Zero", structure(vec![])),
            ("Empty", Value::Map(vec![])),
        ])]
    );
}

/// The types of `Everything` in tests/go/main.go, plus a field Go lacks.
fn define_everything(encoder: &mut Encoder) -> TypeId {
    let point = encoder.define_struct("Point", &[("X", TypeId::INT), ("Y", TypeId::INT)]);
    encoder.register("main.Point", point);
    let ints = encoder.define_slice("[]int", TypeId::INT);
    let node = encoder.next_type_id();
    encoder.define_struct("Node", &[("Value", TypeId::INT), ("Next", node)]);
    let color = encoder.define_struct("Color", &[("R", TypeId::UINT), ("G", TypeId::UINT), ("B", TypeId::UINT)]);
    let fields = [
        ("Int", TypeId::INT),
        ("Uint", TypeId::UINT),
        ("Float", TypeId::FLOAT),
        ("Bool", TypeId::BOOL),
        ("String", TypeId::STRING),
        ("Bytes", TypeId::BYTES),
        ("Array", encoder.define_array("[2]Point", point, 2)),
        ("Slice", encoder.define_slice("[]Point", point)),
        ("Map", encoder.define_map("map[string][]int", TypeId::STRING, ints)),
        ("Nested", point),
        ("Iface", TypeId::INTERFACE),
        ("IfaceInt", TypeId::INTERFACE),
        ("IfaceSlice", TypeId::INTERFACE),
        ("IfaceNil", TypeId::INTERFACE),
        ("List", node),
        ("ID", encoder.define(Type::BinaryMarshaler { name: "UUID".into() })),
        ("Color", color),
        ("Version", encoder.define(Type::GobEncoder { name: "Version".into() })),
        ("Extra", TypeId::STRING),
    ];
    encoder.define_struct("Everything", &fields)
}

fn everything() -> Value {
    let list = structure(vec![("Value", 1i64.into()), ("Next", structure(vec![("Value", 2i64.into())]))]);
    let ints = Value::List(vec![1i64.into(), 0i64.into(), (-1i64).into()]);
    structure(vec![
        ("Extra", "dropped".into()),
        ("Int", (-42i64).into()),
        ("Uint", 300u16.into()),
        ("Float", 0.25f32.into()),
        ("Bool", true.into()),
        ("String", "hi".into()),
        ("Bytes", vec![1u8, 2].into()),
        ("Array", Value::List(vec![point(1, 2), point(0, 0)])),
        ("Slice", Value::List(vec![point(3, 4)])),
        ("Map", Value::Map(vec![("a".into(), ints), ("empty".into(), Value::List(vec![]))])),
        ("Nested", point(5, 6)),
        ("Iface", iface("main.Point", point(7, 8))),
        ("IfaceInt", iface("int", 9i64.into())),
        ("IfaceSlice", iface("[]string", Value::List(vec!["x".into(), "y".into()]))),
        ("IfaceNil", Value::Nil),
        ("List", list),
        ("ID", vec![7u8; 16].into()),
        ("Color", structure(vec![("R", 1u8.into()), ("G", 2u8.into()), ("B", 3u8.into())])),
        ("Version", b"3.4".to_vec().into()),
    ])
}

#[test]
fn go_decodes_every_kind_of_value() {
    let mut encoder = Encoder::new();
    let everything_type = define_everything(&mut encoder);
    encoder.encode(everything_type, &everything()).unwrap();
    encoder.encode(TypeId::COMPLEX, &Value::Complex(1.5, -2.0)).unwrap();
    encoder.encode(TypeId::INT, &42i64.into()).unwrap();
    encoder.encode(everything_type, &structure(vec![("Int", 1i64.into())])).unwrap();
    let Some(output) = go("verify", &encoder.finish()) else { return };

    // Go prints each value it decoded as JSON.
    let expected = [
        format!(
            concat!(
                r#"{{"Int":-42,"Uint":300,"Float":0.25,"Bool":true,"String":"hi","Bytes":"AQI=","#,
                r#""Array":[{{"X":1,"Y":2}},{{"X":0,"Y":0}}],"Slice":[{{"X":3,"Y":4}}],"#,
                r#""Map":{{"a":[1,0,-1],"empty":null}},"Nested":{{"X":5,"Y":6}},"Iface":{{"X":7,"Y":8}},"#,
                r#""IfaceInt":9,"IfaceSlice":["x","y"],"IfaceNil":null,"#,
                r#""List":{{"Value":1,"Next":{{"Value":2,"Next":null}}}},"ID":[{}],"#,
                r##""Color":"#010203","Version":{{"Major":3,"Minor":4}}}}"##
            ),
            ["7"; 16].join(",")
        ),
        r#""(1.5-2i)""#.to_owned(),
        "42".to_owned(),
        format!(
            concat!(
                r#"{{"Int":1,"Uint":0,"Float":0,"Bool":false,"String":"","Bytes":null,"#,
                r#""Array":[{{"X":0,"Y":0}},{{"X":0,"Y":0}}],"Slice":null,"Map":null,"Nested":{{"X":0,"Y":0}},"#,
                r#""Iface":null,"IfaceInt":null,"IfaceSlice":null,"IfaceNil":null,"List":null,"ID":[{}],"#,
                r##""Color":"#000000","Version":{{"Major":0,"Minor":0}}}}"##
            ),
            ["0"; 16].join(",")
        ),
    ];
    assert_eq!(output.lines().collect::<Vec<_>>(), expected);
}
