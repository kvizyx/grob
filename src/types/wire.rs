//! How types describe themselves on the wire: gob sends type definitions as
//! values of its own `wireType` struct, whose types every stream knows from
//! the start (encoding/gob type.go).

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

use super::{Entry, Field, Type, TypeId};
use crate::{Error, Value};

/// Ids of gob's type-description structs, fixed by encoding/gob (`checkId`
/// calls in type.go). They never appear in user streams except as the type
/// of type definitions.
pub(crate) const WIRE_TYPE: TypeId = TypeId::new(16);
const ARRAY_TYPE: TypeId = TypeId::new(17);
const COMMON_TYPE: TypeId = TypeId::new(18);
const SLICE_TYPE: TypeId = TypeId::new(19);
const STRUCT_TYPE: TypeId = TypeId::new(20);
const FIELD_TYPE: TypeId = TypeId::new(21);
const FIELD_TYPE_SLICE: TypeId = TypeId::new(22);
const MAP_TYPE: TypeId = TypeId::new(23);
/// `gobEncoderType` has no fixed id in Go, since it is never sent on its own;
/// this one only needs to be distinct within the bootstrap table.
const GOB_ENCODER_TYPE: TypeId = TypeId::new(24);

/// gob's type-description structs, which every stream starts with.
pub(super) static BOOTSTRAP: LazyLock<BTreeMap<TypeId, Arc<Entry>>> = LazyLock::new(|| {
    let common = ("CommonType", COMMON_TYPE);
    BTreeMap::from([
        (COMMON_TYPE, structure("CommonType", &[("Name", TypeId::STRING), ("Id", TypeId::INT)])),
        (ARRAY_TYPE, structure("arrayType", &[common, ("Elem", TypeId::INT), ("Len", TypeId::INT)])),
        (SLICE_TYPE, structure("sliceType", &[common, ("Elem", TypeId::INT)])),
        (FIELD_TYPE, structure("fieldType", &[("Name", TypeId::STRING), ("Id", TypeId::INT)])),
        (FIELD_TYPE_SLICE, Arc::new(Entry::new(Type::Slice { name: "[]fieldType".into(), elem: FIELD_TYPE }))),
        (STRUCT_TYPE, structure("structType", &[common, ("Field", FIELD_TYPE_SLICE)])),
        (MAP_TYPE, structure("mapType", &[common, ("Key", TypeId::INT), ("Elem", TypeId::INT)])),
        (GOB_ENCODER_TYPE, structure("gobEncoderType", &[common])),
        (
            WIRE_TYPE,
            structure(
                "wireType",
                &[
                    ("ArrayT", ARRAY_TYPE),
                    ("SliceT", SLICE_TYPE),
                    ("StructT", STRUCT_TYPE),
                    ("MapT", MAP_TYPE),
                    ("GobEncoderT", GOB_ENCODER_TYPE),
                    ("BinaryMarshalerT", GOB_ENCODER_TYPE),
                    ("TextMarshalerT", GOB_ENCODER_TYPE),
                ],
            ),
        ),
    ])
});

fn structure(name: &str, fields: &[(&str, TypeId)]) -> Arc<Entry> {
    let fields = fields.iter().map(|&(name, id)| Field::new(name, id)).collect();
    Arc::new(Entry::new(Type::Struct { name: name.to_owned(), fields }))
}

fn id_value(id: TypeId) -> Value {
    Value::Int(id.get().into())
}

