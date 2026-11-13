//! ADR-0039：聚合编码事务、部分导入回收及状态编解码的作用域隔离。

#[path = "support/host.rs"]
pub mod host;
#[path = "support/resources.rs"]
mod resources;

use cordis_abi::testing::environment;
use cordis_abi::{CborEncoder, CborValue, decode_in, from_value_in};
use cordis_sdk::{Context, Result, Service};
use resources::{ResourceCounter, tracked};
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeTuple};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct RejectAfterResource<'a>(&'a Service<dyn ResourceCounter>);

impl Serialize for RejectAfterResource<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut tuple = serializer.serialize_tuple(2)?;
        tuple.serialize_element(self.0)?;
        Err(serde::ser::Error::custom("聚合值尾字段拒绝序列化"))
    }
}

struct PlainEncoding<'a>(&'a Service<dyn ResourceCounter>);

impl Serialize for PlainEncoding<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let error = cordis_sdk::encode(self.0).expect_err("状态编码应拒绝服务引用");
        assert!(error.message().contains("服务值需要调用期编码环境"));
        self.0.serialize(serializer)
    }
}

struct PlainDecoding(Service<dyn ResourceCounter>);

impl<'de> Deserialize<'de> for PlainDecoding {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = CborValue::deserialize(deserializer)?;
        let bytes = cordis_sdk::encode(&value).map_err(serde::de::Error::custom)?;
        let error = cordis_sdk::decode::<Service<dyn ResourceCounter>>(&bytes)
            .expect_err("状态解码应拒绝服务引用");
        assert!(error.message().contains("服务值需要调用期解码环境"));
        value
            .deserialized()
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

#[tokio::test]
async fn aggregate_failures_release_only_their_own_resource_references() -> Result<()> {
    let host = Arc::new(host::Loopback::new());
    cordis_abi::testing::install(host.clone())?;
    let id = host.context(cordis_sdk::encode(&())?, Vec::new(), None);
    let context = Context::new(environment(31), id);
    rollback_preserves_committed_values(&host, &context)?;
    partial_decode_releases_imported_fields(&host, &context)?;
    plain_state_calls_isolate_and_restore_resource_scopes(&host, &context)?;
    host.dispose(id).await?;
    assert!(host.leases().values().all(|count| *count == 0));
    Ok(())
}

/// ADR-0039：失败事务立即回收本轮导出，先前成功值仍能跨缓冲调用服务。
fn rollback_preserves_committed_values(host: &host::Loopback, context: &Context) -> Result<()> {
    let kept_drops = Arc::new(AtomicUsize::new(0));
    let failed_drops = Arc::new(AtomicUsize::new(0));
    let kept = tracked(context, 10, kept_drops.clone())?;
    let failed = tracked(context, 20, failed_drops.clone())?;
    let mut encoder = CborEncoder::new(environment(31));
    let committed = encoder.value(&[kept])?;
    assert_eq!(host.resources(), 1);
    let error = encoder.value(&RejectAfterResource(&failed)).unwrap_err();
    assert!(error.message().contains("聚合值尾字段拒绝序列化"));
    drop(failed);
    assert_eq!(failed_drops.load(Ordering::SeqCst), 1);
    assert_eq!(kept_drops.load(Ordering::SeqCst), 0);
    assert_eq!(host.resources(), 1);

    let buffer = encoder.finish(committed)?;
    let restored: Vec<Service<dyn ResourceCounter>> =
        decode_in(buffer.as_slice(), environment(31))?;
    buffer.release()?;
    assert_eq!(restored[0].add(2)?, 12);
    drop(restored);
    assert_eq!(kept_drops.load(Ordering::SeqCst), 1);
    assert_eq!(host.resources(), 0);
    Ok(())
}

/// ADR-0039：tuple 的首字段导入成功、次字段类型错误时归还已取得的租约。
fn partial_decode_releases_imported_fields(host: &host::Loopback, context: &Context) -> Result<()> {
    let released = Arc::new(AtomicUsize::new(0));
    let service = tracked(context, 40, released.clone())?;
    let buffer = CborEncoder::new(environment(31)).encode(&(service, "invalid integer"))?;
    let retained = host.leases();
    let error =
        decode_in::<(Service<dyn ResourceCounter>, u64)>(buffer.as_slice(), environment(31))
            .unwrap_err();
    assert_eq!(error.code(), 2);
    assert_eq!(host.leases(), retained);
    buffer.release()?;
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(host.resources(), 0);
    Ok(())
}

/// ADR-0039：普通状态入口隔离活动作用域，返回后继续使用原调用期环境。
fn plain_state_calls_isolate_and_restore_resource_scopes(
    host: &host::Loopback,
    context: &Context,
) -> Result<()> {
    let released = Arc::new(AtomicUsize::new(0));
    let service = tracked(context, 60, released.clone())?;
    let mut encoder = CborEncoder::new(environment(31));
    let value = encoder.value(&PlainEncoding(&service))?;
    drop(service);
    let restored: PlainDecoding = from_value_in(value, environment(31))?;
    drop(encoder);
    assert_eq!(restored.0.add(3)?, 63);
    drop(restored);
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(host.resources(), 0);
    Ok(())
}
