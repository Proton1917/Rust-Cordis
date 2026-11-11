//! 类型化服务键与调用方句柄。

/// T-COE-9：按声明、外层、内层的顺序合并元数据。
pub trait Merge: Clone + Default + Send + Sync + 'static {
    fn merge(&self, next: &Self) -> Self;
}

impl Merge for () {
    fn merge(&self, _: &Self) -> Self {}
}

pub trait ServiceKey: 'static {
    const NAME: &'static str;
    type Value: ?Sized + Send + Sync + 'static;
    type Meta: Merge;
}
