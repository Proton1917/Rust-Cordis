//! CP-2、CP-8、ADR-0021：通过Serde访问器实现CBOR输入预算。
//! 同步解码的预算由作用域守卫恢复；错误保留解析、额度和尾随数据类别。
use crate::{Message, OBJECT_TAG, Value};
use serde::de::{self, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor};
use std::{
    cell::Cell,
    fmt,
    io::{Cursor, Write},
};

/// DOC-16 §3：单次值图的预算；帧上限同时约束输入与编码输出。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_frame: usize,
    pub max_depth: usize,
    pub max_collection: usize,
    pub max_values: usize,
}

impl Limits {
    pub const DEFAULT: Self = Self {
        max_frame: 16 * 1024 * 1024,
        max_depth: 64,
        max_collection: 65_536,
        max_values: 262_144,
    };

    fn validate(self) -> Result<(), CodecError> {
        if self.max_frame == 0 || self.max_frame > u32::MAX as usize {
            return Err(CodecError::Limit("max_frame"));
        }
        if self.max_depth < 4 || self.max_depth > 128 {
            return Err(CodecError::Limit("max_depth"));
        }
        if self.max_values == 0 {
            return Err(CodecError::Limit("max_values"));
        }
        Ok(())
    }
}
impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("UP/1 limit exceeded: {0}")]
    Limit(&'static str),
    #[error("invalid CBOR: {0}")]
    Cbor(String),
    #[error("invalid UP/1 message: {0}")]
    Schema(String),
    #[error("trailing bytes after UP/1 message")]
    TrailingBytes,
}

thread_local! {
    static DECODE_LIMITS: Cell<Limits> = const { Cell::new(Limits::DEFAULT) };
}

/// CP-8：错误返回和展开均恢复此前预算，隔离同线程的后续解码。
struct DecodeScope(Limits);
impl DecodeScope {
    fn enter(limits: Limits) -> Self {
        Self(DECODE_LIMITS.with(|current| current.replace(limits)))
    }
}
impl Drop for DecodeScope {
    fn drop(&mut self) {
        DECODE_LIMITS.with(|current| current.set(self.0));
    }
}

/// CP-2、CP-8：输入为一条完整CBOR消息体，帧头由传输层处理。
pub fn decode(bytes: &[u8], limits: &Limits) -> Result<Message, CodecError> {
    limits.validate()?;
    if bytes.len() > limits.max_frame {
        return Err(CodecError::Limit("max_frame"));
    }
    let _scope = DecodeScope::enter(*limits);
    let mut reader = Cursor::new(bytes);
    let message: Message =
        ciborium::de::from_reader_with_recursion_limit(&mut reader, limits.max_depth)
            .map_err(|error| CodecError::Cbor(error.to_string()))?;
    if reader.position() as usize != bytes.len() {
        return Err(CodecError::TrailingBytes);
    }
    envelope(&message)?;
    Ok(message)
}

/// 输出CBOR消息体，写入时执行帧长度限制。
pub fn encode(message: &Message, limits: &Limits) -> Result<Vec<u8>, CodecError> {
    limits.validate()?;
    envelope(message)?;
    let (value, overhead) = match message {
        Message::Request { params, .. } | Message::Notification { params, .. } => (params, 2),
        Message::Response {
            result: Ok(value), ..
        } => (value, 3),
        Message::Response {
            result: Err(error), ..
        } => (&error.detail, 4),
    };
    validate_value(value, limits, overhead)?;
    let mut writer = LimitedWriter {
        bytes: Vec::new(),
        limit: limits.max_frame,
    };
    ciborium::ser::into_writer(message, &mut writer)
        .map_err(|error| CodecError::Cbor(error.to_string()))?;
    Ok(writer.bytes)
}

