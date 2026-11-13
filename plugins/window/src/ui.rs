//! DOC-15 §11.6、ADR-0037：主线程拥有系统窗口，跨线程请求以实际操作结果应答。

use crate::{Result, error};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget},
    platform::run_return::EventLoopExtRunReturn,
    window::{Window, WindowBuilder},
};
use tokio::sync::{mpsc, oneshot};
use wry::{WebView, WebViewBuilder};

#[derive(Clone, Serialize, Deserialize)]
pub struct WindowSpec {
    pub url: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub entry: bool,
}

pub enum Signal {
    Closed { owner: u64, window: u64, last: bool },
    Quit,
}

enum Request {
    Register(u64, oneshot::Sender<Result<()>>),
    Open(u64, u64, WindowSpec, oneshot::Sender<Result<WindowLease>>),
    Show(u64, oneshot::Sender<Result<()>>),
    Close(u64, Option<oneshot::Sender<Result<()>>>),
    UserClose(u64, oneshot::Sender<Result<bool>>),
    Snapshot(u64, oneshot::Sender<Result<Vec<WindowSpec>>>),
    Thaw(u64, oneshot::Sender<Result<()>>),
    Forget(u64),
    Quit,
    Stop,
}

#[derive(Clone)]
pub struct Bridge(EventLoopProxy<Request>);

/// PR-1：窗口创建应答在调用者取消时负责关闭窗口，登记的清理持有同一份租约。
pub struct WindowLease {
    pub id: u64,
    bridge: Bridge,
}

impl Drop for WindowLease {
    fn drop(&mut self) {
        let _ = self.bridge.0.send_event(Request::Close(self.id, None));
    }
}

impl Bridge {
    async fn call<T>(
        &self,
        create: impl FnOnce(oneshot::Sender<Result<T>>) -> Request,
    ) -> Result<T> {
        let (send, receive) = oneshot::channel();
        self.0.send_event(create(send)).map_err(error)?;
        receive.await.map_err(error)?
    }

    pub async fn register(&self, owner: u64) -> Result<()> {
        self.call(|reply| Request::Register(owner, reply)).await
    }

    pub async fn open(&self, owner: u64, id: u64, spec: WindowSpec) -> Result<WindowLease> {
        self.call(|reply| Request::Open(owner, id, spec, reply))
            .await
    }

    pub async fn show(&self, id: u64) -> Result<()> {
        self.call(|reply| Request::Show(id, reply)).await
    }

    pub async fn close(&self, id: u64) -> Result<()> {
        self.call(|reply| Request::Close(id, Some(reply))).await
    }

    pub async fn user_close(&self, id: u64) -> Result<bool> {
        self.call(|reply| Request::UserClose(id, reply)).await
    }

    pub async fn snapshot(&self, owner: u64) -> Result<Vec<WindowSpec>> {
        self.call(|reply| Request::Snapshot(owner, reply)).await
    }

    pub async fn thaw(&self, owner: u64) -> Result<()> {
        self.call(|reply| Request::Thaw(owner, reply)).await
    }

    pub fn forget(&self, owner: u64) -> Result<()> {
        self.0.send_event(Request::Forget(owner)).map_err(error)
    }

    pub fn stop(&self) {
        let _ = self.0.send_event(Request::Stop);
    }
}

struct View {
    // WebView 先释放，使平台页面在所属窗口关闭前停止。
    _webview: WebView,
    window: Window,
    owner: u64,
    spec: WindowSpec,
    visible: bool,
}

#[derive(Default)]
struct Owner {
    frozen: bool,
    pending: VecDeque<Request>,
    closing: BTreeSet<u64>,
}

struct Desktop {
    windows: BTreeMap<u64, View>,
    owners: BTreeMap<u64, Owner>,
    bridge: Bridge,
    events: mpsc::Sender<Signal>,
}

