//! 日志消息、可回退导出器与有界缓冲。

use crate::{
    Context, Effect, Error, Fiber, Merge, Result, ServiceKey,
    context::Scope,
    kernel::{FiberId, Kernel},
};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    fmt,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

#[derive(Clone, Debug)]
pub enum LogValue {
    Text(String),
    Error(Error),
}
impl From<&str> for LogValue {
    fn from(value: &str) -> Self {
        Self::Text(value.into())
    }
}
impl From<String> for LogValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}
impl From<Error> for LogValue {
    fn from(value: Error) -> Self {
        Self::Error(value)
    }
}

#[derive(Clone)]
pub struct WeakFiber {
    kernel: Weak<Kernel>,
    scope: Weak<Scope>,
}
impl WeakFiber {
    pub fn upgrade(&self) -> Option<Fiber> {
        Some(
            Context {
                kernel: self.kernel.upgrade()?,
                scope: self.scope.upgrade()?,
            }
            .fiber(),
        )
    }
}
impl fmt::Debug for WeakFiber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WeakFiber").finish_non_exhaustive()
    }
}

/// S-LOG-2：消息保留序号、宿主时间与弱来源引用。
#[derive(Clone, Debug)]
pub struct LogMessage {
    pub sn: u64,
    pub ts: Duration,
    pub name: String,
    pub level: LogLevel,
    pub args: Vec<LogValue>,
    pub fiber: WeakFiber,
}

#[derive(Clone, Debug, Default)]
pub struct LoggerMeta {
    pub name: Option<String>,
    pub level: Option<LogLevel>,
}
impl Merge for LoggerMeta {
    fn merge(&self, next: &Self) -> Self {
        Self {
            name: next.name.clone().or_else(|| self.name.clone()),
            level: next.level.or(self.level),
        }
    }
}
pub struct LoggerKey;
impl ServiceKey for LoggerKey {
    const NAME: &'static str = "logger";
    type Value = ();
    type Meta = LoggerMeta;
}

#[derive(Clone, Default)]
pub struct ExporterOptions {
    pub levels: HashMap<String, LogLevel>,
    pub default_level: Option<LogLevel>,
}
struct Exporter {
    callback: Arc<dyn Fn(&LogMessage) + Send + Sync>,
    options: ExporterOptions,
    removed: AtomicBool,
}
pub(crate) struct LoggerState {
    buffer: VecDeque<LogMessage>,
    buffer_size: usize,
    exporters: BTreeMap<u64, Arc<Exporter>>,
    next_exporter: u64,
    next_message: u64,
}
impl Default for LoggerState {
    fn default() -> Self {
        Self {
            buffer: VecDeque::new(),
            buffer_size: 1000,
            exporters: BTreeMap::new(),
            next_exporter: 0,
            next_message: 0,
        }
    }
}

#[derive(Clone)]
pub struct Logger {
    context: Context,
    source: Context,
    name: String,
    level: Option<LogLevel>,
}
impl Context {
    pub fn logger(&self) -> Logger {
        Logger::for_service(self.clone(), self)
    }
}
impl Logger {
    pub(crate) fn for_service(context: Context, intercepts: &Context) -> Self {
        let metadata = intercepts.intercept_meta::<LoggerKey>();
        let name = metadata
            .name
            .unwrap_or_else(|| hyphenate(&context.fiber().name()));
        Self {
            context: intercepts.clone(),
            source: context,
            name,
            level: metadata.level,
        }
    }

    /// S-LOG-3：显式名称覆盖上下文解析的名称。
    pub fn named(&self, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..self.clone()
        }
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn buffer(&self) -> Vec<LogMessage> {
        self.context
            .kernel
            .state
            .lock()
            .logger
            .buffer
            .iter()
            .cloned()
            .collect()
    }
    pub fn set_buffer_size(&self, capacity: usize) {
        let discarded = {
            let mut state = self.context.kernel.state.lock();
            let logger = &mut state.logger;
            logger.buffer_size = capacity;
            let overflow = logger.buffer.len().saturating_sub(capacity);
            logger.buffer.drain(..overflow).collect::<Vec<_>>()
        };
        drop(discarded);
    }

    pub fn exporter_count(&self) -> usize {
        self.context.kernel.state.lock().logger.exporters.len()
    }

