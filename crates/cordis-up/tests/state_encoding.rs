use cordis_up::{CodecError, Limits, Value, encode_state, encode_value, payload::State};

fn state(data: Value) -> State {
    State {
        schema: "state".into(),
        version: 2,
        data,
    }
}

#[test]
fn borrowed_state_encoding_preserves_existing_bytes_and_persistence_rules() {
    let data = Value::Map(vec![
        (
            "values".into(),
            Value::Array(vec![
                (-0.0f64).into(),
                42u64.into(),
                Value::Bytes(vec![7; 1024]),
            ]),
        ),
        ("tag".into(), Value::Tag(100, Box::new(Value::Null))),
    ]);
    let snapshot = state(data);
    assert_eq!(
        encode_state(&snapshot, &Limits::DEFAULT).unwrap(),
        encode_value(&Value::serialized(&snapshot).unwrap(), &Limits::DEFAULT).unwrap()
    );
    let resource = state(Value::Tag(cordis_up::OBJECT_TAG, Box::new(1u64.into())));
    assert!(encode_state(&resource, &Limits::DEFAULT).is_err());
}

#[test]
fn borrowed_state_encoding_counts_the_same_outer_map_budget() {
    let snapshot = state(Value::Null);
    for (values, collection, accepted) in [(6, 3, false), (7, 2, false), (7, 3, true)] {
        let limits = Limits {
            max_values: values,
            max_collection: collection,
            ..Limits::DEFAULT
        };
        assert_eq!(encode_state(&snapshot, &limits).is_ok(), accepted);
        assert_eq!(
            encode_value(&Value::serialized(&snapshot).unwrap(), &limits).is_ok(),
            accepted
        );
    }
    let mut data = Value::Null;
    for _ in 0..64 {
        data = Value::Array(vec![data]);
    }
    let snapshot = state(data);
    assert!(matches!(
        encode_state(&snapshot, &Limits::DEFAULT),
        Err(CodecError::Limit(_))
    ));
    assert!(encode_value(&Value::serialized(&snapshot).unwrap(), &Limits::DEFAULT).is_err());
}
