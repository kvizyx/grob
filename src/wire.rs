//! Gob's primitive encodings: unsigned and signed integers, floats and
//! length-prefixed byte strings.
//!
//! An unsigned integer below 0x80 is one byte. Larger ones are a byte holding
//! the negated byte count, then the value's big-endian bytes without leading
//! zeros. Signed integers shift left by one and complement negative values,
//! keeping the sign in the low bit. Floats are their IEEE 754 bits,
//! byte-reversed, sent as an unsigned integer.

use crate::Error;

/// Builds gob-encoded bytes.
#[derive(Debug, Default)]
pub(crate) struct Writer {
    buf: Vec<u8>,
}

/// Appends an unsigned integer to `buf`.
#[inline]
pub(crate) fn push_uint(buf: &mut Vec<u8>, value: u64) {
    if let Ok(byte @ 0..0x80) = u8::try_from(value) {
        buf.push(byte);
        return;
    }
    let skip = value.leading_zeros() / 8;
    let mut bytes = [0; 9];
    // The count byte is the negated number of value bytes, 1 to 8.
    #[expect(clippy::cast_possible_truncation, reason = "skip is below 8")]
    let count = (skip as u8).wrapping_sub(8);
    bytes[0] = count;
    // The value bytes, moved to the front. Copying all nine bytes and
    // dropping the extra ones is faster than copying just the right number.
    bytes[1..].copy_from_slice(&(value << (8 * skip)).to_be_bytes());
    let len = buf.len();
    buf.extend_from_slice(&bytes);
    buf.truncate(len + 9 - skip as usize);
}

