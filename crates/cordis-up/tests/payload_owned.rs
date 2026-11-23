use cordis_up::{
    ObjectRef, Value,
    payload::{Invoke, InvokeResult},
};
use serde::{Serialize, de::DeserializeOwned};

fn encoded<T: Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(value, &mut bytes).unwrap();
    bytes
}

fn equivalent<T: Serialize + DeserializeOwned>(
    value: Value,
    owned: fn(Value) -> Result<T, ciborium::value::Error>,
) {
    let original = value
        .deserialized::<T>()
        .map(|value| encoded(&value))
        .map_err(|error| error.to_string());
    let actual = owned(value)
        .map(|value| encoded(&value))
        .map_err(|error| error.to_string());
    assert_eq!(actual, original);
}

fn plain_invoke() -> Value {
    Value::serialized(&Invoke {
        object: 9,
        method: "write".into(),
        args: vec![ObjectRef(7).into(), Value::Bytes(vec![1, 2, 3])],
        refs: vec![7],
    })
    .unwrap()
}

fn plain_result() -> Value {
    Value::serialized(&InvokeResult {
        value: Value::Map(vec![("object".into(), ObjectRef(7).into())]),
        refs: vec![7],
    })
    .unwrap()
}

fn examples() -> Vec<Value> {
    vec![
        Value::Null,
        Value::Bool(true),
        Value::from(0_u64),
        Value::from(u64::MAX),
        Value::from(-1_i64),
        Value::from(-1_i128 - u64::MAX as i128),
        Value::Float(0.5),
        Value::Float(f64::NAN),
        Value::Float(f64::INFINITY),
        Value::Text("method".into()),
        Value::Bytes(b"method".to_vec()),
        Value::Array(vec![]),
        Value::Array(vec![Value::from(0_u64), Value::from(u64::MAX)]),
        Value::Array(vec![Value::from(-1_i64)]),
        Value::Map(vec![("nested".into(), Value::Bytes(vec![1, 2, 3]))]),
        Value::Tag(42, Box::new(Value::Text("method".into()))),
        Value::Tag(42, Box::new(Value::Array(vec![]))),
        Value::Tag(42, Box::new(Value::from(9_u64))),
        Value::Tag(2, Box::new(Value::Bytes(vec![9]))),
        Value::Tag(2, Box::new(Value::Bytes(vec![0; 20]))),
        Value::Tag(2, Box::new(Value::Bytes(vec![1; 20]))),
        Value::Tag(3, Box::new(Value::Bytes(vec![9]))),
        Value::Array(vec![Value::Tag(2, Box::new(Value::Bytes(vec![9])))]),
        ObjectRef(7).into(),
    ]
}

fn exercise_shapes<T: Serialize + DeserializeOwned>(
    ordinary: Value,
    owned: fn(Value) -> Result<T, ciborium::value::Error>,
) {
    equivalent(ordinary.clone(), owned);
    for input in examples() {
        equivalent(input, owned);
    }
    for tag in [0, 2, 42, 60000] {
        equivalent(Value::Tag(tag, Box::new(ordinary.clone())), owned);
    }
    let mut reversed = ordinary.clone();
    reversed.as_map_mut().unwrap().reverse();
    equivalent(reversed, owned);

    let fields = ordinary.as_map().unwrap();
    for (index, field) in fields.iter().enumerate() {
        let mut missing = ordinary.clone();
        missing.as_map_mut().unwrap().remove(index);
        equivalent(missing, owned);

        let mut duplicate = ordinary.clone();
        duplicate.as_map_mut().unwrap().push(field.clone());
        equivalent(duplicate, owned);

        let mut tagged_key = ordinary.clone();
        tagged_key.as_map_mut().unwrap()[index].0 = Value::Tag(42, Box::new(field.0.clone()));
        equivalent(tagged_key, owned);

        for replacement in examples() {
            let mut modified = ordinary.clone();
            modified.as_map_mut().unwrap()[index].1 = replacement;
            equivalent(modified, owned);
        }
    }

    // Unknown names and the negotiated budget keep Serde's ignored-field rules.
    for name in ["future_extension", "budget_ms"] {
        for extension in examples() {
            let mut extended = ordinary.clone();
            extended
                .as_map_mut()
                .unwrap()
                .push((name.into(), extension));
            equivalent(extended, owned);
        }
        let mut duplicate_unknown = ordinary.clone();
        duplicate_unknown.as_map_mut().unwrap().extend([
            (name.into(), Value::Null),
            (name.into(), Value::from(10_u64)),
        ]);
        equivalent(duplicate_unknown, owned);
    }
    for key in [Value::from(1_u64), Value::Bytes(b"unknown".to_vec())] {
        let mut extended = ordinary.clone();
        extended.as_map_mut().unwrap().push((key, Value::Null));
        equivalent(extended, owned);
    }
    equivalent(
        Value::Array(fields.iter().map(|(_, value)| value.clone()).collect()),
        owned,
    );
}

