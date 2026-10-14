//! Helpers shared by the integration tests.

#![allow(dead_code, reason = "each test crate uses some of the helpers")]

use std::path::{Path, PathBuf};
use std::process::Command;

use grob::Value;

/// The fixtures `tests/go` generates with Go's encoding/gob
/// (`cd tests/go && go run . gen ../fixtures`).
pub const FIXTURES: &[&str] = &[
    "primitives.gob",
    "nested.gob",
    "singleton.gob",
    "interfaces.gob",
    "stream.gob",
    "list.gob",
    "external.gob",
    "orders.gob",
];

pub fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
}

pub fn decode_fixture(name: &str) -> Vec<Value> {
    grob::Decoder::new().decode_all(&fixture(name)).unwrap()
}

/// Runs `go run . <command> <file>` in `tests/go` on a file holding `stream`
/// and returns its output, or `None` when Go is not installed.
///
/// # Panics
///
/// When the Go program fails, with its error output.
pub fn go(command: &str, stream: &[u8]) -> Option<String> {
    if Command::new("go").arg("version").output().is_err() {
        eprintln!("skipping: go is not installed");
        return None;
    }
    let file = temp_file(command);
    std::fs::write(&file, stream).unwrap();
    let output = Command::new("go")
        .args(["run", ".", command])
        .arg(&file)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/go"))
        .output()
        .unwrap();
    std::fs::remove_file(&file).unwrap();
    assert!(output.status.success(), "go {command} failed: {}", String::from_utf8_lossy(&output.stderr));
    Some(String::from_utf8(output.stdout).unwrap())
}

fn temp_file(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("grob-{name}-{}.gob", std::process::id()))
}

pub fn structure(fields: Vec<(&str, Value)>) -> Value {
    Value::Struct(fields.into_iter().map(|(name, value)| (name.into(), value)).collect())
}

pub fn point(x: i64, y: i64) -> Value {
    structure(vec![("X", x.into()), ("Y", y.into())])
}

pub fn iface(name: &str, value: Value) -> Value {
    Value::Interface { name: name.to_owned(), value: Box::new(value) }
}
