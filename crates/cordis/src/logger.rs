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

/// S-LOG-4：按 `Error`、`Warn`、`Info`、`Debug` 递增的日志阈值。
///
/// 消息级别小于或等于目标阈值时被接收；默认阈值为 `Info`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// 错误消息，级别序号为 0。
    Error,
    /// 警告消息，级别序号为 1。
    Warn,
    /// 一般信息，级别序号为 2。
    Info,
    /// 调试消息，级别序号为 3。
    Debug,
}

/// 日志参数；字符串可通过 `Into<LogValue>` 转换为文本。
#[derive(Clone, Debug)]
pub enum LogValue {
    /// 持有一段文本。
    Text(String),
    /// 持有可克隆错误，供单错误参数的原因或聚合成员展开。
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

/// S-LOG-2：日志来源内核及上下文的弱引用。
///
/// 保留日志消息时，来源资源的存续继续由实际上下文和纤程句柄决定。
#[derive(Clone)]
pub struct WeakFiber {
    kernel: Weak<Kernel>,
    scope: Weak<Scope>,
}
impl WeakFiber {
    /// 来源内核和上下文仍存续时返回对应 `Fiber`，资源已释放时返回 `None`。
    ///
    /// 返回的纤程可以处于 `Disposed`；升级成功会建立一次新的强引用。
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
    /// 共享内核内递增的消息序号；错误展开后的每条消息分别取得序号。
    pub sn: u64,
    /// 记录时调用宿主 `Clock::now` 取得的时间。
    pub ts: Duration,
    /// 已解析的日志器名称。
    pub name: String,
    /// 本条消息的级别，供各输出目标独立过滤。
    pub level: LogLevel,
    /// 本条消息携带的参数序列。
    pub args: Vec<LogValue>,
    /// 产生消息的纤程来源，通过弱引用保留。
    pub fiber: WeakFiber,
}

/// 日志拦截元数据，按字段采用后层优先合并。
///
/// `None` 表示继承前层值，全部字段为 `None` 时构成合并单位元。
#[derive(Clone, Debug, Default)]
pub struct LoggerMeta {
    /// 默认日志器名称；显式 `Logger::named` 参数具有更高优先级。
    pub name: Option<String>,
    /// 日志器级别，供有界缓冲及缺省导出器阈值使用。
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
/// 日志拦截键，供 `Context::intercept::<LoggerKey>` 配置名称和级别。
pub struct LoggerKey;
impl ServiceKey for LoggerKey {
    /// 日志拦截使用的键名称。
    const NAME: &'static str = "logger";
    /// 元数据入口使用单位值；日志器通过 `Context::logger` 取得。
    type Value = ();
    /// 日志名称与级别的拦截配置。
    type Meta = LoggerMeta;
}

/// S-LOG-4：单个导出器的独立阈值配置。
///
/// 阈值依次取名称专属配置、导出器默认配置、日志器级别、`Info`。
#[derive(Clone, Default)]
pub struct ExporterOptions {
    /// 日志器名称到接收阈值的映射。
    pub levels: HashMap<String, LogLevel>,
    /// 名称缺少专属配置时采用的导出器阈值。
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

/// 保存日志名称、级别、来源上下文和导出器效应归属的句柄。
///
/// 同一内核共享有界缓冲及导出器集合，默认缓冲容量为 1000 条。
/// 普通日志调用在调用线程内完成消息记录，并在锁外同步调用匹配的导出器。
#[derive(Clone)]
pub struct Logger {
    context: Context,
    source: Context,
    name: String,
    level: Option<LogLevel>,
}
impl Context {
    /// S-LOG-3：返回当前上下文的日志器。
    ///
    /// 名称优先取 `LoggerKey` 拦截配置，其后取纤程名称的连字符形式。
    /// 日志来源及新登记导出器的效应归属均为当前上下文。
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

    /// S-LOG-3：返回使用显式 `name` 的日志器，保留原级别和上下文归属。
    pub fn named(&self, name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..self.clone()
        }
    }
    /// 返回日志器当前解析出的名称。
    pub fn name(&self) -> &str {
        &self.name
    }
    /// 克隆并返回共享有界缓冲中的消息，按记录顺序从旧到新排列。
    ///
    /// 返回值为本次读取的快照，来源字段继续使用弱引用。
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
    /// 将共享缓冲容量设为 `capacity` 条，并立即移除超出容量的最旧消息。
    ///
    /// 容量为 0 时缓冲保持空集合，外部导出器继续接收匹配消息。
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

    /// 返回当前已登记的外部导出器数量；有界缓冲独立维护。
    pub fn exporter_count(&self) -> usize {
        self.context.kernel.state.lock().logger.exporters.len()
    }

    /// S-LOG-1：按 `options` 登记消息回调，返回管理该导出器的效应句柄。
    ///
    /// 每条匹配消息在内核锁外调用一次 `callback`；回调可重入内核。
    /// 登记效应归属于日志器的使用点上下文，显式释放或所属纤程卸载时移除。
    /// 上下文已经释放时返回效应登记错误。
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

    /// 将 `value` 转为日志参数，以 `Error` 级别同步记录并派发匹配导出器。
    pub fn error(&self, value: impl Into<LogValue>) {
        self.log(LogLevel::Error, vec![value.into()]);
    }
    /// 将 `value` 转为日志参数，以 `Warn` 级别同步记录并派发匹配导出器。
    pub fn warn(&self, value: impl Into<LogValue>) {
        self.log(LogLevel::Warn, vec![value.into()]);
    }
    /// 将 `value` 转为日志参数，以 `Info` 级别同步记录并派发匹配导出器。
    pub fn info(&self, value: impl Into<LogValue>) {
        self.log(LogLevel::Info, vec![value.into()]);
    }
    /// 将 `value` 转为日志参数，以 `Debug` 级别同步记录并派发匹配导出器。
    pub fn debug(&self, value: impl Into<LogValue>) {
        self.log(LogLevel::Debug, vec![value.into()]);
    }

    /// S-LOG-4、S-LOG-5：以 `level` 记录参数序列 `args`，分别过滤缓冲和各导出器。
    ///
    /// 参数恰为单个错误时，先递归记录其 `cause`，再记录原错误；
    /// `cause` 为空且 `causes` 含成员时，聚合记录由各成员的递归记录组成。
    /// 空成员集合以原错误产生一条记录。每条记录独立分配序号和宿主时间。
    /// 普通调用中导出器回调的 panic 向调用方传播。
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