#[test]
fn owned_invoke_preserves_serde_acceptance_and_errors() {
    exercise_shapes(plain_invoke(), Invoke::from_value_owned);
}

#[test]
fn owned_result_preserves_serde_acceptance_and_errors() {
    exercise_shapes(plain_result(), InvokeResult::from_value_owned);
}

fn buffer(value: &Value) -> (usize, usize, usize) {
    match value {
        Value::Bytes(bytes) => (bytes.as_ptr() as usize, bytes.len(), bytes.capacity()),
        Value::Text(text) => (text.as_ptr() as usize, text.len(), text.capacity()),
        _ => panic!("expected a byte or text buffer"),
    }
}

#[test]
fn owned_invoke_preserves_wire_bytes_and_payload_buffers() {
    let mut bytes = Vec::with_capacity(1024 * 1024 + 137);
    bytes.resize(1024 * 1024, 73);
    let mut text = String::with_capacity(256);
    text.push_str("text argument");
    let mut method = String::with_capacity(128);
    method.push_str("write");
    let request = Invoke {
        object: 9,
        method,
        args: vec![Value::Bytes(bytes), Value::Text(text), ObjectRef(7).into()],
        refs: vec![7],
    };
    let expected_bytes = encoded(&Value::serialized(&request).unwrap());
    let method_buffer = (
        request.method.as_ptr() as usize,
        request.method.len(),
        request.method.capacity(),
    );
    let args_buffer = (
        request.args.as_ptr() as usize,
        request.args.len(),
        request.args.capacity(),
    );
    let bytes_buffer = buffer(&request.args[0]);
    let text_buffer = buffer(&request.args[1]);

    let mut value = request.into_value();
    assert_eq!(encoded(&value), expected_bytes);
    value
        .as_map_mut()
        .unwrap()
        .insert(0, ("budget_ms".into(), Value::from(1000_u64)));
    let decoded = Invoke::from_value_owned(value).unwrap();
    assert_eq!(
        (
            decoded.method.as_ptr() as usize,
            decoded.method.len(),
            decoded.method.capacity()
        ),
        method_buffer
    );
    assert_eq!(
        (
            decoded.args.as_ptr() as usize,
            decoded.args.len(),
            decoded.args.capacity()
        ),
        args_buffer
    );
    assert_eq!(buffer(&decoded.args[0]), bytes_buffer);
    assert_eq!(buffer(&decoded.args[1]), text_buffer);
    assert_eq!(decoded.refs, [7]);
}

#[test]
fn owned_result_preserves_wire_bytes_and_nested_payload_buffer() {
    let mut bytes = Vec::with_capacity(1024 * 1024 + 251);
    bytes.resize(1024 * 1024, 91);
    let signature = (bytes.as_ptr() as usize, bytes.len(), bytes.capacity());
    let response = InvokeResult {
        value: Value::Tag(42, Box::new(Value::Array(vec![Value::Bytes(bytes)]))),
        refs: vec![],
    };
    let expected_bytes = encoded(&Value::serialized(&response).unwrap());
    let value = response.into_value();
    assert_eq!(encoded(&value), expected_bytes);
    let result = InvokeResult::from_value_owned(value).unwrap();
    let Value::Tag(42, value) = result.value else {
        panic!("result tag changed");
    };
    assert_eq!(buffer(&value.as_array().unwrap()[0]), signature);
    assert!(result.refs.is_empty());
}