    /// S-LOG-1：每个导出器的效应释放其自身登记项。
    #[track_caller]
    pub fn exporter(
        &self,
        options: ExporterOptions,
        callback: impl Fn(&LogMessage) + Send + Sync + 'static,
    ) -> Result<Effect> {
        let kernel = self.context.kernel.clone();
        let exporter = Arc::new(Exporter {
            callback: Arc::new(callback),
            options,
            removed: AtomicBool::new(false),
        });
        self.context.effect("ctx.logger.exporter()", move |fx| {
            let id = {
                let mut state = kernel.state.lock();
                state.logger.next_exporter += 1;
                let id = state.logger.next_exporter;
                state.logger.exporters.insert(id, exporter.clone());
                id
            };
            fx.defer(move || {
                exporter.removed.store(true, Ordering::Release);
                let removed = kernel.state.lock().logger.exporters.remove(&id);
                drop(removed);
                Ok(())
            })
        })
    }

    pub fn error(&self, value: impl Into<LogValue>) {
        self.log(LogLevel::Error, vec![value.into()]);
    }
    pub fn warn(&self, value: impl Into<LogValue>) {
        self.log(LogLevel::Warn, vec![value.into()]);
    }
    pub fn info(&self, value: impl Into<LogValue>) {
        self.log(LogLevel::Info, vec![value.into()]);
    }
    pub fn debug(&self, value: impl Into<LogValue>) {
        self.log(LogLevel::Debug, vec![value.into()]);
    }

    /// S-LOG-4、S-LOG-5：独立过滤每个导出器，并展开错误的原因或成员。
    pub fn log(&self, level: LogLevel, args: Vec<LogValue>) {
        if let [LogValue::Error(error)] = args.as_slice() {
            if let Some(cause) = &error.cause {
                self.log(level, vec![LogValue::Error((**cause).clone())]);
            } else if !error.causes.is_empty() {
                for cause in &error.causes {
                    self.log(level, vec![LogValue::Error(cause.clone())]);
                }
                return;
            }
        }
        self.publish(level, args, true);
    }

    fn publish(&self, level: LogLevel, args: Vec<LogValue>, export: bool) {
        let ts = self.context.now();
        let (message, exporters, discarded) = {
            let mut state = self.context.kernel.state.lock();
            let logger = &mut state.logger;
            logger.next_message += 1;
            let message = LogMessage {
                sn: logger.next_message,
                ts,
                name: self.name.clone(),
                level,
                args,
                fiber: WeakFiber {
                    kernel: Arc::downgrade(&self.source.kernel),
                    scope: Arc::downgrade(&self.source.scope),
                },
            };
            let mut discarded = Vec::new();
            if level <= self.level.unwrap_or(LogLevel::Info) {
                logger.buffer.push_back(message.clone());
                let overflow = logger.buffer.len().saturating_sub(logger.buffer_size);
                discarded.extend(logger.buffer.drain(..overflow));
            }
            let exporters = if export {
                logger.exporters.values().cloned().collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            (message, exporters, discarded)
        };
        drop(discarded);
        for exporter in exporters {
            let threshold = exporter
                .options
                .levels
                .get(&self.name)
                .copied()
                .or(exporter.options.default_level)
                .or(self.level)
                .unwrap_or(LogLevel::Info);
            if level <= threshold && !exporter.removed.load(Ordering::Acquire) {
                (exporter.callback)(&message);
            }
        }
    }
}

pub(crate) fn record_error(kernel: &Arc<Kernel>, fiber: FiberId, error: Error) {
    let scope = kernel.state.lock().fibers[fiber]
        .canonical_scope
        .upgrade()
        .expect("reporting fiber retains a scope lease");
    let logger = Context {
        kernel: kernel.clone(),
        scope,
    }
    .logger();
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| logger.error(error))) {
        logger.publish(
            LogLevel::Error,
            vec![LogValue::Error(Error::panic(payload))],
            false,
        );
    }
}

pub(crate) fn errors(kernel: &Arc<Kernel>) -> Vec<Error> {
    kernel
        .state
        .lock()
        .logger
        .buffer
        .iter()
        .filter(|message| message.level == LogLevel::Error)
        .flat_map(|message| message.args.iter())
        .filter_map(|value| {
            if let LogValue::Error(error) = value {
                Some(error.clone())
            } else {
                None
            }
        })
        .collect()
}

fn hyphenate(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    let mut previous_lower = false;
    for character in name.chars() {
        if character.is_uppercase() {
            if previous_lower {
                result.push('-');
            }
            result.extend(character.to_lowercase());
        } else {
            result.push(character);
        }
        previous_lower = character.is_lowercase() || character.is_ascii_digit();
    }
    result
}
