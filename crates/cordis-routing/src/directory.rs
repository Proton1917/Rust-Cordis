//! DOC-16 §6、CP-3/CP-6：按接收连接分配出口编号并在锁外释放路由。

use crate::{Result, Retirement, RouteLifetime, error};
use cordis_up::{
    Value,
    rpc::{Connection, Outgoing},
};
use parking_lot::Mutex;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Weak},
    time::Duration,
};

struct Entry<R> {
    route: Arc<R>,
    owner: Option<u64>,
    lifetime: Option<Retirement>,
}
struct Recipient<R> {
    next: u64,
    routes: HashMap<u64, Entry<R>>,
}
struct Entries<R> {
    recipients: HashMap<u64, Recipient<R>>,
    lifetimes: HashMap<usize, HashSet<(u64, u64)>>,
}
struct Directory<R> {
    entries: Mutex<Entries<R>>,
    max_exports: usize,
}

pub struct Router<R> {
    directory: Arc<Directory<R>>,
    pub(crate) timeout: Duration,
}

pub struct Batch<R> {
    directory: Weak<Directory<R>>,
    recipient: u64,
    pub ids: Vec<u64>,
    committed: bool,
}

impl<R> Entries<R> {
    fn remove_one(&mut self, key: (u64, u64)) -> Option<Entry<R>> {
        let entry = self.recipients.get_mut(&key.0)?.routes.remove(&key.1)?;
        if let Some(lifetime) = &entry.lifetime {
            let id = lifetime.id();
            if let Some(keys) = self.lifetimes.get_mut(&id) {
                keys.remove(&key);
                if keys.is_empty() {
                    self.lifetimes.remove(&id);
                }
            }
        }
        Some(entry)
    }

    fn remove(&mut self, recipient: u64, ids: &[u64]) -> Vec<Entry<R>> {
        ids.iter()
            .filter_map(|id| self.remove_one((recipient, *id)))
            .collect()
    }
}

impl<R> Drop for Batch<R> {
    fn drop(&mut self) {
        if !self.committed
            && let Some(directory) = self.directory.upgrade()
        {
            let discarded = directory.entries.lock().remove(self.recipient, &self.ids);
            drop(discarded);
        }
    }
}

impl<R: Send + Sync + 'static> Batch<R> {
    /// DOC-16 §6：发送许可取得后提交本批出口，发送前取消归还全部持有。
    pub fn outgoing(self, value: Value) -> Outgoing {
        Outgoing::with_commit(value, move || {
            let mut batch = self;
            batch.committed = true;
        })
    }
}

impl<R: RouteLifetime + Send + Sync + 'static> Router<R> {
    /// ADR-0041：调用方提供既定连接预算；路由数据保持调用方的具体类型。
    pub fn new(max_exports: usize, timeout: Duration) -> Self {
        Self {
            directory: Arc::new(Directory {
                entries: Mutex::new(Entries {
                    recipients: HashMap::new(),
                    lifetimes: HashMap::new(),
                }),
                max_exports,
            }),
            timeout,
        }
    }

    pub fn route(&self, recipient: u64, id: u64) -> Result<Arc<R>> {
        self.directory
            .entries
            .lock()
            .recipients
            .get(&recipient)
            .and_then(|peer| peer.routes.get(&id))
            .map(|entry| entry.route.clone())
            .ok_or_else(|| error(4, "对象编号已退役或属于另一条连接"))
    }

    /// DOC-16 §6、CP-3：出口编号和额度在同一目录变更中分配。
    pub fn export(
        &self,
        recipient: &Connection,
        route: Arc<R>,
        owner: Option<u64>,
        batch: &mut Batch<R>,
    ) -> Result<u64> {
        let lifetime = route.retirement();
        let mut entries = self.directory.entries.lock();
        if lifetime.as_ref().is_some_and(Retirement::ended) {
            return Err(error(4, "服务提供方加载纪元已结束"));
        }
        if !recipient.is_alive() {
            return Err(error(4, "接收连接已结束"));
        }
        let peer = entries
            .recipients
            .entry(recipient.id())
            .or_insert_with(|| Recipient {
                next: 1,
                routes: HashMap::new(),
            });
        if peer.routes.len() >= self.directory.max_exports {
            return Err(error(8, "该连接的活跃出口资源已达上限"));
        }
        let id = peer.next;
        peer.next = peer
            .next
            .checked_add(1)
            .ok_or_else(|| error(8, "出口编号空间已耗尽"))?;
        let lifetime_id = lifetime.as_ref().map(Retirement::id);
        peer.routes.insert(
            id,
            Entry {
                route,
                owner,
                lifetime,
            },
        );
        if let Some(lifetime_id) = lifetime_id {
            entries
                .lifetimes
                .entry(lifetime_id)
                .or_default()
                .insert((recipient.id(), id));
        }
        batch.ids.push(id);
        Ok(id)
    }

    pub fn batch(&self, recipient: u64) -> Batch<R> {
        Batch {
            directory: Arc::downgrade(&self.directory),
            recipient,
            ids: Vec::new(),
            committed: false,
        }
    }

    /// DOC-16 §6：已分配范围内的缺席编号按幂等释放处理。
    pub fn release(&self, recipient: u64, ids: &[u64]) -> Result<()> {
        let discarded = {
            let mut entries = self.directory.entries.lock();
            let next = entries
                .recipients
                .get(&recipient)
                .map_or(1, |peer| peer.next);
            for id in ids {
                if *id == 0 || *id >= next {
                    return Err(error(4, "release编号属于该连接已分配范围之外"));
                }
            }
            entries.remove(recipient, ids)
        };
        drop(discarded);
        Ok(())
    }

    /// CP-6：关闭接收端或来源端时退役受影响路由，其他连接保留原编号计数。
    pub fn reclaim(&self, connection: u64) {
        let discarded = {
            let mut entries = self.directory.entries.lock();
            let mut ids = Vec::new();
            for (&recipient, peer) in &entries.recipients {
                for (&id, entry) in &peer.routes {
                    if recipient == connection || entry.owner == Some(connection) {
                        ids.push((recipient, id));
                    }
                }
            }
            let discarded: Vec<_> = ids
                .into_iter()
                .filter_map(|key| entries.remove_one(key))
                .collect();
            entries.recipients.remove(&connection);
            discarded
        };
        drop(discarded);
    }

    /// ADR-0052：按加载代索引退役出口，编号继续由 release 的幂等范围识别。
    pub fn retire(&self, lifetime: &Retirement) {
        lifetime.finish();
        let discarded = {
            let mut entries = self.directory.entries.lock();
            let keys = entries.lifetimes.remove(&lifetime.id()).unwrap_or_default();
            let mut discarded = Vec::with_capacity(keys.len());
            discarded.extend(keys.into_iter().filter_map(|(recipient, id)| {
                entries.recipients.get_mut(&recipient)?.routes.remove(&id)
            }));
            discarded
        };
        drop(discarded);
    }
}
