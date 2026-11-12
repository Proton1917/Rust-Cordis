//! DOC-11 PR-2、PR-3：任务和系统线程作为核心效应登记，清理等待资源结束。

use crate::{abi, host::Host, native::Cancellation};
use cordis_abi::{AbiUnit, Error, Object, RemoteFuture, Result, async_callback, sync_callback};
use parking_lot::Mutex;
use std::sync::Arc;

struct Stop(Option<Cancellation>);

impl Stop {
    fn cancel(&self) {
        if let Some(cancel) = &self.0 {
            cancel.cancel();
        }
    }
}

impl Drop for Stop {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct Job {
    stop: Stop,
    monitor: tokio::task::JoinHandle<()>,
}

impl Job {
    async fn stop(self) -> Result<()> {
        let Self { stop, monitor } = self;
        stop.cancel();
        monitor
            .await
            .map_err(|error| Error::new(5, error.to_string()))?;
        drop(stop);
        Ok(())
    }
}

impl Host {
    /// DOC-11 PR-2：先登记取消并等待的清理回调，再启动原生 Future。
    pub fn spawn(&self, context: u64, task: RemoteFuture<AbiUnit>) -> Result<()> {
        let task = Mutex::new(Some(task));
        let host = self.clone();
        let body = sync_callback(abi::environment(), move |_, scope, _| {
            let task = task
                .lock()
                .take()
                .ok_or_else(|| Error::new(4, "任务派发效应已执行"))?;
            let resource = Arc::new(Mutex::new(None::<Job>));
            let cleanup = async_callback(abi::environment(), {
                let resource = resource.clone();
                move |_, _, _| {
                    let job = resource.lock().take();
                    async move {
                        if let Some(job) = job {
                            job.stop().await?;
                        }
                        Ok(())
                    }
                }
            })?;
            host.defer(scope, cleanup, true)?;
            let task = host.0.executor.start(task, true);
            let cancel = task.cancellation();
            let retained = host.clone();
            let monitor = host.0.runtime.spawn(async move {
                if let Err(error) = task.await
                    && error.code() != 7
                {
                    abi::environment().report(Error::new(
                        error.code(),
                        format!("context {context} 后台任务：{error}"),
                    ));
                }
                drop(retained);
            });
            *resource.lock() = Some(Job {
                stop: Stop(cancel),
                monitor,
            });
            Ok(())
        })?;
        let effect = self.effect(context, "spawn", body, false)?;
        self.effect_release(effect)
    }

    /// DOC-11 PR-3：系统线程随效应反登记完成 unpark 与 join。
    pub fn spawn_thread(&self, context: u64, name: &str, body: Object) -> Result<()> {
        let body = Mutex::new(Some(body));
        let name = name.to_owned();
        let host = self.clone();
        let callback = sync_callback(abi::environment(), move |_, scope, _| {
            let body = body
                .lock()
                .take()
                .ok_or_else(|| Error::new(4, "线程派发效应已执行"))?;
            let resource = Arc::new(Mutex::new(None::<std::thread::JoinHandle<()>>));
            let cleanup = async_callback(abi::environment(), {
                let resource = resource.clone();
                move |_, _, _| {
                    let thread = resource.lock().take();
                    async move {
                        if let Some(thread) = thread {
                            thread.thread().unpark();
                            tokio::task::spawn_blocking(move || thread.join())
                                .await
                                .map_err(|error| Error::new(5, error.to_string()))?
                                .map_err(|_| Error::new(5, "原生系统线程发生 panic"))?;
                        }
                        Ok(())
                    }
                }
            })?;
            host.defer(scope, cleanup, true)?;
            let thread = std::thread::Builder::new()
                .name(name.clone())
                .spawn(move || {
                    if let Err(error) = body.call_sync(context, 0, &[]) {
                        abi::environment().report(Error::new(
                            error.code(),
                            format!("context {context} 系统线程：{error}"),
                        ));
                    }
                })
                .map_err(|error| Error::new(1, error.to_string()))?;
            *resource.lock() = Some(thread);
            Ok(())
        })?;
        let effect = self.effect(context, "spawn-thread", callback, false)?;
        self.effect_release(effect)
    }
}