/// DOC-15 §11.6：GUI 初始化和循环保持主线程，UP/1 工作线程由调用者启动并回收。
pub fn run(
    start: impl FnOnce(Bridge, mpsc::Receiver<Signal>) -> std::thread::JoinHandle<()>,
) -> Result<()> {
    let mut event_loop = EventLoopBuilder::<Request>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
        event_loop.set_dock_visibility(false);
    }
    let bridge = Bridge(event_loop.create_proxy());
    let menu = application_menu(bridge.clone())?;
    let (events, receiver) = mpsc::channel(256);
    let worker = start(bridge.clone(), receiver);
    let mut desktop = Desktop {
        windows: BTreeMap::new(),
        owners: BTreeMap::new(),
        bridge,
        events,
    };
    event_loop.run_return(|event, target, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(Request::Stop) => *flow = ControlFlow::Exit,
            Event::UserEvent(request) => desktop.request(request, target, flow),
            Event::WindowEvent {
                window_id,
                event: WindowEvent::CloseRequested,
                ..
            } => {
                let selected = desktop.windows.iter().find_map(|(id, view)| {
                    (view.window.id() == window_id).then_some((*id, view.owner))
                });
                if let Some((id, owner)) = selected {
                    desktop.close_requested(owner, id, flow);
                }
            }
            _ => {}
        }
    });
    drop(desktop);
    drop(menu);
    worker.join().map_err(|_| error("窗口协议线程退出异常"))
}

impl Desktop {
    fn signal(&self, signal: Signal, flow: &mut ControlFlow) {
        if let Err(problem) = self.events.try_send(signal) {
            eprintln!("窗口事件发送失败：{problem}");
            *flow = ControlFlow::Exit;
        }
    }

    fn request(
        &mut self,
        request: Request,
        target: &EventLoopWindowTarget<Request>,
        flow: &mut ControlFlow,
    ) {
        let owner = match &request {
            Request::Open(owner, ..) => Some(*owner),
            Request::UserClose(id, _) | Request::Show(id, _) => {
                self.windows.get(id).map(|view| view.owner)
            }
            _ => None,
        };
        if let Some(owner) = owner.and_then(|owner| self.owners.get_mut(&owner))
            && owner.frozen
        {
            owner.pending.push_back(request);
            return;
        }
        match request {
            Request::Register(owner, reply) => {
                self.owners.insert(owner, Owner::default());
                let _ = reply.send(Ok(()));
            }
            Request::Open(owner, id, spec, reply) => {
                if reply.is_closed() {
                    return;
                }
                let result = self.open(owner, id, spec, target);
                let _ = reply.send(result);
            }
            Request::Show(id, reply) => {
                let result = self.show(id, target);
                let _ = reply.send(result);
            }
            Request::Close(id, reply) => {
                self.windows.remove(&id);
                if let Some(reply) = reply {
                    let _ = reply.send(Ok(()));
                }
            }
            Request::UserClose(id, reply) => {
                let result = self.user_close(id);
                let _ = reply.send(result);
            }
            Request::Snapshot(owner, reply) => {
                let result = self.freeze(owner);
                let _ = reply.send(result);
            }
            Request::Thaw(owner, reply) => {
                self.thaw(owner, reply, target, flow);
            }
            Request::Forget(owner) => {
                self.windows.retain(|_, view| view.owner != owner);
                self.owners.remove(&owner);
            }
            Request::Quit => self.signal(Signal::Quit, flow),
            Request::Stop => *flow = ControlFlow::Exit,
        }
    }

    fn freeze(&mut self, owner: u64) -> Result<Vec<WindowSpec>> {
        let record = self
            .owners
            .get_mut(&owner)
            .ok_or_else(|| error("窗口上下文已释放"))?;
        record.frozen = true;
        Ok(self
            .windows
            .values()
            .filter(|view| view.owner == owner && view.visible)
            .map(|view| view.spec.clone())
            .collect())
    }