impl Type {
    /// The `wireType` value that defines this type under `id`.
    pub(crate) fn to_wire(&self, id: TypeId) -> Value {
        let common = Value::Struct(vec![("Name".into(), self.name().into()), ("Id".into(), id_value(id))]);
        let mut body = vec![("CommonType".into(), common)];
        let variant = match self {
            Type::Array { elem, len, .. } => {
                let len = i64::try_from(*len).expect("array lengths fit in an int64");
                body.extend([("Elem".into(), id_value(*elem)), ("Len".into(), Value::Int(len))]);
                "ArrayT"
            }
            Type::Slice { elem, .. } => {
                body.push(("Elem".into(), id_value(*elem)));
                "SliceT"
            }
            Type::Struct { fields, .. } => {
                let fields = fields
                    .iter()
                    .map(|field| {
                        Value::Struct(vec![
                            ("Name".into(), field.name.as_str().into()),
                            ("Id".into(), id_value(field.id)),
                        ])
                    })
                    .collect();
                body.push(("Field".into(), Value::List(fields)));
                "StructT"
            }
            Type::Map { key, elem, .. } => {
                body.extend([("Key".into(), id_value(*key)), ("Elem".into(), id_value(*elem))]);
                "MapT"
            }
            Type::GobEncoder { .. } => "GobEncoderT",
            Type::BinaryMarshaler { .. } => "BinaryMarshalerT",
            Type::TextMarshaler { .. } => "TextMarshalerT",
        };
        Value::Struct(vec![(variant.into(), Value::Struct(body))])
    }

    /// Reads a type definition from its `wireType` value. Fields gob left out
    /// are zero, like everywhere else.
    pub(crate) fn from_wire(value: &Value) -> Result<Type, Error> {
        let Value::Struct(variants) = value else {
            return Err(Error::syntax("type definition is not a struct"));
        };
        let [(variant, body)] = variants.as_slice() else {
            return Err(Error::syntax("type definition must describe exactly one kind of type"));
        };
        let name = body.get("CommonType").map(|common| string(common, "Name")).unwrap_or_default();

        Ok(match &**variant {
            "ArrayT" => {
                let len = usize::try_from(int(body, "Len")).map_err(|_| Error::syntax("negative array length"))?;
                Type::Array { name, elem: id(body, "Elem")?, len }
            }
            "SliceT" => Type::Slice { name, elem: id(body, "Elem")? },
            "StructT" => {
                let fields = body
                    .get("Field")
                    .and_then(Value::as_list)
                    .unwrap_or_default()
                    .iter()
                    .map(|field| Ok(Field::new(string(field, "Name"), id(field, "Id")?)))
                    .collect::<Result<_, Error>>()?;
                Type::Struct { name, fields }
            }
            "MapT" => Type::Map { name, key: id(body, "Key")?, elem: id(body, "Elem")? },
            "GobEncoderT" => Type::GobEncoder { name },
            "BinaryMarshalerT" => Type::BinaryMarshaler { name },
            "TextMarshalerT" => Type::TextMarshaler { name },
            other => return Err(Error::syntax(format!("unknown kind of type {other}"))),
        })
    }
}

fn string(value: &Value, field: &str) -> String {
    value.get(field).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn int(value: &Value, field: &str) -> i64 {
    value.get(field).and_then(Value::as_i64).unwrap_or_default()
}

fn id(value: &Value, field: &str) -> Result<TypeId, Error> {
    i32::try_from(int(value, field)).map(TypeId::new).map_err(|_| Error::syntax("type id out of range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_round_trip_through_wire_values() {
        let types = [
            Type::Array { name: "[3]float64".into(), elem: TypeId::FLOAT, len: 3 },
            Type::Array { name: String::new(), elem: TypeId::FLOAT, len: 0 },
            Type::Slice { name: "[]string".into(), elem: TypeId::STRING },
            Type::Struct {
                name: "T".into(),
                fields: vec![Field::new("A", TypeId::INT), Field::new("B", TypeId::new(65))],
            },
            Type::Struct { name: "Empty".into(), fields: vec![] },
            Type::Map { name: "map[string][]byte".into(), key: TypeId::STRING, elem: TypeId::BYTES },
            Type::GobEncoder { name: "G".into() },
            Type::BinaryMarshaler { name: "B".into() },
            Type::TextMarshaler { name: "T".into() },
        ];
        for ty in types {
            assert_eq!(Type::from_wire(&ty.to_wire(TypeId::new(64))).unwrap(), ty);
        }
    }
}
