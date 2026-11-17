//! ADR-0056：同名同版本的接口声明布局不同时，使用方不得按槽位调用提供方。

#[allow(dead_code)]
#[path = "support/host.rs"]
mod host;

use cordis_abi::testing::environment;
use cordis_sdk::prelude::*;

/// 提供方编译时的接口声明。
#[cordis_sdk::interface(name = "test.layout", version = "1.0")]
trait Provided: Send + Sync {
    fn label(&self, text: String) -> Result<String>;
    fn double(&self, value: u64) -> Result<u64>;
}

/// 使用方编译时的接口声明：名称与版本相同，方法顺序相反。
#[cordis_sdk::interface(name = "test.layout", version = "1.0")]
trait Reordered: Send + Sync {
    fn double(&self, value: u64) -> Result<u64>;
    fn label(&self, text: String) -> Result<String>;
}

/// 只改变一个原始类型参数。
#[cordis_sdk::interface(name = "test.layout", version = "1.0")]
trait Narrowed: Send + Sync {
    fn label(&self, text: String) -> Result<String>;
    fn double(&self, value: u32) -> Result<u64>;
}

/// 只把一个方法改为异步。
#[cordis_sdk::interface(name = "test.layout", version = "1.0")]
trait Asynchronous: Send + Sync {
    fn label(&self, text: String) -> Result<String>;
    async fn double(&self, value: u64) -> Result<u64>;
}

/// 与 Provided 完全相同的声明，模拟使用方用同一份接口编译。
#[cordis_sdk::interface(name = "test.layout", version = "1.0")]
trait Same: Send + Sync {
    fn label(&self, text: String) -> Result<String>;
    fn double(&self, value: u64) -> Result<u64>;
}

struct Implementation;
impl Provided for Implementation {
    fn label(&self, text: String) -> Result<String> {
        Ok(format!("{text}!"))
    }
    fn double(&self, value: u64) -> Result<u64> {
        Ok(value * 2)
    }
}

#[test]
fn digest_tracks_slot_layout() {
    let provided = <dyn Provided as Interface>::LAYOUT_DIGEST;
    assert_ne!(provided, 0);
    assert_eq!(provided, <dyn Same as Interface>::LAYOUT_DIGEST);
    for (name, other) in [
        ("方法顺序", <dyn Reordered as Interface>::LAYOUT_DIGEST),
        ("原始类型", <dyn Narrowed as Interface>::LAYOUT_DIGEST),
        ("同步与异步", <dyn Asynchronous as Interface>::LAYOUT_DIGEST),
    ] {
        assert_ne!(provided, other, "{name}改变时摘要应改变");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mismatched_layout_uses_named_invocation() -> Result<()> {
    let host = std::sync::Arc::new(host::Loopback::new());
    cordis_abi::testing::install(host)?;
    let provide = || -> Result<cordis_abi::Object> {
        <dyn Provided as Interface>::object(std::sync::Arc::new(Implementation), environment(3))
    };

    // 同一份声明：摘要一致，按槽位调用。
    let object = provide()?;
    assert!(object.typed_layout_matches(<dyn Same as Interface>::LAYOUT_DIGEST));
    let same = Service::<dyn Same>::from_object(object)?;
    assert_eq!(same.double(21)?, 42);
    assert_eq!(same.label("same".into())?, "same!");

    // 方法顺序不同：按槽位调用会把 double 的参数交给 label。修复后按方法名调用，结果正确。
    let object = provide()?;
    assert!(!object.typed_layout_matches(<dyn Reordered as Interface>::LAYOUT_DIGEST));
    let reordered = Service::<dyn Reordered>::from_object(object)?;
    assert_eq!(reordered.double(21)?, 42);
    assert_eq!(reordered.label("reordered".into())?, "reordered!");

    // 原始类型不同：按方法名调用时参数经 CBOR 解码，u32 值被提供方按 u64 正确接收。
    let narrowed = Service::<dyn Narrowed>::from_object(provide()?)?;
    assert_eq!(narrowed.double(21)?, 42);

    // 同步改异步：按方法名进入通用异步入口。
    let asynchronous = Service::<dyn Asynchronous>::from_object(provide()?)?;
    assert_eq!(asynchronous.double(21).await?, 42);
    Ok(())
}
