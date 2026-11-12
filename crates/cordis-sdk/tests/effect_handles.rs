#[path = "support/host.rs"]
pub mod host;

use cordis_abi::testing::environment;
use cordis_sdk::{Context, EmptyConfig, Result};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_disposal_reclaims_external_handles() -> Result<()> {
    let host = Arc::new(host::Loopback::new());
    cordis_abi::testing::install(host.clone())?;
    let context = host.context(cordis_sdk::encode(&EmptyConfig {})?, Vec::new(), None);
    let ctx = Context::new(environment(9), context);
    let cleaned = Arc::new(AtomicUsize::new(0));
    for _ in 0..3 {
        let effect = ctx.effect("local resource", {
            let cleaned = cleaned.clone();
            move |scope| {
                scope.defer(move || {
                    cleaned.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }
        })?;
        let last = effect.clone();
        effect.dispose().await?;
        drop(effect);
        last.settled().await?;
        drop(last);
    }
    assert_eq!(cleaned.load(Ordering::SeqCst), 3);
    assert_eq!(
        host.effect_records(),
        (0, 0),
        "局部效应已清理，最后一个SDK句柄释放后应回收其外部记录"
    );

    let effect = ctx.effect("context resource", {
        let cleaned = cleaned.clone();
        move |scope| {
            scope.defer(move || {
                cleaned.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
    })?;
    drop(effect);
    assert_eq!(host.effect_records(), (1, 0));
    assert_eq!(cleaned.load(Ordering::SeqCst), 3);
    host.dispose(context).await?;
    assert_eq!(cleaned.load(Ordering::SeqCst), 4);
    assert_eq!(host.effect_records(), (0, 0));
    Ok(())
}
