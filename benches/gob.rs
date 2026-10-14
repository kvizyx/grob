//! `cargo bench`. `tests/go/bench_test.go` runs the same orders through Go's
//! encoding/gob for comparison (`cd tests/go && go test -bench .`).

use std::collections::BTreeMap;
use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use grob::{Decoder, Encoder, Type, TypeId, Value};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "PascalCase")]
struct Point {
    x: i64,
    y: i64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "PascalCase")]
struct Item {
    #[serde(rename = "SKU")]
    sku: String,
    count: u16,
    price: f32,
}

#[derive(Serialize, Deserialize, Clone)]
enum Extra {
    #[serde(rename = "main.Point")]
    Point(Point),
    #[serde(rename = "int")]
    Int(i64),
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "PascalCase")]
struct Order {
    #[serde(rename = "ID", with = "grob::binary_marshaler")]
    id: [u8; 16],
    customer: String,
    items: Vec<Item>,
    tags: Vec<String>,
    quantities: BTreeMap<String, u32>,
    total: f64,
    paid: bool,
    note: Option<String>,
    position: [i64; 2],
    payload: Vec<u8>,
    extra: Option<Extra>,
    #[serde(with = "grob::gob_encoder")]
    version: String,
}

/// Same data as `makeOrders` in `tests/go/bench_test.go`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    reason = "benchmark data: small values, and bytes that wrap on purpose"
)]
fn orders(count: usize) -> Vec<Order> {
    (0..count)
        .map(|i| {
            let items: Vec<Item> =
                (0..=i % 5).map(|j| Item { sku: format!("sku-{j}"), count: j as u16, price: j as f32 * 1.5 }).collect();
            Order {
                id: std::array::from_fn(|b| (i + b) as u8),
                customer: format!("customer-{i}"),
                quantities: items.iter().map(|item| (item.sku.clone(), item.count.into())).collect(),
                total: items.iter().map(|item| f64::from(item.price) * f64::from(item.count)).sum(),
                items,
                tags: vec![format!("t{}", i % 3), "x".into()],
                paid: i % 2 == 0,
                note: (i % 3 == 0).then(|| format!("note {i}")),
                position: [i as i64, -(i as i64)],
                payload: (0..32).map(|b| (i * b) as u8).collect(),
                extra: Some(if i % 2 == 0 { Extra::Point(Point { x: i as i64, y: 1 }) } else { Extra::Int(i as i64) }),
                version: format!("1.{}", i % 10),
            }
        })
        .collect()
}

/// Declares the order types by hand, as a user of the Value API would.
fn define_order_types(encoder: &mut Encoder) -> TypeId {
    let point = encoder.define_struct("Point", &[("X", TypeId::INT), ("Y", TypeId::INT)]);
    encoder.register("main.Point", point);
    let item =
        encoder.define_struct("Item", &[("SKU", TypeId::STRING), ("Count", TypeId::UINT), ("Price", TypeId::FLOAT)]);
    let items = encoder.define_slice("[]Item", item);
    let strings = encoder.define_slice("[]string", TypeId::STRING);
    let quantities = encoder.define_map("map[string]uint32", TypeId::STRING, TypeId::UINT);
    let position = encoder.define_array("[2]int", TypeId::INT, 2);
    let uuid = encoder.define(Type::BinaryMarshaler { name: "UUID".into() });
    let version = encoder.define(Type::GobEncoder { name: "Version".into() });
    let order = encoder.define_struct(
        "Order",
        &[
            ("ID", uuid),
            ("Customer", TypeId::STRING),
            ("Items", items),
            ("Tags", strings),
            ("Quantities", quantities),
            ("Total", TypeId::FLOAT),
            ("Paid", TypeId::BOOL),
            ("Note", TypeId::STRING),
            ("Position", position),
            ("Payload", TypeId::BYTES),
            ("Extra", TypeId::INTERFACE),
            ("Version", version),
        ],
    );
    encoder.define_slice("[]Order", order)
}

