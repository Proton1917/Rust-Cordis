use cordis_up::{
    Limits, Message, ObjectRef, Value, WireError, encode,
    payload::{Emit, Invoke, InvokeResult},
};
use std::{fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/fuzz/corpus/up1_decoder");
    fs::create_dir_all(&directory)?;
    let limits = Limits::default();
    let invoke = Value::serialized(&Invoke {
        object: 9,
        method: "get".into(),
        args: vec!["key".into()],
        refs: Vec::new(),
    })?;
    let resource = Value::serialized(&Invoke {
        object: 9,
        method: "retain".into(),
        args: vec![ObjectRef(7).into(), 7u64.into()],
        refs: vec![7],
    })?;
    let result = Value::serialized(&InvokeResult {
        value: ObjectRef(7).into(),
        refs: vec![7],
    })?;
    let event = Value::serialized(&Emit {
        context: 1,
        event: "changed@1.0".into(),
        payload: "ready".into(),
        asynchronous: true,
        refs: Vec::new(),
    })?;
    let nested = (0..limits.max_depth - 2).fold(Value::Null, |value, _| Value::Array(vec![value]));
    let row = Value::Array(vec![Value::Null; 1023]);
    let mut values = vec![row; 255];
    values.push(Value::Array(vec![Value::Null; 1022]));
    let samples = [
        (
            "request",
            Message::Request {
                id: 1,
                method: "invoke".into(),
                params: invoke,
            },
        ),
        (
            "response",
            Message::Response {
                id: 1,
                result: Ok(Value::Null),
            },
        ),
        (
            "notification",
            Message::Notification {
                method: "emit".into(),
                params: event,
            },
        ),
        (
            "resource-input",
            Message::Request {
                id: 2,
                method: "invoke".into(),
                params: resource,
            },
        ),
        (
            "resource-result",
            Message::Response {
                id: 2,
                result: Ok(result),
            },
        ),
        (
            "error-detail",
            Message::Response {
                id: 3,
                result: Err(WireError {
                    code: 1001,
                    message: "contract failure".into(),
                    detail: Value::Map(vec![("field".into(), "counter".into())]),
                }),
            },
        ),
        (
            "depth-boundary",
            Message::Request {
                id: 4,
                method: "depth".into(),
                params: nested,
            },
        ),
        (
            "collection-boundary",
            Message::Request {
                id: 5,
                method: "collection".into(),
                params: Value::Array(vec![Value::Null; limits.max_collection]),
            },
        ),
        (
            "value-budget-boundary",
            Message::Request {
                id: 6,
                method: "values".into(),
                params: Value::Array(values),
            },
        ),
    ];
    for (name, message) in samples {
        let bytes = encode(&message, &limits)?;
        fs::write(directory.join(name), &bytes)?;
        println!("{name}: {} bytes", bytes.len());
    }
    Ok(())
}
