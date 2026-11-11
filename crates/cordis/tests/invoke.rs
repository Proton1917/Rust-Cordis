use cordis::{Context, Inject, Merge, Scoped, ServiceKey};
use cordis_testkit::TestExecutor;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Default, Debug, PartialEq, Eq)]
struct Config(BTreeMap<&'static str, usize>);
impl Merge for Config {
    fn merge(&self, next: &Self) -> Self {
        let mut values = self.0.clone();
        values.extend(next.0.clone());
        Self(values)
    }
}
fn config(key: &'static str, value: usize) -> Config {
    Config(BTreeMap::from([(key, value)]))
}
struct Callable;
impl ServiceKey for Callable {
    const NAME: &'static str = "callable";
    type Value = Config;
    type Meta = Config;
}
struct Dependency;
impl ServiceKey for Dependency {
    const NAME: &'static str = "dependency";
    type Value = usize;
    type Meta = ();
}
struct Outer;
impl ServiceKey for Outer {
    const NAME: &'static str = "outer";
    type Value = ();
    type Meta = ();
}

trait Invoke {
    fn invoke(&self, args: Config) -> Config;
    fn extend(&self, config: Config) -> Self;
}
impl Invoke for Scoped<Callable> {
    fn invoke(&self, args: Config) -> Config {
        self.value().merge(&self.meta()).merge(&args)
    }
    fn extend(&self, config: Config) -> Self {
        self.with_value(Arc::new(self.value().merge(&config)))
    }
}

#[test]
fn inv01_callable_extension_preserves_caller_metadata() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Callable>(Arc::new(config("a", 1))).unwrap();
    assert_eq!(
        root.get::<Callable>().unwrap().invoke(Config::default()),
        config("a", 1)
    );
    let first = root
        .intercept::<Callable>(config("b", 2))
        .get::<Callable>()
        .unwrap();
    assert_eq!(
        first.invoke(Config::default()),
        config("a", 1).merge(&config("b", 2))
    );
    let second = root.get::<Callable>().unwrap().extend(config("c", 3));
    assert_eq!(
        second.invoke(Config::default()),
        config("a", 1).merge(&config("c", 3))
    );
    let third = first.extend(config("d", 4));
    assert_eq!(
        third.invoke(config("a", 9)),
        config("a", 9).merge(&config("b", 2)).merge(&config("d", 4))
    );
    assert_eq!(
        first.invoke(Config::default()),
        config("a", 1).merge(&config("b", 2))
    );
    host.finish(root.fiber().dispose()).unwrap();
}

#[test]
fn inv02_callable_extension_preserves_definition_dependencies() {
    let host = TestExecutor::new();
    let root = Context::new(host.clone());
    root.provide::<Dependency>(Arc::new(42)).unwrap();
    let callable = root
        .inject(Inject::new().with::<Dependency>(), |ctx| async move {
            ctx.provide::<Callable>(Arc::new(Config::default()))?;
            Ok(())
        })
        .unwrap();
    let outer = root
        .inject(Inject::new().with::<Callable>(), |ctx| async move {
            ctx.provide::<Outer>(Arc::new(()))?;
            Ok(())
        })
        .unwrap();
    host.finish(callable.settled()).unwrap();
    host.finish(outer.settled()).unwrap();
    let child = root
        .inject(Inject::new().with::<Outer>(), |ctx| async move {
            let callable = ctx.get::<Outer>()?.dependency::<Callable>()?;
            assert_eq!(*callable.dependency::<Dependency>()?, 42);
            assert_eq!(
                *callable.extend(config("x", 1)).dependency::<Dependency>()?,
                42
            );
            Ok(())
        })
        .unwrap();
    host.finish(child.settled()).unwrap();
    host.finish(root.fiber().dispose()).unwrap();
}
