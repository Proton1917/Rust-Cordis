use cordis_up::{
    CodecError, Limits, Message, ObjectRef, Value, WireError, decode, encode,
    payload::{Invoke, State},
    payload_refs,
};

fn request(params: Value) -> Message {
    Message::Request {
        id: 1,
        method: "invoke".into(),
        params,
    }
}
fn raw(message: &Message) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(message, &mut bytes).unwrap();
    bytes
}
fn nested(depth: usize) -> Value {
    (0..depth).fold(Value::Null, |value, _| Value::Array(vec![value]))
}

// UP/1 envelope与资源标签保持跨语言可解释的CBOR结构。
#[test]
fn envelope_and_resource_tags_survive_codec() {
    let params = Value::serialized(&Invoke {
        object: 9,
        method: "write".into(),
        args: vec![ObjectRef(7).into(), 7u64.into()],
        refs: vec![7],
    })
    .unwrap();
    let bytes = encode(&request(params), &Limits::default()).unwrap();
    let wire: Value = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    let outer = wire.as_map().unwrap();
    assert_eq!(outer.len(), 1);
    assert_eq!(outer[0].0.as_text(), Some("request"));
    let Message::Request { id, method, params } = decode(&bytes, &Limits::default()).unwrap()
    else {
        panic!("request envelope changed")
    };
    let invoke: Invoke = params.deserialized().unwrap();
    assert_eq!(id, 1);
    assert_eq!(method, "invoke");
    assert_eq!(invoke.object, 9);
    assert_eq!(invoke.args[0], Value::Tag(60000, Box::new(7u64.into())));
    assert_eq!(invoke.args[1], Value::Integer(7u64.into()));
    assert_eq!(payload_refs(&params).unwrap(), [7]);
    let response = Message::Response {
        id: 1,
        result: Ok(Value::Null),
    };
    assert!(matches!(
        decode(
            &encode(&response, &Limits::default()).unwrap(),
            &Limits::default()
        )
        .unwrap(),
        Message::Response {
            result: Ok(Value::Null),
            ..
        }
    ));
}

// CP-8：明确长度和不定长集合都在增长边界执行同一上限。
#[test]
fn collections_and_declared_lengths_are_bounded() {
    let limits = Limits {
        max_collection: 4,
        ..Limits::default()
    };
    let accepted = request(Value::Array(vec![Value::Null; 4]));
    assert!(decode(&raw(&accepted), &limits).is_ok());
    assert!(decode(&raw(&request(Value::Array(vec![Value::Null; 5]))), &limits).is_err());
    for header in [0x9b, 0xbb] {
        let mut bytes = raw(&request(Value::Null));
        assert_eq!(bytes.pop(), Some(0xf6));
        bytes.extend_from_slice(&[header, 255, 255, 255, 255, 255, 255, 255, 255]);
        assert!(decode(&bytes, &limits).is_err());
    }
    let mut bytes = raw(&request(Value::Null));
    assert_eq!(bytes.pop(), Some(0xf6));
    bytes.extend_from_slice(&[0x9f, 0, 1, 2, 3, 4, 0xff]);
    assert!(decode(&bytes, &limits).is_err());
}

// CP-8：包络容器计入深度，多个各自合法的集合共同消耗节点预算。
#[test]
fn nesting_and_aggregate_values_are_bounded() {
    let limits = Limits {
        max_depth: 8,
        ..Limits::default()
    };
    assert!(decode(&raw(&request(nested(6))), &limits).is_ok());
    assert!(decode(&raw(&request(nested(7))), &limits).is_err());
    assert!(encode(&request(nested(7)), &limits).is_err());
    let limits = Limits {
        max_values: 20,
        ..Limits::default()
    };
    let row = Value::Array(vec![Value::Null; 3]);
    assert!(decode(&raw(&request(Value::Array(vec![row.clone(); 4]))), &limits).is_ok());
    assert!(decode(&raw(&request(Value::Array(vec![row; 5]))), &limits).is_err());
}

