//! Which way values get written, which only the crate itself can observe.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::thread::LocalKey;

use serde::{Deserialize, Serialize};

use crate::{Decoder, Encoder};

thread_local! {
    pub(super) static DIRECT: Cell<usize> = const { Cell::new(0) };
    pub(super) static INFERRED: Cell<usize> = const { Cell::new(0) };
    pub(super) static BUILT: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn count(counter: &'static LocalKey<Cell<usize>>) {
    counter.with(|c| c.set(c.get() + 1));
}

fn counts() -> [usize; 3] {
    [DIRECT.with(Cell::get), INFERRED.with(Cell::get), BUILT.with(Cell::get)]
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
struct Point {
    x: i64,
    y: i64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
enum Any {
    #[serde(rename = "main.Point")]
    Point(Point),
    #[serde(rename = "int")]
    Int(i64),
    #[serde(rename = "main.Shape")]
    Shape { sides: u8, name: String },
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
struct Node {
    value: i64,
    next: Option<Box<Node>>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Default)]
struct All {
    flag: bool,
    small: i8,
    big: u64,
    float: f32,
    letter: char,
    text: String,
    bytes: Vec<u8>,
    array: [u8; 4],
    points: Vec<Point>,
    grid: Vec<Vec<i64>>,
    pair: (i64, i64),
    map: BTreeMap<String, Vec<u16>>,
    any: Option<Any>,
    anys: Vec<Any>,
    list: Option<Node>,
    unit: (),
    #[serde(with = "crate::binary_marshaler")]
    id: [u8; 16],
    #[serde(with = "crate::gob_encoder")]
    version: String,
}

fn sample(seed: u8) -> All {
    let signed = i64::from(seed);
    All {
        flag: seed.is_multiple_of(2),
        small: i8::try_from(seed % 100).unwrap(),
        big: u64::from(seed) * 1000,
        float: f32::from(seed) / 4.0,
        letter: 'é',
        text: format!("text {seed}"),
        bytes: vec![seed; usize::from(seed % 4)],
        array: [1, 2, 3, seed],
        points: (0..signed % 3).map(|i| Point { x: i, y: -i }).collect(),
        grid: vec![vec![signed], vec![], vec![1, 2]],
        pair: (signed, -signed),
        map: BTreeMap::from([("a".into(), vec![1, 2]), ("b".into(), vec![])]),
        any: Some(if seed.is_multiple_of(2) { Any::Point(Point { x: signed, y: 0 }) } else { Any::Int(signed) }),
        anys: vec![Any::Shape { sides: 3, name: "tri".into() }, Any::Int(0), Any::Point(Point::default())],
        list: Some(Node { value: 1, next: Some(Box::new(Node { value: 2, next: None })) }),
        unit: (),
        id: [seed; 16],
        version: format!("1.{seed}"),
    }
}

#[test]
fn values_are_written_directly_once_their_types_are_known() {
    let mut encoder = Encoder::new();
    let values: Vec<All> = (0..20).map(sample).collect();
    let before = counts();
    for value in &values {
        encoder.serialize(value).unwrap();
    }
    let after = counts();
    let [direct, inferred, built] = [0, 1, 2].map(|i| after[i] - before[i]);
    assert_eq!(built, 0, "never falls back to building values");
    assert!(inferred <= 4, "types settle quickly, yet were inferred {inferred} times");
    assert_eq!(direct + inferred, values.len());

    let stream = encoder.finish();
    let mut reader = stream.as_slice();
    let mut decoder = Decoder::new();
    for value in &values {
        assert_eq!(decoder.deserialize_from::<All>(&mut reader).unwrap().as_ref(), Some(value));
    }
}

#[test]
fn direct_and_value_paths_agree() {
    for seed in 0..10 {
        let value = sample(seed);
        let bytes = crate::to_vec(&value).unwrap();
        let built = crate::to_value(&value).unwrap();
        assert_eq!(crate::decode(&bytes).unwrap(), built);
        assert_eq!(crate::from_slice::<All>(&bytes).unwrap(), value);
        assert_eq!(crate::from_value::<All>(built).unwrap(), value);
    }
}

#[test]
fn new_streams_reuse_what_earlier_new_streams_defined() {
    let mut encoder = Encoder::new();
    let values: Vec<All> = (0..20).map(sample).collect();
    let mut streams = Vec::new();
    let before = counts();
    for value in &values {
        encoder.reset();
        encoder.serialize(value).unwrap();
        streams.push(encoder.take());
    }
    let after = counts();
    let [direct, inferred, built] = [0, 1, 2].map(|i| after[i] - before[i]);
    assert_eq!(built, 0, "never falls back to building values");
    assert!(inferred <= 4, "types settle quickly, yet were inferred {inferred} times");
    assert_eq!(direct + inferred, values.len());

    // Each stream stands on its own.
    for (stream, value) in streams.iter().zip(&values) {
        assert_eq!(&crate::from_slice::<All>(stream).unwrap(), value);
    }
}

#[test]
fn a_failed_value_leaves_a_new_stream_new() {
    let mut encoder = Encoder::new();
    encoder.serialize(&sample(1)).unwrap();
    encoder.take();
    encoder.reset();

    // Restored definitions type what inference alone could not.
    let mut value = sample(1);
    value.grid = vec![vec![]];
    assert!(crate::to_vec(&value).unwrap_err().is_data());
    let list: Vec<Option<All>> = vec![Some(sample(3))];
    encoder.serialize(&list).unwrap();
    encoder.take();
    encoder.reset();
    // The list's definitions are restored, then the value fails.
    let failing: Vec<Option<All>> = vec![Some(sample(3)), None];
    assert!(encoder.serialize(&failing).is_err());
    assert!(encoder.take().is_empty());
    encoder.serialize(&value).unwrap();
    assert_eq!(crate::from_slice::<All>(&encoder.take()).unwrap(), value);
}
