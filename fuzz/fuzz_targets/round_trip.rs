//! Arbitrary Rust values, serialized as a stream of several values on one
//! encoder, come back the same through both decoding paths, and so do the
//! same values serialized each on a stream of its own. Values without a gob
//! equivalent may only fail as data errors.

#![no_main]

use std::collections::BTreeMap;

use arbitrary::{Arbitrary, Unstructured};
use grob::{Decoder, Encoder, Value};
use libfuzzer_sys::fuzz_target;
use serde::{Deserialize, Serialize};

#[derive(Arbitrary, Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct Record {
    flag: bool,
    small: i8,
    signed: i64,
    unsigned: u32,
    byte: u8,
    #[arbitrary(with = finite)]
    float: f64,
    text: String,
    letter: char,
    bytes: Vec<u8>,
    #[serde(rename = "ID", with = "grob::binary_marshaler")]
    id: [u8; 16],
    #[serde(with = "grob::gob_encoder")]
    version: String,
    pair: [i16; 2],
    items: Vec<Item>,
    nested: Vec<Vec<u16>>,
    by_name: BTreeMap<String, Item>,
    by_number: BTreeMap<i64, Vec<String>>,
    maybe: Option<Item>,
    any: Option<Any>,
    anys: Vec<Any>,
    kind: Kind,
    #[arbitrary(with = short_list)]
    list: Option<Box<Node>>,
    unit: (),
}

#[derive(Arbitrary, Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct Item {
    name: String,
    count: i32,
    tags: Vec<String>,
}

/// A Go `any` holding types the Go side registered.
#[derive(Arbitrary, Serialize, Deserialize, Debug)]
enum Any {
    #[serde(rename = "main.Item")]
    Item(Item),
    #[serde(rename = "int")]
    Int(i64),
    #[serde(rename = "string")]
    Text(String),
    #[serde(rename = "main.Shape")]
    Shape { sides: u8, label: String },
}

/// Go string constants.
#[derive(Arbitrary, Serialize, Deserialize, Debug)]
enum Kind {
    Small,
    Medium,
    Large,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct Node {
    value: i64,
    next: Option<Box<Node>>,
}

/// Floats without NaN, which never equals itself.
fn finite(u: &mut Unstructured) -> arbitrary::Result<f64> {
    let float = f64::arbitrary(u)?;
    Ok(if float.is_nan() { 0.0 } else { float })
}

/// A linked list short enough to stay within the decoder's nesting limit.
fn short_list(u: &mut Unstructured) -> arbitrary::Result<Option<Box<Node>>> {
    let values: Vec<i64> = u.arbitrary_iter()?.take(64).collect::<arbitrary::Result<_>>()?;
    Ok(values.into_iter().rev().fold(None, |next, value| Some(Box::new(Node { value, next }))))
}

/// A value with struct fields sorted by name: decoded structs list fields
/// in the stream's definition order, which may differ from Rust's.
fn normalized(value: Value) -> Value {
    match value {
        Value::Struct(mut fields) => {
            fields.sort_by(|(a, _), (b, _)| a.cmp(b));
            Value::Struct(fields.into_iter().map(|(name, value)| (name, normalized(value))).collect())
        }
        Value::List(items) => Value::List(items.into_iter().map(normalized).collect()),
        Value::Map(entries) => Value::Map(entries.into_iter().map(|(k, v)| (normalized(k), normalized(v))).collect()),
        Value::Interface { name, value } => Value::Interface { name, value: Box::new(normalized(*value)) },
        value => value,
    }
}

/// What gob makes of a record, the way Go would see it.
fn gob_view(record: &Record) -> Value {
    normalized(grob::to_value(record).expect("records that serialize convert to values"))
}

fuzz_target!(|records: Vec<Record>| {
    let mut encoder_per_stream = Encoder::new();
    let mut encoder = Encoder::new();
    let mut sent = Vec::new();
    for record in records.iter().take(4) {
        match encoder.serialize(record) {
            Ok(()) => sent.push(record),
            Err(error) => assert!(error.is_data(), "unexpected error {error:?} for {record:?}"),
        }
    }
    let stream = encoder.finish();

    let mut decoder = Decoder::new();
    let mut reader = stream.as_slice();
    for record in &sent {
        let read: Record = decoder.deserialize_from(&mut reader).unwrap().expect("one record per value");
        assert_eq!(gob_view(&read), gob_view(record));
    }
    assert!(decoder.deserialize_from::<Record>(&mut reader).unwrap().is_none());

    let values = Decoder::new().decode_all(&stream).unwrap();
    assert_eq!(values.len(), sent.len());
    for (value, record) in values.into_iter().zip(&sent) {
        assert_eq!(normalized(value), gob_view(record));
    }

    // Each record also as a stream of its own, starting from what earlier
    // new streams defined.
    for record in &sent {
        encoder_per_stream.reset();
        encoder_per_stream.serialize(*record).expect("serializes on a new stream as it did on a shared one");
        let read: Record = grob::from_slice(&encoder_per_stream.take()).unwrap();
        assert_eq!(gob_view(&read), gob_view(record));
    }
});
