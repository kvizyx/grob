//! Arbitrary input through every decoding entry point: errors are fine,
//! panics are not.

#![no_main]

use grob::{Decoder, Value};
use grob_fuzz::{Any, Chunked};
use libfuzzer_sys::fuzz_target;
use serde::Deserialize;
use serde::de::IgnoredAny;

/// A typed target, so typed deserialization paths (zero fill, enums,
/// externals, skipping unknown fields) run too.
#[derive(Deserialize)]
#[expect(dead_code, reason = "only deserialized")]
struct Typed {
    #[serde(rename = "A")]
    a: i64,
    #[serde(rename = "B")]
    b: Vec<String>,
    #[serde(rename = "C")]
    c: Option<Box<Typed>>,
    #[serde(rename = "D")]
    d: [u8; 2],
    #[serde(rename = "E")]
    e: std::collections::BTreeMap<String, f64>,
    #[serde(rename = "F", with = "grob::binary_marshaler")]
    f: Vec<u8>,
    #[serde(rename = "G")]
    g: Option<Variant>,
}

#[derive(Deserialize)]
#[expect(dead_code, reason = "only deserialized")]
enum Variant {
    #[serde(rename = "int")]
    Int(i64),
    #[serde(rename = "main.Typed")]
    Typed(Box<Typed>),
    #[serde(rename = "main.Pair")]
    Pair {
        x: u8,
        y: u8,
    },
    Unit,
}

fuzz_target!(|input: (&[u8], &[u8])| {
    let (data, chunks) = input;
    let _values: Result<Vec<Value>, _> = Decoder::new().decode_all(data);
    let _typed = grob::from_slice::<Typed>(data);
    let _ignored = grob::from_slice::<IgnoredAny>(data);

    let mut decoder = Decoder::new().with_max_depth(32);
    let mut reader = Chunked::new(data, chunks);
    while let Ok(Some(_)) = decoder.decode_from(&mut reader) {}

    let mut decoder = Decoder::new();
    let mut reader = Chunked::new(data, chunks);
    while let Ok(Some(_)) = decoder.deserialize_from::<Any>(&mut reader) {}
});