/// CP-8：持久状态和接口值复用同一集合、深度与节点预算。
pub fn decode_value(bytes: &[u8], limits: &Limits) -> Result<Value, CodecError> {
    #[derive(serde::Deserialize)]
    struct LimitedValue(#[serde(deserialize_with = "standalone_value")] Value);
    limits.validate()?;
    if bytes.len() > limits.max_frame {
        return Err(CodecError::Limit("max_frame"));
    }
    let _scope = DecodeScope::enter(*limits);
    let mut reader = Cursor::new(bytes);
    let value: LimitedValue =
        ciborium::de::from_reader_with_recursion_limit(&mut reader, limits.max_depth)
            .map_err(|problem| CodecError::Cbor(problem.to_string()))?;
    if reader.position() as usize != bytes.len() {
        return Err(CodecError::TrailingBytes);
    }
    Ok(value.0)
}

/// CP-8、DOC-16 §2：编码独立状态值，超限在持久化之前返回错误。
pub fn encode_value(value: &Value, limits: &Limits) -> Result<Vec<u8>, CodecError> {
    limits.validate()?;
    validate_value(value, limits, 0)?;
    let mut writer = LimitedWriter {
        bytes: Vec::new(),
        limit: limits.max_frame,
    };
    ciborium::ser::into_writer(value, &mut writer)
        .map_err(|problem| CodecError::Cbor(problem.to_string()))?;
    Ok(writer.bytes)
}

fn standalone_value<'de, D: de::Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
    bounded(deserializer, 0)
}

/// DOC-16 §4：验证消息编号、方法名与错误码的基本协议形状。
fn envelope(message: &Message) -> Result<(), CodecError> {
    match message {
        Message::Request { id: 0, .. } | Message::Response { id: 0, .. } => {
            Err(CodecError::Schema("request id must be nonzero".into()))
        }
        Message::Request { method, .. } | Message::Notification { method, .. }
            if method.is_empty() =>
        {
            Err(CodecError::Schema("method must be nonempty".into()))
        }
        Message::Response {
            result: Err(error), ..
        } if error.code == 0 => Err(CodecError::Schema("error code must be nonzero".into())),
        Message::Response {
            result: Err(error), ..
        } => crate::payload::persistent_value(&error.detail),
        _ => Ok(()),
    }
}

/// CP-8：编码前按实际值图计量；overhead计入所属消息包络的嵌套层级。
fn validate_value(value: &Value, limits: &Limits, overhead: usize) -> Result<(), CodecError> {
    let mut pending = vec![(value, overhead)];
    let mut count = 0;
    while let Some((value, depth)) = pending.pop() {
        count += 1;
        if count > limits.max_values {
            return Err(CodecError::Limit("max_values"));
        }
        match value {
            Value::Array(items) => {
                if depth >= limits.max_depth {
                    return Err(CodecError::Limit("max_depth"));
                }
                if items.len() > limits.max_collection {
                    return Err(CodecError::Limit("max_collection"));
                }
                pending.extend(items.iter().map(|value| (value, depth + 1)));
            }
            Value::Map(items) => {
                if depth >= limits.max_depth {
                    return Err(CodecError::Limit("max_depth"));
                }
                if items.len() > limits.max_collection {
                    return Err(CodecError::Limit("max_collection"));
                }
                for (key, value) in items {
                    pending.push((key, depth + 1));
                    pending.push((value, depth + 1));
                }
            }
            Value::Tag(tag, value) => {
                if depth >= limits.max_depth {
                    return Err(CodecError::Limit("max_depth"));
                }
                resource_tag(*tag, value).map_err(CodecError::Schema)?;
                pending.push((value, depth + 1));
            }
            _ => {}
        }
    }
    Ok(())
}

/// CP-2：序列化写入过程中执行字节上限，避免先产生完整超额缓冲。
struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "max_frame",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn params<'de, D: de::Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
    bounded(deserializer, 2)
}
pub(crate) fn result<'de, D: de::Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
    bounded(deserializer, 3)
}
pub(crate) fn detail<'de, D: de::Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
    bounded(deserializer, 4)
}

/// CP-8：根值建立共享节点预算，子访问器沿同一预算递减。
fn bounded<'de, D: de::Deserializer<'de>>(
    deserializer: D,
    depth: usize,
) -> Result<Value, D::Error> {
    let limits = DECODE_LIMITS.with(Cell::get);
    let mut budget = Budget {
        limits,
        remaining: limits.max_values,
    };
    Node {
        budget: &mut budget,
        depth,
        allowed: true,
    }
    .deserialize(deserializer)
}

struct Budget {
    limits: Limits,
    remaining: usize,
}
impl Budget {
    fn node<E: de::Error>(&mut self) -> Result<(), E> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or_else(|| E::custom("max_values"))?;
        Ok(())
    }
    fn container<E: de::Error>(&self, depth: usize) -> Result<(), E> {
        if depth >= self.limits.max_depth {
            Err(E::custom("max_depth"))
        } else {
            Ok(())
        }
    }
}

