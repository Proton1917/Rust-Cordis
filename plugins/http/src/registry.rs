//! DOC-15 §11.3：活动入口与版本资源按登记句柄发布和撤回。

use axum::{
    body::Body,
    http::{Request, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use cordis_sdk::{Error, Result};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Component, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tower::ServiceExt;
use tower_http::services::ServeDir;
use web_api::WebRegistration;

#[derive(Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub version: String,
    pub url: String,
    pub component: String,
    pub mount: String,
}
#[derive(Clone)]
pub(super) struct Registration {
    pub info: WebRegistration,
    root: PathBuf,
    prefix: String,
}
#[derive(Default)]
pub(super) struct State {
    pub versions: HashMap<u64, Registration>,
    active: BTreeMap<String, u64>,
    pub routes: BTreeMap<String, super::routes::Slot>,
}
pub struct Registry {
    pub(super) pages: std::sync::Arc<super::page::Pages>,
    pub(super) next: AtomicU64,
    pub(super) state: Mutex<State>,
    pub(super) changed: tokio::sync::watch::Sender<()>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            pages: std::sync::Arc::new(super::page::Pages::default()),
            next: AtomicU64::new(0),
            state: Mutex::new(State::default()),
            changed: tokio::sync::watch::channel(()).0,
        }
    }
}

impl Registry {
    /// ADR-0041：路径在资源登记边界规范化，实际静态协议由 tower-http 执行。
    pub fn register(&self, info: WebRegistration) -> Result<u64> {
        let root = PathBuf::from(&info.assets)
            .canonicalize()
            .map_err(super::server::problem)?;
        let entry = PathBuf::from(&info.entry);
        if !root.is_dir()
            || info.name.is_empty()
            || info.sequence == 0
            || !info.mount.starts_with('/')
            || entry
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(Error::new(2, "网页登记需要有效名称、版本路径与挂载路径"));
        }
        let file = root
            .join(&entry)
            .canonicalize()
            .map_err(super::server::problem)?;
        if !file.is_file() || !file.starts_with(&root) {
            return Err(Error::new(2, "网页入口需要位于资源目录内"));
        }
        let prefix = format!("/_cordis/{}/{}/", info.name, info.sequence);
        let mut state = self.state.lock().unwrap();
        if state.routes.iter().any(|(prefix, slot)| {
            matches!(slot, super::routes::Slot::Active(_))
                && super::routes::overlaps(prefix, &info.mount)
        }) {
            return Err(Error::new(2, "网页挂载路径与 HTTP 路由重叠"));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        state
            .versions
            .insert(id, Registration { info, root, prefix });
        Ok(id)
    }

    pub fn activate(&self, id: u64) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let name = state
            .versions
            .get(&id)
            .ok_or_else(|| Error::new(4, "网页登记已撤回"))?
            .info
            .name
            .clone();
        state.active.insert(name, id);
        Ok(())
    }

    pub fn unregister(&self, id: u64) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        state.versions.remove(&id);
        state.active.retain(|_, current| *current != id);
        Ok(())
    }

    pub fn entries(&self) -> Vec<Entry> {
        let state = self.state.lock().unwrap();
        state
            .active
            .values()
            .filter_map(|id| state.versions.get(id))
            .map(|registration| Entry {
                name: registration.info.name.clone(),
                version: registration.info.version.clone(),
                url: format!("{}{}", registration.prefix, registration.info.entry),
                component: registration.info.component.clone(),
                mount: registration.info.mount.clone(),
            })
            .collect()
    }

    /// DOC-15 §11.3：版本路由始终引用对应目录，活动 mount 路由指向当前版本。
    pub async fn serve(&self, mut request: Request<Body>) -> Response {
        let path = request.uri().path();
        let selected = {
            let state = self.state.lock().unwrap();
            state
                .versions
                .values()
                .find_map(|entry| {
                    path.strip_prefix(&entry.prefix)
                        .map(|tail| (entry.root.clone(), tail.to_owned()))
                })
                .or_else(|| {
                    state
                        .active
                        .values()
                        .filter_map(|id| state.versions.get(id))
                        .find_map(|entry| {
                            path.strip_prefix(&format!(
                                "{}/",
                                entry.info.mount.trim_end_matches('/')
                            ))
                            .map(|tail| (entry.root.clone(), tail.to_owned()))
                        })
                })
        };
        let Some((root, tail)) = selected else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let uri = match Uri::builder().path_and_query(format!("/{tail}")).build() {
            Ok(uri) => uri,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        };
        *request.uri_mut() = uri;
        match ServeDir::new(root).oneshot(request).await {
            Ok(response) => response.into_response(),
            Err(problem) => match problem {},
        }
    }
}