#[test]
fn owned_encoding_preserves_nested_cbor_values() {
    for value in examples() {
        let request = Invoke {
            object: u64::MAX,
            method: "nested".into(),
            args: vec![value.clone()],
            refs: vec![0, 9, 9, u64::MAX],
        };
        let expected = encoded(&Value::serialized(&request).unwrap());
        assert_eq!(encoded(&request.into_value()), expected);

        let response = InvokeResult {
            value,
            refs: vec![0, 9, 9, u64::MAX],
        };
        let expected = encoded(&Value::serialized(&response).unwrap());
        assert_eq!(encoded(&response.into_value()), expected);
    }
}

// CBOR writers may canonicalize NaNs, so byte equality alone cannot establish
// that the in-memory Value representation, including its map keys, is unchanged.
fn assert_same_value(actual: &Value, expected: &Value) {
    match (actual, expected) {
        (Value::Float(actual), Value::Float(expected)) => {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
        (Value::Array(actual), Value::Array(expected)) => {
            assert_eq!(actual.len(), expected.len());
            for (actual, expected) in actual.iter().zip(expected) {
                assert_same_value(actual, expected);
            }
        }
        (Value::Map(actual), Value::Map(expected)) => {
            assert_eq!(actual.len(), expected.len());
            for ((actual_key, actual), (expected_key, expected)) in actual.iter().zip(expected) {
                assert_same_value(actual_key, expected_key);
                assert_same_value(actual, expected);
            }
        }
        (Value::Tag(actual_tag, actual), Value::Tag(expected_tag, expected)) => {
            assert_eq!(actual_tag, expected_tag);
            assert_same_value(actual, expected);
        }
        _ => assert_eq!(actual, expected),
    }
}

#[test]
fn owned_conversion_preserves_nested_value_representation() {
    let mut values = examples();
    values.extend([
        Value::Tag(2, Box::new(Value::Null)),
        Value::Tag(3, Box::new(Value::Text("opaque big integer tag".into()))),
        Value::Tag(2, Box::new(Value::Bytes(vec![0, 0, 9]))),
        Value::Tag(3, Box::new(Value::Bytes(vec![0xff; 20]))),
    ]);
    values.extend(
        [
            0x8000_0000_0000_0000,
            0x0000_0000_0000_0001,
            0x7ff0_0000_0000_0001,
            0x7ff8_0000_0000_0042,
            0xfff0_0000_0000_0001,
            0xfff8_0000_0000_00ab,
        ]
        .into_iter()
        .map(|bits| Value::Float(f64::from_bits(bits))),
    );
    // Keep tags and arbitrary/duplicate map keys inside otherwise ordinary DTOs
    // so both owned decoders exercise their fast path for every nested value.
    let nested = Value::Tag(
        42,
        Box::new(Value::Map(
            values
                .iter()
                .flat_map(|value| {
                    [
                        (value.clone(), Value::Array(vec![value.clone()])),
                        (value.clone(), Value::Tag(43, Box::new(value.clone()))),
                    ]
                })
                .collect(),
        )),
    );
    values.push(nested);

    for value in values {
        let request = Invoke {
            object: 9,
            method: "inspect".into(),
            args: vec![value.clone()],
            refs: vec![7],
        };
        let expected_encoding = Value::serialized(&request).unwrap();
        let input = request.into_value();
        assert_same_value(&input, &expected_encoding);
        let expected: Invoke = input.deserialized().unwrap();
        let actual = Invoke::from_value_owned(input).unwrap();
        assert_eq!(actual.object, expected.object);
        assert_eq!(actual.method, expected.method);
        assert_eq!(actual.refs, expected.refs);
        assert_same_value(&Value::Array(actual.args), &Value::Array(expected.args));

        let response = InvokeResult {
            value,
            refs: vec![7],
        };
        let expected_encoding = Value::serialized(&response).unwrap();
        let input = response.into_value();
        assert_same_value(&input, &expected_encoding);
        let expected: InvokeResult = input.deserialized().unwrap();
        let actual = InvokeResult::from_value_owned(input).unwrap();
        assert_eq!(actual.refs, expected.refs);
        assert_same_value(&actual.value, &expected.value);
    }
}
