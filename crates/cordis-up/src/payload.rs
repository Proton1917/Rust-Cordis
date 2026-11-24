//! DOC-16 §5–§9、DOC-15 §6.4：核心与执行宿主共用的方法数据。
//! 资源所有权由根refs声明；应用值与持久状态放在对应子字段中。
use crate::{CodecError, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const OBJECT_TAG: u64 = 60000;
pub use cordis_types::Isolation;
pub type Interception = cordis_types::Interception<Value>;
pub type ContextOptions = cordis_types::ContextOptions<Value>;

/// 资源值使用CBOR应用标签，普通业务整数继续按整数编码。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectRef(pub u64);

impl Serialize for ObjectRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ciborium::tag::Required::<_, OBJECT_TAG>(self.0).serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for ObjectRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let reference = ciborium::tag::Required::<u64, OBJECT_TAG>::deserialize(deserializer)?;
        if reference.0 == 0 {
            return Err(serde::de::Error::custom("resource id must be nonzero"));
        }
        Ok(Self(reference.0))
    }
}
impl From<ObjectRef> for Value {
    fn from(value: ObjectRef) -> Self {
        Self::Tag(OBJECT_TAG, Box::new(Self::Integer(value.0.into())))
    }
}

/// 读取协议payload根的持有表，应用DTO位于args、value、config等子字段。
pub fn payload_refs(value: &Value) -> Result<Vec<u64>, CodecError> {
    let Value::Map(fields) = value else {
        return Ok(Vec::new());
    };
    let mut matches = fields
        .iter()
        .filter(|(key, _)| key.as_text() == Some("refs"));
    let Some((_, value)) = matches.next() else {
        return Ok(Vec::new());
    };
    if matches.next().is_some() {
        return Err(CodecError::Schema("duplicate refs field".into()));
    }
    let Value::Array(values) = value else {
        return Err(CodecError::Schema("refs must be an array".into()));
    };
    let mut seen = HashSet::new();
    let mut ids = Vec::with_capacity(values.len());
    for value in values {
        let id = value
            .as_integer()
            .and_then(|id| u64::try_from(id).ok())
            .filter(|id| *id != 0)
            .ok_or_else(|| CodecError::Schema("refs requires nonzero u64 ids".into()))?;
        if !seen.insert(id) {
            return Err(CodecError::Schema("duplicate resource id".into()));
        }
        ids.push(id);
    }
    Ok(ids)
}

