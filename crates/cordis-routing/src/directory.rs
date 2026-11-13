//! DOC-16 §6、CP-3/CP-6：按接收连接分配出口编号并在锁外释放路由。

use crate::{Result, error};
use cordis_up::{
    Value,
    rpc::{Connection, Outgoing},
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
    time::Duration,
};

struct Entry<R> {
    route: Arc<R>,
    owner: Option<u64>,
}
struct Counter {
    next: u64,
    active: usize,
}
struct Entries<R> {
    values: HashMap<(u64, u64), Entry<R>>,
    counters: HashMap<u64, Counter>,
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
    fn remove(&mut self, recipient: u64, ids: &[u64]) -> Vec<Entry<R>> {
        let discarded: Vec<_> = ids
            .iter()
            .filter_map(|id| self.values.remove(&(recipient, *id)))
            .collect();
        if let Some(counter) = self.counters.get_mut(&recipient) {
            counter.active -= discarded.len();
        }
        discarded
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

impl<R: Send + Sync + 'static> Router<R> {
    /// ADR-0041：调用方提供既定连接预算；路由数据保持调用方的具体类型。
    pub fn new(max_exports: usize, timeout: Duration) -> Self {
        Self {
            directory: Arc::new(Directory {
                entries: Mutex::new(Entries {
                    values: HashMap::new(),
                    counters: HashMap::new(),
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
            .values
            .get(&(recipient, id))
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
        let mut entries = self.directory.entries.lock();
        if !recipient.is_alive() {
            return Err(error(4, "接收连接已结束"));
        }
        let counter = entries
            .counters
            .entry(recipient.id())
            .or_insert(Counter { next: 1, active: 0 });
        if counter.active >= self.directory.max_exports {
            return Err(error(8, "该连接的活跃出口资源已达上限"));
        }
        let id = counter.next;
        counter.next = counter
            .next
            .checked_add(1)
            .ok_or_else(|| error(8, "出口编号空间已耗尽"))?;
        counter.active += 1;
        entries
            .values
            .insert((recipient.id(), id), Entry { route, owner });
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
                .counters
                .get(&recipient)
                .map_or(1, |counter| counter.next);
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
            let ids: Vec<_> = entries
                .values
                .iter()
                .filter_map(|(id, entry)| {
                    (id.0 == connection || entry.owner == Some(connection)).then_some(*id)
                })
                .collect();
            let discarded: Vec<_> = ids
                .iter()
                .filter_map(|id| entries.values.remove(id).map(|entry| (id.0, entry)))
                .collect();
            for (recipient, _) in &discarded {
                if let Some(counter) = entries.counters.get_mut(recipient) {
                    counter.active -= 1;
                }
            }
            entries.counters.remove(&connection);
            discarded
        };
        drop(discarded);
    }
}
