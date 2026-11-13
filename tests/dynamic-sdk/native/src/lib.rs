//! ADR-0047 AU-27–AU-30：真实类型化与动态调用共用依赖，记录返回值、身份与资源清理。

mod probe;
mod streams;

use cordis_sdk::prelude::*;

#[cordis_sdk::interface(name = "dynamic.identity", version = "1.0")]
trait Identity: Send + Sync {
    fn metadata(&self) -> Result<CborValue>;
}

#[cordis_sdk::interface(name = "dynamic.undeclared", version = "1.0")]
trait Undeclared: Send + Sync {
    fn value(&self) -> Result<u64>;
}

struct IdentityValue(CborValue);
#[cordis_sdk::interface]
impl Identity for IdentityValue {
    fn metadata(&self) -> Result<CborValue> {
        Ok(self.0.clone())
    }
}

#[cordis_sdk::interface]
impl Undeclared for IdentityValue {
    fn value(&self) -> Result<u64> {
        Ok(42)
    }
}

struct IdentityProvider;
impl Component for IdentityProvider {
    const NAME: &'static str = "identity";
    type Config = EmptyConfig;
    fn provides() -> Provides {
        Provides::new()
            .with::<dyn Identity>()
            .with::<dyn Undeclared>()
    }
    async fn apply(context: Context, _: EmptyConfig) -> Result<()> {
        context.provide::<dyn Undeclared>(Arc::new(IdentityValue(CborValue::Null)))?;
        context.provide_scoped::<dyn Identity>(Scoped::new(
            Arc::new(IdentityValue(CborValue::Null)),
            |metadata: CborValue| Ok(Arc::new(IdentityValue(metadata))),
        ))
    }
}

fn problem(value: impl std::fmt::Display) -> Error {
    Error::new(1, value.to_string())
}

cordis_sdk::export_plugin!(
    name = "dynamic-tests",
    version = "0.1.0",
    components = [probe::Probe, IdentityProvider, streams::Provider]
);
