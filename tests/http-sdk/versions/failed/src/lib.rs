use cordis_sdk::prelude::*;

struct Rejected;
impl Component for Rejected {
    const NAME: &'static str = "http-echo";
    type Config = CborValue;
    fn inject() -> Inject {
        Inject::new()
            .with::<dyn http_api::Http>()
            .with::<dyn cordis_sdk::builtin::DataDir>()
    }
    async fn apply(_: Context, _: CborValue) -> Result<()> {
        Err(Error::new(1001, "HTTP 处理方候选拒绝激活"))
    }
}
cordis_sdk::export_plugin!(
    name = "http-echo",
    version = "0.1.4",
    components = [Rejected]
);
