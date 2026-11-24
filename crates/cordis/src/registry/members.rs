//! Component membership preserves insertion order and removes an instance by identity.

use crate::kernel::FiberId;
use std::collections::{HashMap, hash_map::Entry};

struct Links {
    previous: Option<FiberId>,
    next: Option<FiberId>,
}

#[derive(Default)]
pub(super) struct Members {
    links: HashMap<FiberId, Links>,
    first: Option<FiberId>,
    last: Option<FiberId>,
}

impl Members {
    pub fn push(&mut self, id: FiberId) {
        let Entry::Vacant(slot) = self.links.entry(id) else {
            return;
        };
        slot.insert(Links {
            previous: self.last,
            next: None,
        });
        if let Some(last) = self.last {
            self.links.get_mut(&last).expect("tail is a member").next = Some(id);
        } else {
            self.first = Some(id);
        }
        self.last = Some(id);
    }

    pub fn remove(&mut self, id: FiberId) {
        let Some(links) = self.links.remove(&id) else {
            return;
        };
        if let Some(previous) = links.previous {
            self.links
                .get_mut(&previous)
                .expect("predecessor is a member")
                .next = links.next;
        } else {
            self.first = links.next;
        }
        if let Some(next) = links.next {
            self.links
                .get_mut(&next)
                .expect("successor is a member")
                .previous = links.previous;
        } else {
            self.last = links.previous;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &FiberId> {
        let mut next = self.first;
        std::iter::from_fn(move || {
            let id = next?;
            let (id, links) = self
                .links
                .get_key_value(&id)
                .expect("successor is a member");
            next = links.next;
            Some(id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slotmap::SlotMap;

    #[test]
    fn removal_and_append_keep_survivor_order_at_each_scale() {
        for count in [100, 1_000, 10_000] {
            let mut slots = SlotMap::<FiberId, ()>::with_key();
            let ids: Vec<_> = (0..count).map(|_| slots.insert(())).collect();
            let mut members = Members::default();
            for &id in &ids {
                members.push(id);
            }
            for &id in ids.iter().step_by(2) {
                members.remove(id);
            }
            assert_eq!(
                members.iter().copied().collect::<Vec<_>>(),
                ids.iter().copied().skip(1).step_by(2).collect::<Vec<_>>()
            );
            for &id in ids.iter().step_by(2) {
                members.push(id);
            }
            let expected: Vec<_> = ids
                .iter()
                .copied()
                .skip(1)
                .step_by(2)
                .chain(ids.iter().copied().step_by(2))
                .collect();
            assert_eq!(members.iter().copied().collect::<Vec<_>>(), expected);
            for &id in &ids {
                members.remove(id);
                members.remove(id);
            }
            assert!(members.is_empty());
            assert!(members.first.is_none());
            assert!(members.last.is_none());
        }
    }
}
