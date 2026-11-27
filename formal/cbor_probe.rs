//! Independent executions of the production codec, including dependency normalization.
use cordis_up::{Value, OBJECT_TAG};
use cordis_up::{encode_value, decode_value, Limits};

fn equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Float(a), Value::Float(b)) => a.to_bits() == b.to_bits(),
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x,y)| equal(x,y)),
        (Value::Map(a), Value::Map(b)) => a.len() == b.len() && a.iter().zip(b).all(|((k,v),(l,w))| equal(k,l) && equal(v,w)),
        (Value::Tag(a,x), Value::Tag(b,y)) => a == b && equal(x,y),
        _ => left == right,
    }
}

fn roundtrip(value: Value) {
    let bytes = encode_value(&value, &Limits::DEFAULT).expect("encode legal value");
    let decoded = decode_value(&bytes, &Limits::DEFAULT).expect("decode encoded value");
    assert!(equal(&value, &decoded), "roundtrip: {value:?} -> {decoded:?}");
}

fn next(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *seed
}

fn tree(seed: &mut u64, depth: usize) -> Value {
    let random = next(seed);
    match random % if depth == 0 { 7 } else { 10 } {
        0 => Value::Null,
        1 => Value::Bool(random & 256 != 0),
        2 => random.into(),
        3 => Value::Integer((-1_i128 - random as i128).try_into().unwrap()),
        4 => Value::Float(f64::from_bits(random)),
        5 => Value::Bytes(random.to_be_bytes().to_vec()),
        6 => Value::Text(format!("λ中\0{random}")),
        7 => Value::Array((0..3).map(|_| tree(seed, depth-1)).collect()),
        8 => Value::Map((0..3).map(|_| (tree(seed, depth-1), tree(seed, depth-1))).collect()),
        _ => Value::Tag(42, Box::new(tree(seed, depth-1))),
    }
}

