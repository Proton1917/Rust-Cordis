//! 支持按句柄移除的有序表。

use std::collections::VecDeque;

pub(crate) struct DisposableList<T>(VecDeque<T>);

impl<T> Default for DisposableList<T> {
    fn default() -> Self {
        Self(VecDeque::new())
    }
}

impl<T: PartialEq> DisposableList<T> {
    pub fn insert(&mut self, value: T, prepend: bool) {
        if prepend {
            self.0.push_front(value);
        } else {
            self.0.push_back(value);
        }
    }

    pub fn remove(&mut self, value: &T) -> bool {
        if let Some(index) = self.0.iter().position(|item| item == value) {
            self.0.remove(index);
            true
        } else {
            false
        }
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.0.iter()
    }
    pub fn take(&mut self) -> VecDeque<T> {
        std::mem::take(&mut self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_tail_and_handle_removal() {
        let mut list = DisposableList::default();
        list.insert(1, false);
        list.insert(2, true);
        list.insert(3, true);
        list.insert(4, false);
        assert_eq!(list.take(), [3, 2, 1, 4]);
        list.insert(5, false);
        list.insert(6, false);
        assert!(list.remove(&5));
        assert!(!list.remove(&5));
        assert_eq!(list.take(), [6]);
    }
}
