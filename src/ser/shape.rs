//! Gob types as inferred from serialized values.

use std::collections::BTreeSet;
use std::fmt;

use crate::Error;

/// The gob type of a serialized value. Structs are referenced by their serde
/// name and described separately (see [`Schema`](super::schema::Schema)).
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum Shape {
    /// Not known yet: `None`, or the element of an empty collection.
    Unknown,
    Bool,
    Int,
    Uint,
    /// A `u8`; a sequence of them is a `[]byte`.
    Byte,
    Float,
    String,
    Bytes,
    Interface,
    GobEncoder,
    BinaryMarshaler,
    Array(Box<Shape>, usize),
    Seq(Box<Shape>),
    Map(Box<Shape>, Box<Shape>),
    Struct(&'static str),
}

impl fmt::Display for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Shape::Unknown => f.write_str("unknown"),
            Shape::Bool => f.write_str("bool"),
            Shape::Int => f.write_str("int"),
            Shape::Uint => f.write_str("uint"),
            Shape::Byte => f.write_str("uint8"),
            Shape::Float => f.write_str("float"),
            Shape::String => f.write_str("string"),
            Shape::Bytes => f.write_str("[]byte"),
            Shape::Interface => f.write_str("interface"),
            Shape::GobEncoder => f.write_str("GobEncoder"),
            Shape::BinaryMarshaler => f.write_str("BinaryMarshaler"),
            Shape::Array(elem, len) => write!(f, "[{len}]{elem}"),
            Shape::Seq(elem) => write!(f, "[]{elem}"),
            Shape::Map(key, elem) => write!(f, "map[{key}]{elem}"),
            Shape::Struct(name) => write!(f, "struct {name}"),
        }
    }
}

impl Shape {
    /// Merges `other` into `self`, so `self` describes values of both.
    #[inline]
    pub fn merge(&mut self, other: Shape) -> Result<(), Error> {
        // Usually the same shape again, e.g. for every element of a list.
        if *self == other { Ok(()) } else { self.merge_different(other) }
    }

    fn merge_different(&mut self, other: Shape) -> Result<(), Error> {
        match (self, other) {
            (_, Shape::Unknown) | (Shape::Uint, Shape::Byte) => {}
            (this @ Shape::Unknown, other) | (this @ Shape::Byte, other @ Shape::Uint) => *this = other,
            (Shape::Array(a, n), Shape::Array(b, m)) if *n == m => a.merge(*b)?,
            (Shape::Seq(a), Shape::Seq(b)) => a.merge(*b)?,
            (Shape::Map(ak, av), Shape::Map(bk, bv)) => {
                ak.merge(*bk)?;
                av.merge(*bv)?;
            }
            (this, other) => return Err(Error::data(format!("conflicting types {this} and {other}"))),
        }
        Ok(())
    }

    /// Whether `self` already describes every value `other` does.
    pub fn covers(&self, other: &Shape) -> bool {
        match (self, other) {
            (_, Shape::Unknown) | (Shape::Uint, Shape::Byte) => true,
            (Shape::Array(a, n), Shape::Array(b, m)) => n == m && a.covers(b),
            (Shape::Seq(a), Shape::Seq(b)) => a.covers(b),
            (Shape::Map(ak, av), Shape::Map(bk, bv)) => ak.covers(bk) && av.covers(bv),
            (this, other) => this == other,
        }
    }

    /// Checks that every type that will be sent is known.
    ///
    /// Struct fields never hold unknown shapes, since empty values are left
    /// out of structs. Collection elements are always sent, so a collection
    /// whose elements are all empty collections has no element type.
    pub fn check_known(&self) -> Result<(), Error> {
        match self {
            Shape::Unknown => Err(Error::data("cannot infer the element type of an empty collection")),
            Shape::Array(elem, _) | Shape::Seq(elem) => elem.check_known(),
            Shape::Map(key, elem) => {
                key.check_known()?;
                elem.check_known()
            }
            _ => Ok(()),
        }
    }

    /// Whether the shape refers to one of `structs`.
    pub fn references(&self, structs: &BTreeSet<&str>) -> bool {
        match self {
            Shape::Struct(name) => structs.contains(name),
            Shape::Array(elem, _) | Shape::Seq(elem) => elem.references(structs),
            Shape::Map(key, elem) => key.references(structs) || elem.references(structs),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(elem: Shape) -> Shape {
        Shape::Seq(Box::new(elem))
    }

    #[test]
    fn merging_fills_in_unknown_parts_and_widens_bytes() {
        let mut shape = seq(Shape::Unknown);
        shape.merge(seq(Shape::Byte)).unwrap();
        assert_eq!(shape, seq(Shape::Byte));
        shape.merge(seq(Shape::Uint)).unwrap();
        assert_eq!(shape, seq(Shape::Uint));
        shape.merge(seq(Shape::Byte)).unwrap();
        assert_eq!(shape, seq(Shape::Uint));
        assert!(shape.merge(seq(Shape::Int)).unwrap_err().is_data());
    }

    #[test]
    fn a_merged_shape_covers_what_was_merged_into_it() {
        let parts = [Shape::Unknown, Shape::Byte, Shape::Uint];
        for a in &parts {
            for b in &parts {
                let mut merged = seq(a.clone());
                merged.merge(seq(b.clone())).unwrap();
                assert!(merged.covers(&seq(a.clone())) && merged.covers(&seq(b.clone())), "{a} {b}");
            }
        }
        assert!(!Shape::Array(Box::new(Shape::Int), 2).covers(&Shape::Array(Box::new(Shape::Int), 3)));
    }
}
