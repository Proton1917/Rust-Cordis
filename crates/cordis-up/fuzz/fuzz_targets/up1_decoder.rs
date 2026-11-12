#![no_main]

use cordis_up::{Limits, Message, decode, encode, payload_refs};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    if let Ok(message) = decode(data, &limits) {
        let value = match &message {
            Message::Request { params, .. } | Message::Notification { params, .. } => params,
            Message::Response { result: Ok(value), .. } => value,
            Message::Response { result: Err(error), .. } => &error.detail,
        };
        let _ = payload_refs(value);
        if let Ok(encoded) = encode(&message, &limits) {
            decode(&encoded, &limits).expect("encoded accepted message remains decodable");
        }
    }
});