fn orders_benches(c: &mut Criterion) {
    let orders = orders(1000);
    let bytes = grob::to_vec(&orders).unwrap();
    let value = grob::decode(&bytes).unwrap();

    let mut group = c.benchmark_group("orders");
    group.throughput(Throughput::Bytes(bytes.len() as u64));
    group.bench_function("serialize", |b| b.iter(|| grob::to_vec(black_box(&orders)).unwrap()));
    group.bench_function("deserialize", |b| b.iter(|| grob::from_slice::<Vec<Order>>(black_box(&bytes)).unwrap()));
    group.bench_function("encode_value", |b| {
        b.iter(|| {
            let mut encoder = Encoder::new();
            let ty = define_order_types(&mut encoder);
            encoder.encode(ty, black_box(&value)).unwrap();
            encoder.finish()
        });
    });
    group.bench_function("decode_value", |b| b.iter(|| grob::decode(black_box(&bytes)).unwrap()));
    group.bench_function("to_value", |b| b.iter(|| grob::to_value(black_box(&orders)).unwrap()));
    group.bench_function("from_value", |b| {
        b.iter_batched(|| value.clone(), |value| grob::from_value::<Vec<Order>>(value).unwrap(), BatchSize::LargeInput);
    });
    group.finish();
}

/// One message per order on a long-lived stream: types are sent once.
fn stream_benches(c: &mut Criterion) {
    let orders = orders(1000);
    let mut encoder = Encoder::new();
    for order in &orders {
        encoder.serialize(order).unwrap();
    }
    let bytes = encoder.finish();

    let mut group = c.benchmark_group("stream");
    group.throughput(Throughput::Bytes(bytes.len() as u64));
    group.bench_function("serialize", |b| {
        b.iter(|| {
            let mut encoder = Encoder::new();
            for order in &orders {
                encoder.serialize(black_box(order)).unwrap();
            }
            encoder.finish()
        });
    });
    // A stream per order, like Go code creating a `gob.Encoder` per message.
    group.bench_function("serialize_reset", |b| {
        let mut encoder = Encoder::new();
        b.iter(|| {
            let mut total = 0;
            for order in &orders {
                encoder.reset();
                encoder.serialize(black_box(order)).unwrap();
                total += encoder.take().len();
            }
            total
        });
    });
    group.bench_function("to_vec_each", |b| {
        b.iter(|| orders.iter().map(|order| grob::to_vec(black_box(order)).unwrap().len()).sum::<usize>());
    });
    group.bench_function("deserialize_from", |b| {
        b.iter(|| {
            let mut decoder = Decoder::new();
            let mut reader = black_box(bytes.as_slice());
            let mut count = 0;
            while decoder.deserialize_from::<Order>(&mut reader).unwrap().is_some() {
                count += 1;
            }
            count
        });
    });
    group.finish();
}

fn bench_data<T: Serialize + for<'de> Deserialize<'de>>(c: &mut Criterion, name: &str, data: &T) {
    let bytes = grob::to_vec(data).unwrap();
    let mut group = c.benchmark_group(name);
    group.throughput(Throughput::Bytes(bytes.len() as u64));
    group.bench_function("serialize", |b| b.iter(|| grob::to_vec(black_box(data)).unwrap()));
    group.bench_function("deserialize", |b| b.iter(|| grob::from_slice::<T>(black_box(&bytes)).unwrap()));
    group.bench_function("decode_value", |b| b.iter(|| grob::decode(black_box(&bytes)).unwrap()));
    group.finish();
}

fn primitive_benches(c: &mut Criterion) {
    let ints: Vec<i64> = (0..100_000).map(|i| (i * 7919) % 1_000_003 - 500_000).collect();
    let floats: Vec<f64> = (0..100_000).map(|i| f64::from(i) * 0.37).collect();
    let strings: Vec<String> = (0..10_000).map(|i| format!("string number {i}")).collect();
    let blob: Vec<u8> = (0..=u8::MAX).cycle().take(1 << 20).collect();

    bench_data(c, "ints", &ints);
    bench_data(c, "floats", &floats);
    bench_data(c, "strings", &strings);
    bench_data(c, "bytes", &Blob(blob));

    let value = Value::Int(-123_456_789);
    c.bench_function("single_value_round_trip", |b| {
        b.iter(|| {
            let mut encoder = Encoder::new();
            encoder.encode(TypeId::INT, black_box(&value)).unwrap();
            grob::decode(&encoder.finish()).unwrap()
        });
    });
}

/// A `Vec<u8>` that serializes through `serialize_bytes`, like `serde_bytes`.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct Blob(#[serde(with = "blob")] Vec<u8>);

mod blob {
    pub fn serialize<S: serde::Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Vec<u8>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("bytes")
            }

            fn visit_bytes<E>(self, v: &[u8]) -> Result<Vec<u8>, E> {
                Ok(v.to_vec())
            }

            fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Vec<u8>, E> {
                Ok(v)
            }
        }
        deserializer.deserialize_byte_buf(Visitor)
    }
}

criterion_group!(benches, orders_benches, stream_benches, primitive_benches);
criterion_main!(benches);
