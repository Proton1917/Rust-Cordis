use cordis_sdk::{Result, Service};

#[cordis_sdk::interface(name = "loader.counter", version = "1.0")]
pub trait Counter: Send + Sync {
    fn value(&self) -> Result<u64>;
    fn add(&self, amount: u64) -> Result<u64>;
    fn implementation(&self) -> Result<u64>;
}

#[cordis_sdk::interface(name = "loader.counter", version = "1.2")]
pub trait NewCounter: Send + Sync {
    fn value(&self) -> Result<u64>;
    fn add(&self, amount: u64) -> Result<u64>;
    fn implementation(&self) -> Result<u64>;
}

#[cordis_sdk::interface(name = "loader.counter", version = "2.0")]
pub trait CounterV2: Send + Sync {
    fn value(&self) -> Result<u64>;
    fn add(&self, amount: u64) -> Result<u64>;
    fn implementation(&self) -> Result<u64>;
}

#[cordis_sdk::event(name = "loader.pulse", version = "1.0")]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Pulse {
    pub amount: u64,
}

#[cordis_sdk::event(name = "loader.pulse", version = "1.0", kind = "async")]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct AsyncPulse {
    pub amount: u64,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Metadata {
    #[serde(default)]
    pub offset: u64,
    #[serde(default)]
    pub label: String,
}

#[cordis_sdk::interface(name = "host.scoped", version = "1.0")]
pub trait ScopedCounter: Send + Sync {
    fn value(&self) -> Result<u64>;
    fn add(&self, amount: u64) -> Result<u64>;
    fn bindings(&self) -> Result<u64>;
    fn label(&self) -> Result<String>;
}

#[cordis_sdk::interface(name = "host.control", version = "1.0")]
pub trait Control: Send + Sync {
    fn offer(&self, service: Service<dyn Counter>) -> Result<()>;
}

#[cordis_sdk::interface(name = "host.wait-probe", version = "1.0")]
pub trait WaitProbe: Send + Sync {
    fn call(&self) -> Result<u64>;
    fn nested(&self) -> Result<u64>;
    fn system_thread(&self) -> Result<u64>;
    fn task(&self) -> Result<u64>;
    fn failure(&self) -> Result<u64>;
    fn panics(&self) -> Result<u64>;
    fn legacy(&self) -> Result<u64>;
    fn polls(&self) -> Result<u64>;
}

#[cordis_sdk::interface(name = "host.wait-control", version = "1.0")]
pub trait WaitControl: Send + Sync {
    fn offer(&self, service: Service<dyn WaitProbe>) -> Result<()>;
}
