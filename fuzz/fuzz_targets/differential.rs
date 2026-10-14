//! The two deserializers, through a `Value` and straight from the stream,
//! must agree on every input.

#![no_main]

use grob::Decoder;
use grob_fuzz::Any;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut via_value = Decoder::new();
    let mut direct = Decoder::new();
    let mut via_value_input = data;
    let mut direct_input = data;
    loop {
        let value = via_value.decode_from(&mut via_value_input).map(|value| value.map(|v| grob::from_value::<Any>(v)));
        let read = direct.deserialize_from::<Any>(&mut direct_input);
        match (value, read) {
            (Ok(Some(value)), Ok(Some(read))) => assert_eq!(value.expect("anything fits Any"), read),
            (Ok(None), Ok(None)) | (Err(_), Err(_)) => return,
            (value, read) => panic!("deserializers disagree: {value:?} vs {read:?}"),
        }
        assert_eq!(via_value_input.len(), direct_input.len(), "both read the same amount");
    }
});