/// 状态与诊断保存普通数据，运行时对象在新上下文中通过get取得。
/// DOC-11 §5、DOC-16 §6：状态与诊断以普通数据保存，资源由新上下文重新取得。
pub fn persistent_value(value: &Value) -> Result<(), CodecError> {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Tag(OBJECT_TAG, _) => {
                return Err(CodecError::Schema(
                    "persistent data contains a resource reference".into(),
                ));
            }
            Value::Tag(_, value) => pending.push(value),
            Value::Array(values) => pending.extend(values),
            Value::Map(values) => {
                for (key, value) in values {
                    pending.push(key);
                    pending.push(value);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn state_data<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
    let value = Value::deserialize(deserializer)?;
    persistent_value(&value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

fn serialize_state_data<S: serde::Serializer>(
    value: &Value,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    persistent_value(value).map_err(serde::ser::Error::custom)?;
    value.serialize(serializer)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    pub process: u64,
    pub group: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Describe {
    pub protocol: u32,
    pub plugins: Vec<Plugin>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plugin {
    pub name: String,
    pub version: String,
    pub sdk_version: String,
    pub build_info: String,
    pub format: u8,
    pub library: Option<u64>,
    pub components: Vec<Component>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Component {
    pub name: String,
    pub inject: Vec<Dependency>,
    pub provides: Vec<String>,
    pub listens: Vec<String>,
    pub state_schema: String,
    pub state_version_min: u32,
    pub state_version_max: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dependency {
    pub interface: String,
    pub required: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub schema: String,
    pub version: u32,
    #[serde(
        deserialize_with = "state_data",
        serialize_with = "serialize_state_data"
    )]
    pub data: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Apply<C = Value, S = State> {
    pub plugin: String,
    pub component: String,
    pub library: Option<u64>,
    pub context: u64,
    pub config: C,
    pub inherited: Option<S>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resolve {
    pub plugin: String,
    pub component: String,
    pub library: Option<u64>,
    pub config: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryPrepare {
    pub path: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryPrepared {
    pub library: u64,
    pub plugin: Plugin,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryHandle {
    pub library: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryRetired {
    pub leases: u64,
    pub closed: bool,
    pub quiescence_ms: Option<u64>,
    pub retired_images: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retirement: Option<LibraryRetirement>,
}

/// Retirement history, handle ownership and measured mappings have separate units.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LibraryRetirement {
    pub retired_total: u64,
    /// Library handles returned by the host; mappings are measured separately.
    pub closed_total: u64,
    pub open_retired_handles: u64,
    pub retired_leases: u64,
    pub retired_cache_pins: u64,
    /// Linux /proc/self/maps observation. Absent when unavailable or incomplete.
    pub mapped_retired_images: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Context {
    pub context: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValueResult {
    pub value: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provide {
    pub context: u64,
    pub interface: String,
    pub object: u64,
    pub methods: Vec<String>,
    #[serde(default)]
    pub scoped: bool,
    pub refs: Vec<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Get {
    pub context: u64,
    pub interface: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalObject {
    pub process: u64,
    pub object: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectInfoRequest {
    pub object: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectInfo {
    pub interface: String,
    pub methods: Vec<String>,
    pub local: Option<LocalObject>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetResult {
    pub object: u64,
    pub interface: String,
    pub methods: Vec<String>,
    pub local: Option<LocalObject>,
    pub refs: Vec<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invoke {
    pub object: u64,
    pub method: String,
    pub args: Vec<Value>,
    pub refs: Vec<u64>,
}

impl Invoke {
    /// DOC-16 §6：转移参数值的所有权，保留既有协议字段和顺序。
    pub fn into_value(self) -> Value {
        Value::Map(vec![
            ("object".into(), self.object.into()),
            ("method".into(), Value::Text(self.method)),
            ("args".into(), Value::Array(self.args)),
            ("refs".into(), reference_values(self.refs)),
        ])
    }

    /// 普通协议形状直接转移参数缓冲；其他形状沿用 Serde 的兼容性和错误。
    pub fn from_value_owned(value: Value) -> Result<Self, ciborium::value::Error> {
        let names = ["object", "method", "args", "refs"];
        let indices = plain_fields(&value, &names, true).filter(|indices| {
            let fields = value.as_map().expect("plain map");
            unsigned(&fields[indices[0]].1).is_some()
                && fields[indices[1]].1.as_text().is_some()
                && fields[indices[2]].1.as_array().is_some()
                && plain_references(&fields[indices[3]].1)
        });
        let Some([object, method, args, refs]) = indices else {
            return value.deserialized();
        };
        let mut fields = value.into_map().expect("plain map");
        Ok(Self {
            object: unsigned(&fields[object].1).expect("plain u64"),
            method: take_field(&mut fields, method)
                .into_text()
                .expect("plain text"),
            args: take_field(&mut fields, args)
                .into_array()
                .expect("plain array"),
            refs: take_references(take_field(&mut fields, refs)),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InvokeResult {
    pub value: Value,
    pub refs: Vec<u64>,
}

impl InvokeResult {
    /// DOC-16 §6：转移返回值的所有权，资源提交仍由发送方的守卫负责。
    pub fn into_value(self) -> Value {
        Value::Map(vec![
            ("value".into(), self.value),
            ("refs".into(), reference_values(self.refs)),
        ])
    }

    /// 普通协议形状直接转移返回值缓冲；其他形状沿用 Serde 的兼容性和错误。
    pub fn from_value_owned(value: Value) -> Result<Self, ciborium::value::Error> {
        let indices = plain_fields(&value, &["value", "refs"], false)
            .filter(|indices| plain_references(&value.as_map().expect("plain map")[indices[1]].1));
        let Some([result, refs]) = indices else {
            return value.deserialized();
        };
        let mut fields = value.into_map().expect("plain map");
        Ok(Self {
            value: take_field(&mut fields, result),
            refs: take_references(take_field(&mut fields, refs)),
        })
    }
}

// The fast path covers the ordinary field representation emitted above. Tagged
// fields, extended records and malformed input retain the existing deserializer.
// The negotiated invocation budget is an ignored extension of the Invoke DTO.
fn plain_fields<const N: usize>(
    value: &Value,
    names: &[&str; N],
    invoke_budget: bool,
) -> Option<[usize; N]> {
    let fields = value.as_map()?;
    let mut indices = [usize::MAX; N];
    for (index, (key, value)) in fields.iter().enumerate() {
        let name = key.as_text()?;
        if let Some(field) = names.iter().position(|expected| *expected == name) {
            if indices[field] != usize::MAX {
                return None;
            }
            indices[field] = index;
        } else if invoke_budget && name == "budget_ms" {
            value.deserialized::<serde::de::IgnoredAny>().ok()?;
        } else {
            return None;
        }
    }
    indices
        .iter()
        .all(|index| *index != usize::MAX)
        .then_some(indices)
}

fn unsigned(value: &Value) -> Option<u64> {
    value
        .as_integer()
        .and_then(|integer| u64::try_from(integer).ok())
}

fn plain_references(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|values| values.iter().all(|value| unsigned(value).is_some()))
}

fn take_field(fields: &mut [(Value, Value)], index: usize) -> Value {
    std::mem::replace(&mut fields[index].1, Value::Null)
}

fn reference_values(refs: Vec<u64>) -> Value {
    Value::Array(refs.into_iter().map(Value::from).collect())
}

fn take_references(value: Value) -> Vec<u64> {
    value
        .into_array()
        .expect("plain reference array")
        .into_iter()
        .map(|value| unsigned(&value).expect("plain u64"))
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Effect {
    pub context: u64,
    pub label: String,
    pub body: u64,
    pub asynchronous: bool,
    pub refs: Vec<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Defer {
    pub scope: u64,
    pub callback: u64,
    pub asynchronous: bool,
    pub refs: Vec<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EffectHandle {
    pub effect: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Callback {
    pub object: u64,
    pub context: u64,
    pub scope: u64,
    pub payload: Value,
    pub refs: Vec<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct On {
    pub context: u64,
    pub event: String,
    pub callback: u64,
    pub asynchronous: bool,
    pub refs: Vec<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Emit {
    pub context: u64,
    pub event: String,
    pub payload: Value,
    pub asynchronous: bool,
    pub refs: Vec<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportState {
    pub context: u64,
    pub schema: String,
    pub version: u32,
    pub exporter: u64,
    pub refs: Vec<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Export {
    pub object: u64,
    pub context: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Thaw {
    pub object: u64,
    pub context: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Release {
    pub objects: Vec<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cancel {
    pub id: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Log {
    pub context: u64,
    pub level: u32,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextDerive {
    pub context: u64,
    #[serde(default)]
    pub isolate: Vec<Isolation>,
    #[serde(default)]
    pub intercept: Vec<Interception>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// ADR-0025、DOC-16 §8：返回派生视图编号及其独立资源租约。
pub struct ContextDerived {
    pub context: u64,
    pub object: u64,
    pub refs: Vec<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// DOC-15 §6.4：子组件名称在调用方插件的当前描述符内解析。
pub struct PluginRequest {
    pub context: u64,
    pub component: String,
    pub config: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginResult {
    pub instance: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryStatus {
    pub libraries: Vec<cordis_types::LibraryInfo>,
    pub retired_images: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retirement: Option<LibraryRetirement>,
}

/// DOC-11 SW-9：候选确认后的清理错误携带该次事务的提交事实，运行时据此持久化部署。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplacementFailure {
    pub group: String,
    pub plugin: Option<String>,
    pub committed: bool,
    pub cause: Value,
}

/// DOC-16 §11：转发方分配页面编号，元数据由网页部署解释。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelOpen {
    pub channel: u64,
    pub metadata: Value,
}

/// DOC-16 §11：frame 为完整内层 CBOR 字节串，类型错误按页面连接处理。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelFrame {
    pub channel: u64,
    pub frame: Value,
}

/// DOC-16 §11：按当前物理连接定位页面，关闭后回收其编号与资源。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelClose {
    pub channel: u64,
    pub reason: String,
}
