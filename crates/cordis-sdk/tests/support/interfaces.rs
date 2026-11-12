use cordis_sdk::prelude::*;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Document {
    title: String,
    bytes: Vec<u8>,
    revisions: u32,
}

#[cordis_sdk::interface(name = "test.value", version = "1.0")]
trait Value: Send + Sync {
    fn value(&self) -> Result<u64>;
}

#[cordis_sdk::interface(name = "test.types", version = "1.0")]
trait Types: Send + Sync {
    fn numeric(&self, flag: bool, a: i8, b: i16, c: i32, d: i64,
        e: u8, f: u16, g: u32, h: u64, x: f32, y: f64) -> Result<i64>;
    fn invert(&self, value: bool) -> Result<bool>;
    fn annotate(&self, title: &str, bytes: &[u8]) -> Result<String>;
    fn owned(&self, text: String) -> Result<String>;
    fn unit(&self) -> Result<()>;
    fn normalize(&self, bytes: Vec<u8>) -> Result<Vec<u8>>;
    fn optional(&self, title: Option<&str>, bytes: Option<Vec<u8>>) -> Result<Option<String>>;
    fn revise(&self, document: Document) -> Result<Document>;
    fn service(&self, value: Service<dyn Value>) -> Result<Service<dyn Value>>;
    fn maybe_service(&self, value: Option<Service<dyn Value>>) -> Result<Option<Service<dyn Value>>>;
    fn nullable(&self, value: Option<Option<()>>) -> Result<Option<Option<()>>>;
    fn reject_service(&self, value: Service<dyn Value>) -> Result<()>;
    async fn service_async(&self, value: Service<dyn Value>) -> Result<Service<dyn Value>>;
    async fn hold_service(&self, value: Service<dyn Value>) -> Result<()>;
    async fn borrowed(&self, title: &str, bytes: &[u8]) -> Result<Document>;
    fn business_error(&self) -> Result<()>;
    fn panic(&self) -> Result<()>;
}

struct Transformer;

#[cordis_sdk::interface]
impl Types for Transformer {
    fn numeric(&self, flag: bool, a: i8, b: i16, c: i32, d: i64,
        e: u8, f: u16, g: u32, h: u64, x: f32, y: f64) -> Result<i64>
    {
        let integers = i64::from(a) + i64::from(b) + i64::from(c) + d
            + i64::from(e) + i64::from(f) + i64::from(g) + h as i64;
        Ok(if flag { integers + (f64::from(x) + y) as i64 } else { integers })
    }

    fn invert(&self, value: bool) -> Result<bool> { Ok(!value) }

    fn annotate(&self, title: &str, bytes: &[u8]) -> Result<String> {
        Ok(format!("{title}:{}", bytes.len()))
    }

    fn owned(&self, text: String) -> Result<String> { Ok(text.to_uppercase()) }
    fn unit(&self) -> Result<()> { Ok(()) }

    fn normalize(&self, mut bytes: Vec<u8>) -> Result<Vec<u8>> {
        bytes.sort_unstable();
        bytes.dedup();
        Ok(bytes)
    }

    fn optional(&self, title: Option<&str>, bytes: Option<Vec<u8>>) -> Result<Option<String>> {
        Ok(title.zip(bytes).map(|(title, bytes)| format!("{title}:{}", bytes.len())))
    }

    fn revise(&self, mut document: Document) -> Result<Document> {
        document.revisions += 1;
        document.title.push_str(" / revised");
        Ok(document)
    }

    fn service(&self, value: Service<dyn Value>) -> Result<Service<dyn Value>> { Ok(value) }

    fn maybe_service(&self, value: Option<Service<dyn Value>>) -> Result<Option<Service<dyn Value>>> { Ok(value) }
    fn nullable(&self, value: Option<Option<()>>) -> Result<Option<Option<()>>> { Ok(value) }
    fn reject_service(&self, _value: Service<dyn Value>) -> Result<()> { Err(Error::new(1044, "resource rejected")) }

    async fn service_async(&self, value: Service<dyn Value>) -> Result<Service<dyn Value>> {
        tokio::task::yield_now().await;
        Ok(value)
    }

    async fn hold_service(&self, value: Service<dyn Value>) -> Result<()> {
        value.value()?;
        std::future::pending::<()>().await;
        Ok(())
    }

    async fn borrowed(&self, title: &str, bytes: &[u8]) -> Result<Document> {
        tokio::task::yield_now().await;
        Ok(Document { title: title.to_owned(), bytes: bytes.to_vec(), revisions: 1 })
    }

    fn business_error(&self) -> Result<()> {
        Err(Error::new(1042, "record is locked").with_detail(cordis_sdk::encode(&"locked")?))
    }
    fn panic(&self) -> Result<()> { panic!("service failure") }
}

mod old {
    use cordis_sdk::prelude::*;
    #[cordis_sdk::interface(name = "test.version", version = "1.0")]
    pub trait Versioned: Send + Sync {
        fn value(&self) -> Result<String>;
    }
}

mod current {
    use cordis_sdk::prelude::*;
    #[cordis_sdk::interface(name = "test.version", version = "1.1")]
    pub trait Versioned: Send + Sync {
        fn value(&self) -> Result<String>;
        fn added(&self) -> Result<u64>;
    }
}

struct OldImplementation;
impl old::Versioned for OldImplementation {
    fn value(&self) -> Result<String> { Ok("old".into()) }
}
struct CurrentImplementation;
impl current::Versioned for CurrentImplementation {
    fn value(&self) -> Result<String> { Ok("current".into()) }
    fn added(&self) -> Result<u64> { Ok(11) }
}

#[cordis_sdk::event(name = "test.changed", version = "1.0")]
#[derive(serde::Serialize, serde::Deserialize)]
struct OldEvent { count: u64 }

#[cordis_sdk::event(name = "test.changed", version = "1.1")]
#[derive(serde::Serialize, serde::Deserialize)]
struct CurrentEvent {
    count: u64,
    #[serde(default)]
    text: String,
}

#[cordis_sdk::event(name = "test.changed", version = "1.1", kind = "async")]
#[derive(serde::Serialize, serde::Deserialize)]
struct AsyncChanged { count: u64 }