impl Writer {
    /// A writer appending to `buf`, after clearing it.
    pub fn from_vec(mut buf: Vec<u8>) -> Self {
        buf.clear();
        Self { buf }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    #[inline]
    pub fn uint(&mut self, value: u64) {
        push_uint(&mut self.buf, value);
    }

    #[inline]
    pub fn int(&mut self, value: i64) {
        // Shifted left, with all bits flipped for negative values: the sign
        // mask does that without a branch, which random signs would make
        // costly.
        self.uint((value.cast_unsigned() << 1) ^ (value >> 63).cast_unsigned());
    }

    #[inline]
    pub fn bool(&mut self, value: bool) {
        self.uint(value.into());
    }

    #[inline]
    pub fn float(&mut self, value: f64) {
        self.uint(value.to_bits().swap_bytes());
    }

    #[inline]
    pub fn bytes(&mut self, value: &[u8]) {
        self.uint(value.len() as u64);
        self.buf.extend_from_slice(value);
    }

    /// Appends raw bytes, without a length.
    #[cfg_attr(not(feature = "serde"), expect(dead_code, reason = "used by the serde writer"))]
    #[inline]
    pub fn raw(&mut self, value: &[u8]) {
        self.buf.extend_from_slice(value);
    }

    /// The bytes written since `start`.
    #[cfg_attr(not(feature = "serde"), expect(dead_code, reason = "used by the serde writer"))]
    pub fn since(&self, start: usize) -> &[u8] {
        &self.buf[start..]
    }

    /// Drops everything written after `len` bytes.
    #[cfg_attr(not(feature = "serde"), expect(dead_code, reason = "used by the serde writer"))]
    #[inline]
    pub fn truncate(&mut self, len: usize) {
        self.buf.truncate(len);
    }

    /// Inserts the length of everything written since `start` before it.
    pub fn prefix_length(&mut self, start: usize) {
        self.prefix_uint(start, (self.buf.len() - start) as u64);
    }

    /// Inserts `value` at `start`, before what was written since.
    pub fn prefix_uint(&mut self, start: usize, value: u64) {
        let end = self.buf.len();
        push_uint(&mut self.buf, value);
        let size = self.buf.len() - end;
        self.buf[start..].rotate_right(size);
    }

    /// Makes room for `additional` more bytes.
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.buf.reserve(additional);
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}

/// Reads an unsigned integer at the start of `data`; returns it and its size.
#[inline]
pub(crate) fn read_uint(data: &[u8]) -> Result<(u64, usize), Error> {
    let Some(&first) = data.first() else {
        return Err(Error::eof());
    };
    if first < 0x80 {
        return Ok((first.into(), 1));
    }
    let size = usize::from(first.wrapping_neg());
    if size > 8 {
        return Err(Error::syntax("invalid uint length"));
    }
    // Usually there is room to load a whole word and drop the extra bytes.
    if let Some(word) = data.get(1..9) {
        let word = u64::from_be_bytes(word.try_into().expect("slice of 8 bytes"));
        return Ok((word >> (8 * (8 - size)), size + 1));
    }
    let bytes = data.get(1..=size).ok_or_else(Error::eof)?;
    Ok((bytes.iter().fold(0, |value, byte| value << 8 | u64::from(*byte)), size + 1))
}

/// Reads a length-prefixed byte string; returns it and the total size.
#[inline]
pub(crate) fn read_bytes(data: &[u8]) -> Result<(&[u8], usize), Error> {
    let (length, prefix) = read_uint(data)?;
    let rest = &data[prefix..];
    match usize::try_from(length).ok().and_then(|length| rest.get(..length)) {
        Some(bytes) => Ok((bytes, prefix + bytes.len())),
        None => Err(Error::eof()),
    }
}

/// Decodes a signed integer from its unsigned encoding.
#[inline]
pub(crate) fn uint_to_int(encoded: u64) -> i64 {
    let shifted = (encoded >> 1).cast_signed();
    if encoded & 1 == 1 { !shifted } else { shifted }
}

/// Decodes a float from its unsigned encoding.
#[inline]
pub(crate) fn uint_to_float(encoded: u64) -> f64 {
    f64::from_bits(encoded.swap_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(write: impl FnOnce(&mut Writer)) -> Vec<u8> {
        let mut w = Writer::default();
        write(&mut w);
        w.into_inner()
    }

    #[test]
    fn integers_round_trip() {
        for value in [0u64, 1, 0x7f, 0x80, 0xff, 0x100, 0xdead_beef, u64::MAX] {
            let bytes = encode(|w| w.uint(value));
            assert_eq!(read_uint(&bytes).unwrap(), (value, bytes.len()));
        }
        for value in [0i64, 1, -1, 63, -64, 64, i64::MIN, i64::MAX] {
            let bytes = encode(|w| w.int(value));
            assert_eq!(uint_to_int(read_uint(&bytes).unwrap().0), value);
        }
        for value in [0.0, -0.0, 17.5, -1e300, f64::INFINITY] {
            let bytes = encode(|w| w.float(value));
            assert_eq!(uint_to_float(read_uint(&bytes).unwrap().0).to_bits(), value.to_bits());
        }
    }

    #[test]
    fn matches_the_encodings_documented_by_go() {
        assert_eq!(encode(|w| w.uint(7)), [0x07]);
        assert_eq!(encode(|w| w.uint(256)), [0xfe, 0x01, 0x00]);
        assert_eq!(encode(|w| w.int(-129)), [0xfe, 0x01, 0x01]);
        assert_eq!(encode(|w| w.float(17.0)), [0xfe, 0x31, 0x40]);
    }

    #[test]
    fn short_input_is_eof_and_bad_lengths_are_syntax_errors() {
        assert!(read_uint(&[]).unwrap_err().is_eof());
        assert!(read_uint(&[0xfe, 0x01]).unwrap_err().is_eof());
        assert!(read_uint(&[0xf0]).unwrap_err().is_syntax());
        assert!(read_bytes(&[0x05, 0x01]).unwrap_err().is_eof());
    }

    #[test]
    fn prefixes_are_inserted_before_the_data() {
        let mut w = Writer::default();
        w.uint(1);
        let start = w.len();
        w.bytes(&[0; 200]);
        w.prefix_length(start);
        let bytes = w.into_inner();
        assert_eq!(bytes[0], 1);
        let (length, size) = read_uint(&bytes[1..]).unwrap();
        assert_eq!(usize::try_from(length).unwrap(), bytes.len() - 1 - size);
        assert_eq!(read_bytes(&bytes[1 + size..]).unwrap().0, [0; 200]);
    }
}
