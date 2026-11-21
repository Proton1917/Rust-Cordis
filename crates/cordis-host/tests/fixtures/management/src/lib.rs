use cordis_sdk::{builtin::Manage, prelude::*};
use serde::Deserialize;
use std::{fs, path::PathBuf, time::Duration};

#[cordis_sdk::interface(name = "fixture.switch", version = "1.0")]
trait Switch: Send + Sync {
    fn value(&self) -> Result<u32>;
}
struct Provider;
#[cordis_sdk::interface]
impl Switch for Provider {
    fn value(&self) -> Result<u32> {
        Ok(107)
    }
}
#[derive(Deserialize)]
struct Settings {
    enabled: bool,
}
impl Component for Provider {
    const NAME: &'static str = "provider";
    type Config = Settings;
    fn provides() -> Provides {
        Provides::new().with::<dyn Switch>()
    }
    async fn apply(ctx: Context, config: Settings) -> Result<()> {
        if config.enabled {
            ctx.provide::<dyn Switch>(Arc::new(Provider))?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Caller {
    directory: PathBuf,
}
struct Reader;
struct Admin;
impl Component for Reader {
    const NAME: &'static str = "reader";
    type Config = Caller;
    fn inject() -> Inject {
        Inject::new().with::<dyn Manage>().optional::<dyn Switch>()
    }
    async fn apply(ctx: Context, config: Caller) -> Result<()> {
        match ctx.get::<dyn Switch>() {
            Ok(switch) => {
                return fs::write(config.directory.join("value"), switch.value()?.to_string())
                    .map_err(io_error);
            }
            Err(error) if error.code() == 4 => (),
            Err(error) => return Err(error),
        }
        command_thread(ctx, config, false)
    }
}
impl Component for Admin {
    const NAME: &'static str = "admin";
    type Config = Caller;
    fn inject() -> Inject {
        Inject::new().with::<dyn Manage>()
    }
    async fn apply(ctx: Context, config: Caller) -> Result<()> {
        command_thread(ctx, config, true)
    }
}
fn io_error(error: std::io::Error) -> Error {
    Error::new(1, error.to_string())
}
fn command_thread(ctx: Context, config: Caller, replace: bool) -> Result<()> {
    ctx.spawn_thread("management-caller", move |ctx| {
        let trigger = config.directory.join("go");
        loop {
            match fs::remove_file(&trigger) {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(io_error(e)),
            }
            if !ctx.is_current()? {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let manage = ctx.get::<dyn Manage>()?;
        if replace {
            let candidate = config
                .directory
                .join("candidate")
                .with_extension(std::env::consts::DLL_EXTENSION);
            let reference = ctx.wait(manage.install(candidate.to_str().unwrap()))?;
            ctx.wait(manage.replace("provider", &reference))?;
        } else {
            let config = CborValue::Map(vec![("enabled".into(), true.into())]);
            ctx.wait(manage.configure("provider", config))?;
        }
        fs::write(config.directory.join("accepted"), "ok").map_err(io_error)
    })
}
cordis_sdk::export_plugin!(
    name = "management-caller",
    version = "0.1.0",
    components = [Reader, Admin, Provider]
);