// CP-2：消息体长度按实际字节计，完整消息后的附加值也属于协议错误。
#[test]
fn frame_body_limit_truncation_and_trailing_data_are_rejected() {
    let message = request(Value::Bytes(vec![4; 128]));
    let bytes = raw(&message);
    let limits = Limits {
        max_frame: bytes.len(),
        ..Limits::default()
    };
    assert!(decode(&bytes, &limits).is_ok());
    let smaller = Limits {
        max_frame: bytes.len() - 1,
        ..limits
    };
    assert!(matches!(
        decode(&bytes, &smaller),
        Err(CodecError::Limit("max_frame"))
    ));
    assert!(encode(&message, &smaller).is_err());
    assert!(decode(&bytes[..bytes.len() - 1], &Limits::default()).is_err());
    let mut trailing = bytes;
    trailing.push(0xf6);
    assert!(matches!(
        decode(&trailing, &Limits::default()),
        Err(CodecError::TrailingBytes)
    ));
}

// 请求编号、错误代码及资源值采用协议指定的非零整数。
#[test]
fn invalid_envelopes_and_resource_values_are_rejected() {
    let invalid = [
        Message::Request {
            id: 0,
            method: "invoke".into(),
            params: Value::Null,
        },
        Message::Notification {
            method: String::new(),
            params: Value::Null,
        },
        Message::Response {
            id: 1,
            result: Err(WireError::new(0, "invalid error")),
        },
        request(Value::Tag(60000, Box::new(Value::Text("pointer".into())))),
        request(Value::Tag(60000, Box::new(0u64.into()))),
    ];
    for message in invalid {
        assert!(encode(&message, &Limits::default()).is_err());
        assert!(decode(&raw(&message), &Limits::default()).is_err());
    }
}

// refs属于协议根；普通DTO的同名属性保持业务数据，重复出口编号按协议错误处理。
#[test]
fn payload_reference_list_is_root_scoped_and_unique() {
    let params = Value::Map(vec![
        (
            "value".into(),
            Value::Map(vec![("refs".into(), Value::Array(vec![99u64.into()]))]),
        ),
        ("refs".into(), Value::Array(vec![7u64.into()])),
    ]);
    assert_eq!(payload_refs(&params).unwrap(), [7]);
    let duplicate = Value::Map(vec![(
        "refs".into(),
        Value::Array(vec![7u64.into(), 7u64.into()]),
    )]);
    assert!(matches!(
        payload_refs(&duplicate),
        Err(CodecError::Schema(_))
    ));
    let duplicate_field = Value::Map(vec![
        ("refs".into(), Value::Array(Vec::new())),
        ("refs".into(), Value::Array(Vec::new())),
    ]);
    assert!(matches!(
        payload_refs(&duplicate_field),
        Err(CodecError::Schema(_))
    ));
}

// 诊断与持久状态承载普通值，资源引用通过正常调用结果转交。
#[test]
fn error_details_and_state_reject_resource_tags() {
    let message = Message::Response {
        id: 1,
        result: Err(WireError {
            code: 2,
            message: "invalid argument".into(),
            detail: Value::Array(vec![ObjectRef(7).into()]),
        }),
    };
    assert!(matches!(
        encode(&message, &Limits::default()),
        Err(CodecError::Schema(_))
    ));
    assert!(matches!(
        decode(&raw(&message), &Limits::default()),
        Err(CodecError::Schema(_))
    ));
    let state = State {
        schema: "counter".into(),
        version: 1,
        data: ObjectRef(7).into(),
    };
    assert!(Value::serialized(&state).is_err());
    let raw_state = Value::Map(vec![
        ("schema".into(), "counter".into()),
        ("version".into(), 1u64.into()),
        ("data".into(), ObjectRef(7).into()),
    ]);
    assert!(raw_state.deserialized::<State>().is_err());
}

// 同步解码预算在错误返回后恢复，普通Serde转换继续使用默认预算。
#[test]
fn decoding_budget_scope_restores_after_failure() {
    let bytes = raw(&request(Value::Array(vec![Value::Null; 5])));
    let tight = Limits {
        max_collection: 4,
        ..Limits::default()
    };
    assert!(decode(&bytes, &tight).is_err());
    let plain: Message = ciborium::de::from_reader(bytes.as_slice()).unwrap();
    assert!(
        matches!(plain, Message::Request { params: Value::Array(values), .. } if values.len() == 5)
    );
}
