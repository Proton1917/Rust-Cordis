use cordis::{Context, Merge, Realm, ServiceKey};
use cordis_testkit::TestExecutor;

struct Database;
impl ServiceKey for Database {
    const NAME: &'static str = "database";
    type Value = String;
    type Meta = Permissions;
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
struct Permissions {
    read: bool,
    write: bool,
}
impl Merge for Permissions {
    fn merge(&self, next: &Self) -> Self {
        Self {
            read: self.read || next.read,
            write: self.write || next.write,
        }
    }
}

// S-CTX-2、S-CTX-3、S-CTX-4：子视图覆盖当前键，父级数据保持原值。
#[test]
fn persistent_views() {
    let root = Context::new(TestExecutor::new()).extend(12_u32);
    let child = root.extend(34_u32).extend("child");
    assert_eq!(*root.extension::<u32>().unwrap(), 12);
    assert_eq!(*child.extension::<u32>().unwrap(), 34);
    assert_eq!(*child.extension::<&str>().unwrap(), "child");
    assert!(root.extension::<&str>().is_none());
    let realm = Realm::new();
    let isolated = child.isolate::<Database>(realm);
    assert_eq!(isolated.realm::<Database>(), realm);
    assert_eq!(root.realm::<Database>(), Realm::DEFAULT);
    let outer = isolated.intercept::<Database>(Permissions {
        read: true,
        write: false,
    });
    let inner = outer.intercept::<Database>(Permissions {
        read: false,
        write: true,
    });
    assert_eq!(
        inner.intercept_meta::<Database>(),
        Permissions {
            read: true,
            write: true
        }
    );
    assert_eq!(
        outer.intercept_meta::<Database>(),
        Permissions {
            read: true,
            write: false
        }
    );
    assert_eq!(root.intercept_meta::<Database>(), Permissions::default());
}
