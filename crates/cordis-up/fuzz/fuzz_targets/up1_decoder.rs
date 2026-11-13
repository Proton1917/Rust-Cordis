#![no_main]

use cordis_up::{Limits, Message, Value, decode, encode, payload, payload_refs};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    if let Ok(message) = decode(data, &limits) {
        let value = match &message {
            Message::Request { params, .. } | Message::Notification { params, .. } => params,
            Message::Response {
                result: Ok(value), ..
            } => value,
            Message::Response {
                result: Err(error), ..
            } => &error.detail,
        };
        let _ = payload_refs(value);
        if let Message::Request { method, params, .. } | Message::Notification { method, params } =
            &message
        {
            match method.as_str() {
                "channel_open" => {
                    let _ = params.deserialized::<payload::ChannelOpen>();
                }
                "channel_frame" => {
                    if let Ok(frame) = params.deserialized::<payload::ChannelFrame>()
                        && let Value::Bytes(bytes) = frame.frame
                    {
                        let _ = decode(&bytes, &limits);
                    }
                }
                "channel_close" => {
                    let _ = params.deserialized::<payload::ChannelClose>();
                }
                _ => {}
            }
        }
        if let Ok(encoded) = encode(&message, &limits) {
            decode(&encoded, &limits).expect("encoded accepted message remains decodable");
        }
    }
});
