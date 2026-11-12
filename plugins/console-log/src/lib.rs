//! ADR-0032：终端插件按 logger-console 的布局规则输出 UP/1 日志事件。

mod config;
mod format;
#[cfg(test)]
mod tests;

pub use config::ConsoleConfig;
use cordis_sdk::{builtin::LogRecord, prelude::*};
use format::Renderer;
use std::{
    io::{self, IsTerminal, Write},
    sync::Mutex,
};

pub struct Console;
impl Component for Console {
    const NAME: &'static str = "console";
    type Config = ConsoleConfig;
    fn listens() -> Listens {
        Listens::new().with::<LogRecord>()
    }
    fn resolve(config: &ConsoleConfig) -> Result<()> {
        if config.level > 3 {
            return Err(Error::new(2, "level取值为0到3"));
        }
        Ok(())
    }
    /// ADR-0032：串行渲染接收到的事件，以标准错误保持 UP/1 标准输出通道完整。
    async fn apply(ctx: Context, config: ConsoleConfig) -> Result<()> {
        let renderer = Mutex::new(Renderer::new(
            config,
            io::stderr().is_terminal(),
            chrono::Local::now().timestamp_millis(),
        ));
        ctx.on::<LogRecord, _>(move |record| {
            let mut renderer = renderer
                .lock()
                .map_err(|error| Error::new(5, error.to_string()))?;
            if let Some(output) = renderer.render(&record, chrono::Local::now().fixed_offset()) {
                writeln!(io::stderr().lock(), "{output}")
                    .map_err(|error| Error::new(1, error.to_string()))?;
            }
            Ok(())
        })
    }
}

cordis_sdk::export_plugin!(
    name = "console-log",
    version = env!("CARGO_PKG_VERSION"),
    components = [Console]
);