/// CP-8：每个子值进入解码前消费预算，集合扩展先判定其许可。
struct Node<'a> {
    budget: &'a mut Budget,
    depth: usize,
    allowed: bool,
}
impl<'de> DeserializeSeed<'de> for Node<'_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        if !self.allowed {
            return Err(de::Error::custom("max_collection"));
        }
        self.budget.node()?;
        deserializer.deserialize_any(ValueVisitor {
            budget: self.budget,
            depth: self.depth,
        })
    }
}

struct ValueVisitor<'a> {
    budget: &'a mut Budget,
    depth: usize,
}
impl ValueVisitor<'_> {
    fn number<E: de::Error>(self, value: Value) -> Result<Value, E> {
        if matches!(value, Value::Tag(..)) {
            self.budget.container(self.depth)?;
            self.budget.node()?;
        }
        Ok(value)
    }
}

impl<'de> Visitor<'de> for ValueVisitor<'_> {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded CBOR value")
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(value.into())
    }
    fn visit_u128<E: de::Error>(self, value: u128) -> Result<Value, E> {
        self.number(value.into())
    }
    fn visit_i128<E: de::Error>(self, value: i128) -> Result<Value, E> {
        self.number(value.into())
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Ok(Value::Float(value))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::Text(value.to_owned()))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::Text(value))
    }
    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Value, E> {
        Ok(Value::Bytes(value.to_vec()))
    }
    fn visit_byte_buf<E: de::Error>(self, value: Vec<u8>) -> Result<Value, E> {
        Ok(Value::Bytes(value))
    }
    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_some<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        self.budget.container(self.depth)?;
        let maximum = self.budget.limits.max_collection;
        if access.size_hint().is_some_and(|length| length > maximum) {
            return Err(de::Error::custom("max_collection"));
        }
        let mut items = Vec::new();
        while let Some(value) = access.next_element_seed(Node {
            budget: &mut *self.budget,
            depth: self.depth + 1,
            allowed: items.len() < maximum,
        })? {
            items.push(value);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        self.budget.container(self.depth)?;
        let maximum = self.budget.limits.max_collection;
        if access.size_hint().is_some_and(|length| length > maximum) {
            return Err(de::Error::custom("max_collection"));
        }
        let mut items = Vec::new();
        while let Some(key) = access.next_key_seed(Node {
            budget: &mut *self.budget,
            depth: self.depth + 1,
            allowed: items.len() < maximum,
        })? {
            let value = access.next_value_seed(Node {
                budget: &mut *self.budget,
                depth: self.depth + 1,
                allowed: true,
            })?;
            items.push((key, value));
        }
        Ok(Value::Map(items))
    }

    fn visit_enum<A: EnumAccess<'de>>(self, access: A) -> Result<Value, A::Error> {
        self.budget.container(self.depth)?;
        let (name, variant) = access.variant::<String>()?;
        if name != "@@TAGGED@@" {
            return Err(de::Error::custom("expected CBOR tag"));
        }
        variant.tuple_variant(
            2,
            Tagged {
                budget: self.budget,
                depth: self.depth,
            },
        )
    }
}

struct Tagged<'a> {
    budget: &'a mut Budget,
    depth: usize,
}
impl<'de> Visitor<'de> for Tagged<'_> {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CBOR tag and value")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let tag = access
            .next_element::<u64>()?
            .ok_or_else(|| de::Error::custom("missing tag"))?;
        let value = access
            .next_element_seed(Node {
                budget: self.budget,
                depth: self.depth + 1,
                allowed: true,
            })?
            .ok_or_else(|| de::Error::custom("missing tagged value"))?;
        resource_tag(tag, &value).map_err(de::Error::custom)?;
        Ok(Value::Tag(tag, Box::new(value)))
    }
}

/// DOC-16 §6：资源标签的载荷为非零编号；普通CBOR标签保留原语义。
fn resource_tag(tag: u64, value: &Value) -> Result<(), String> {
    if tag == OBJECT_TAG
        && value
            .as_integer()
            .and_then(|id| u64::try_from(id).ok())
            .is_none_or(|id| id == 0)
    {
        Err("resource tag requires a nonzero u64".into())
    } else {
        Ok(())
    }
}
