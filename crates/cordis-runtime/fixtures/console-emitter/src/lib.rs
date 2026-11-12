use cordis_sdk::prelude::*;
use cordis_sdk::builtin::Runtime;
use std::time::Duration;

struct Emitter;
impl Component for Emitter {
    const NAME: &'static str = "emitter";
    type Config = EmptyConfig;

    fn inject() -> Inject { Inject::new().with::<dyn Runtime>() }

    async fn apply(ctx: Context, _: EmptyConfig) -> Result<()> {
        let task = ctx.clone();
        ctx.spawn(async move {
            let runtime = task.get::<dyn Runtime>()?;
            loop {
                let snapshot = runtime.snapshot()?;
                if ["console", "emitter"].iter().all(|name| snapshot.instances.iter()
                    .any(|instance| instance.name == *name && instance.state == "active")) { break; }
                task.sleep(Duration::from_millis(1)).await?;
            }
            task.log(2, "first\nsecond")?;
            task.log(3, "debug")?;
            task.log(1, "warning")
        })
    }
}

cordis_sdk::export_plugin!(name = "console-emitter", version = env!("CARGO_PKG_VERSION"), components = [Emitter]);