fn main() {
    let mut cases = 0_usize;
    let values = [
        Value::Null, Value::Bool(false), Value::Bool(true), 0_u64.into(), u64::MAX.into(),
        Value::Integer((-1_i128 - u64::MAX as i128).try_into().unwrap()),
        Value::Bytes((0..=255).collect()), Value::Text("Unicode 中🙂\0".into()),
        Value::Array(vec![]), Value::Map(vec![]),
        Value::Map(vec![(1_u64.into(), Value::Null),(1_u64.into(), Value::Bool(true))]),
        Value::Tag(42, Box::new(Value::Array(vec![1_u64.into()]))),
        Value::Tag(OBJECT_TAG, Box::new(u64::MAX.into())),
    ];
    for value in values { roundtrip(value); cases += 1; }
    let patterns = [0,1,0x8000000000000000,0x8000000000000001,
        0x7ff0000000000000,0xfff0000000000000,0x7ff8000000000000,0xfff8000000000000,
        0x7ff0000000000001,0x7ff8000000012345,0x3ff0000000000000,0x0010000000000000];
    for bits in patterns { roundtrip(Value::Float(f64::from_bits(bits))); cases += 1; }
    let mut seed = 0xa6f73c25790df128;
    for _ in 0..1024 { roundtrip(Value::Float(f64::from_bits(next(&mut seed)))); cases += 1; }
    for _ in 0..256 { roundtrip(tree(&mut seed, 4)); cases += 1; }
    for value in [Value::Null, 0_u64.into(), (-1_i64).into(), Value::Bytes(vec![1]), Value::Float(1.0)] {
        assert!(encode_value(&Value::Tag(OBJECT_TAG, Box::new(value)), &Limits::DEFAULT).is_err()); cases += 1;
    }
    for value in [
        Value::Tag(OBJECT_TAG, Box::new(1_u64.into())),
        Value::Array(vec![Value::Tag(OBJECT_TAG, Box::new(1_u64.into()))]),
        Value::Map(vec![(Value::Tag(OBJECT_TAG, Box::new(1_u64.into())),Value::Null)]),
    ] {
        assert!(cordis_up::payload::persistent_value(&value).is_err()); cases += 1;
    }
    let collection = Limits { max_collection: 1, ..Limits::DEFAULT };
    assert!(encode_value(&Value::Array(vec![Value::Null, Value::Null]), &collection).is_err()); cases += 1;
    assert!(decode_value(&[0x82,0xf6,0xf6], &collection).is_err()); cases += 1;
    let nodes = Limits { max_values: 2, ..Limits::DEFAULT };
    assert!(encode_value(&Value::Array(vec![Value::Null, Value::Null]), &nodes).is_err()); cases += 1;
    assert!(decode_value(&[0x82,0xf6,0xf6], &nodes).is_err()); cases += 1;
    let depth = Limits { max_depth: 4, ..Limits::DEFAULT };
    let nested = (0..5).fold(Value::Null, |v,_| Value::Array(vec![v]));
    assert!(encode_value(&nested, &depth).is_err()); cases += 1;
    assert!(decode_value(&[0x81,0x81,0x81,0x81,0x81,0xf6], &depth).is_err()); cases += 1;
    let malformed: &[&[u8]] = &[&[], &[0xff], &[0x1c], &[0xf8,0], &[0xfb,0], &[0x61,0xff],
        &[0xf6,0xf6], &[0x9f,0x01], &[0xbf,0x01,0xff], &[0xd9,0xea,0x60,0x00]];
    for bytes in malformed { assert!(decode_value(bytes, &Limits::DEFAULT).is_err(), "accepted malformed {bytes:?}"); cases += 1; }
    let accepted = [
        (vec![0xf7], Value::Null), (vec![0x18,1], 1_u64.into()),
        (vec![0x9f,1,2,0xff], Value::Array(vec![1_u64.into(),2_u64.into()])),
        (vec![0xbf,1,2,0xff], Value::Map(vec![(1_u64.into(),2_u64.into())])),
        (vec![0x5f,0x41,1,0x41,2,0xff], Value::Bytes(vec![1,2])),
        (vec![0x7f,0x61,b'a',0x61,b'b',0xff], Value::Text("ab".into())),
        (vec![0xc2,0x41,1], 1_u64.into()), (vec![0xc3,0x41,0], (-1_i64).into()),
    ];
    for (bytes, expected) in accepted {
        assert!(equal(&decode_value(&bytes, &Limits::DEFAULT).unwrap(), &expected)); cases += 1;
    }
    let mut excessive_negative = vec![0xc3,0x50]; excessive_negative.extend([0xff;16]);
    assert!(decode_value(&excessive_negative, &Limits::DEFAULT).is_err()); cases += 1;
    let mut large_positive = vec![0xc2,0x50]; large_positive.extend([0xff;16]);
    assert!(matches!(decode_value(&large_positive, &Limits::DEFAULT).unwrap(), Value::Tag(2,_))); cases += 1;
    let mut long_tag = vec![0xc2,0x51]; long_tag.extend([0xff;17]);
    assert!(matches!(decode_value(&long_tag, &Limits::DEFAULT).unwrap(), Value::Tag(2,_))); cases += 1;
    let nested_numeric_resource = [0xd9,0xea,0x60,0xc2,0x41,1];
    assert!(equal(&decode_value(&nested_numeric_resource, &Limits::DEFAULT).unwrap(),
        &Value::Tag(OBJECT_TAG, Box::new(1_u64.into())))); cases += 1;
    let mut overflow_map_key = vec![0xa1,0xc3,0x50]; overflow_map_key.extend([0xff;16]); overflow_map_key.push(0xf6);
    assert!(decode_value(&overflow_map_key, &Limits::DEFAULT).is_err()); cases += 1;
    // The dependency preserves an indefinite byte body under tag 2 as a tag.
    let indefinite_big = [0xc2,0x5f,0x41,1,0xff];
    assert!(matches!(decode_value(&indefinite_big, &Limits::DEFAULT).unwrap(), Value::Tag(2,_))); cases += 1;
    // Real Value::from lowering at arbitrary nesting, plus noncanonical leading
    // zero bignums under an ordinary tag; decoding retains mathematical values.
    let large = (1_u128 << 80) + 7;
    let negative = -1_i128 - large as i128;
    let nested = Value::Array(vec![
        u128::MAX.into(),
        Value::Map(vec![(i128::MIN.into(), Value::Tag(42, Box::new(Value::Array(vec![
            Value::Tag(2, Box::new(Value::Bytes(large.to_be_bytes().to_vec()))),
            Value::Tag(3, Box::new(Value::Bytes(large.to_be_bytes().to_vec()))),
        ]))))]),
    ]);
    let expected = Value::Array(vec![
        u128::MAX.into(),
        Value::Map(vec![(i128::MIN.into(), Value::Tag(42, Box::new(Value::Array(vec![
            large.into(), negative.into(),
        ]))))]),
    ]);
    let bytes = encode_value(&nested, &Limits::DEFAULT).unwrap();
    assert!(equal(&decode_value(&bytes, &Limits::DEFAULT).unwrap(), &expected)); cases += 1;
    println!("{{\"compiled_input_cases\":{cases},\"float_cases\":1036,\"recursive_tree_cases\":256}}");
}