    /// PR-7：异步窗口调用在检查点冻结期间等候，thaw 后按提交次序继续。
    fn thaw(
        &mut self,
        owner: u64,
        reply: oneshot::Sender<Result<()>>,
        target: &EventLoopWindowTarget<Request>,
        flow: &mut ControlFlow,
    ) {
        let Some(record) = self.owners.get_mut(&owner) else {
            let _ = reply.send(Err(error("窗口上下文已释放")));
            return;
        };
        record.frozen = false;
        let pending = std::mem::take(&mut record.pending);
        let closing = std::mem::take(&mut record.closing);
        let _ = reply.send(Ok(()));
        for request in pending {
            self.request(request, target, flow);
        }
        for id in closing {
            self.close_requested(owner, id, flow);
        }
    }

    fn close_requested(&mut self, owner: u64, id: u64, flow: &mut ControlFlow) {
        if let Some(record) = self.owners.get_mut(&owner)
            && record.frozen
        {
            record.closing.insert(id);
            return;
        }
        if self.windows.remove(&id).is_some() {
            let last = self.windows.values().all(|view| view.owner != owner);
            self.signal(
                Signal::Closed {
                    owner,
                    window: id,
                    last,
                },
                flow,
            );
        }
    }

    /// PR-7：显式关闭与状态导出在同一主线程内决定次序，清理使用独立的 Close 操作。
    fn user_close(&mut self, id: u64) -> Result<bool> {
        let view = self.windows.get(&id).ok_or_else(|| error("窗口已关闭"))?;
        let owner = view.owner;
        self.windows.remove(&id);
        Ok(self.windows.values().all(|view| view.owner != owner))
    }

    fn open(
        &mut self,
        owner: u64,
        id: u64,
        spec: WindowSpec,
        target: &EventLoopWindowTarget<Request>,
    ) -> Result<WindowLease> {
        if !self.owners.contains_key(&owner) {
            return Err(error("窗口上下文已释放"));
        }
        let window = WindowBuilder::new()
            .with_visible(false)
            .with_title(&spec.title)
            .with_inner_size(LogicalSize::new(spec.width, spec.height))
            .build(target)
            .map_err(error)?;
        let builder = WebViewBuilder::new()
            .with_incognito(true)
            .with_url(&spec.url);
        #[cfg(target_os = "macos")]
        let webview = builder.build(&window).map_err(error)?;
        #[cfg(target_os = "linux")]
        let webview = {
            use tao::platform::unix::WindowExtUnix;
            use wry::WebViewBuilderExtUnix;
            builder
                .build_gtk(window.default_vbox().expect("窗口 GTK 容器"))
                .map_err(error)?
        };
        self.windows.insert(
            id,
            View {
                _webview: webview,
                window,
                owner,
                spec,
                visible: false,
            },
        );
        Ok(WindowLease {
            id,
            bridge: self.bridge.clone(),
        })
    }

    fn show(&mut self, id: u64, target: &EventLoopWindowTarget<Request>) -> Result<()> {
        let view = self
            .windows
            .get_mut(&id)
            .ok_or_else(|| error("窗口已关闭"))?;
        #[cfg(target_os = "macos")]
        {
            use tao::platform::macos::{ActivationPolicy, EventLoopWindowTargetExtMacOS};
            target.set_activation_policy_at_runtime(ActivationPolicy::Regular);
            target.set_dock_visibility(true);
        }
        view.window.set_visible(true);
        view.visible = true;
        view.window.set_focus();
        Ok(())
    }
}

fn application_menu(bridge: Bridge) -> Result<muda::Menu> {
    use muda::{
        Menu, MenuItem, Submenu,
        accelerator::{Accelerator, Code, Modifiers},
    };
    let quit = MenuItem::new(
        "退出",
        true,
        Some(Accelerator::new(Modifiers::META, Code::KeyQ)),
    );
    let quit_id = quit.id().clone();
    let app = Submenu::with_items("Cordis", true, &[&quit]).map_err(error)?;
    let menu = Menu::with_items(&[&app]).map_err(error)?;
    #[cfg(target_os = "macos")]
    menu.init_for_nsapp();
    muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
        if event.id == quit_id {
            let _ = bridge.0.send_event(Request::Quit);
        }
    }));
    Ok(menu)
}
