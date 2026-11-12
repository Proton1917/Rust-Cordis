//! DOC-15 CP-1、CP-8：原生代码在独立执行器轮询和析构，协议读写保持响应。

use cordis_abi::{Error, Result};
use std::{
    future::Future,
    ops::Deref,
    pin::Pin,
    task::{Context, Poll},
};

/// 插件轮询和析构使用独立运行时，UP/1读写和EOF监视留在主运行时。
#[derive(Clone)]
pub struct NativeExecutor {
    runtime: tokio::runtime::Handle,
    io: tokio::runtime::Handle,
}

pub struct RuntimeOwner(Option<tokio::runtime::Runtime>);

impl RuntimeOwner {
    pub fn new(workers: usize) -> Result<Self> {
        if workers == 0 {
            return Err(Error::new(2, "原生执行线程数需要为正值"));
        }
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers)
            .thread_name("cordis-native")
            .enable_all()
            .build()
            .map(|runtime| Self(Some(runtime)))
            .map_err(|error| Error::new(1, error.to_string()))
    }

    pub fn handle(&self) -> tokio::runtime::Handle {
        self.0
            .as_ref()
            .expect("运行时由当前宿主持有")
            .handle()
            .clone()
    }
}

impl Drop for RuntimeOwner {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

impl NativeExecutor {
    pub fn new(runtime: tokio::runtime::Handle, io: tokio::runtime::Handle) -> Self {
        Self { runtime, io }
    }

    /// DOC-15 CP-8：原生 Future 在独立线程池执行，调用方等待结果或取消。
    pub async fn run<F, T>(&self, future: F, cancellable: bool) -> Result<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        self.start(future, cancellable).await
    }

    pub fn start<F, T>(&self, future: F, cancellable: bool) -> NativeTask<T>
    where
        F: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let task = self.runtime.spawn(future);
        let cancellation = cancellable.then(|| Cancellation(task.abort_handle()));
        NativeTask { task, cancellation }
    }

    /// HostV1允许插件从运行时任务和系统线程调用同步方法。
    pub fn wait<F: Future>(&self, future: F) -> F::Output {
        tokio::task::block_in_place(|| self.io.block_on(future))
    }

    /// DOC-15 CP-1：最终析构交回原生执行器，协议运行时继续处理控制消息。
    pub fn own<T: Send + 'static>(&self, value: T) -> Owned<T> {
        Owned {
            value: Some(value),
            runtime: self.runtime.clone(),
        }
    }
}

#[derive(Clone)]
pub struct Cancellation(tokio::task::AbortHandle);

impl Cancellation {
    pub fn cancel(&self) {
        self.0.abort();
    }
}

pub struct NativeTask<T> {
    task: tokio::task::JoinHandle<Result<T>>,
    cancellation: Option<Cancellation>,
}

impl<T> NativeTask<T> {
    pub fn cancellation(&self) -> Option<Cancellation> {
        self.cancellation.clone()
    }
}

impl<T> Future for NativeTask<T> {
    type Output = Result<T>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.task).poll(context) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(error)) if error.is_cancelled() => {
                Poll::Ready(Err(Error::new(7, "原生调用已取消")))
            }
            Poll::Ready(Err(error)) => {
                Poll::Ready(Err(Error::new(5, format!("原生执行任务失败：{error}"))))
            }
        }
    }
}

pub struct Owned<T: Send + 'static> {
    value: Option<T>,
    runtime: tokio::runtime::Handle,
}

impl<T: Send + 'static> Deref for Owned<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.value.as_ref().expect("原生所有者保持其值")
    }
}

impl<T: Send + 'static> Drop for Owned<T> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            self.runtime.spawn(async move {
                drop(value);
                crate::lease::reap_ready();
            });
        }
    }
}

impl<T> Drop for NativeTask<T> {
    fn drop(&mut self) {
        if let Some(cancellation) = &self.cancellation {
            cancellation.cancel();
        }
    }
}
