//! Web assembly reads and releases the provider actually selected by its kernel.

use cordis::{Clock, Context, FiberState, Realm, Spawn, Task};
use cordis_supervisor::manager::{InstanceSpec, Manager, ManagerOptions};
use cordis_up::{
    Value, WireError, payload,
    rpc::{Connection, ConnectionOptions, Handler, HandlerFuture, Outgoing, Payload},
};
use cordis_web_deployment::{Deployment, Spec};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

struct Executor(Instant);
impl Spawn for Executor {
    fn spawn(&self, task: Task) {
        tokio::spawn(task);
    }
}
impl Clock for Executor {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&self, duration: Duration) -> Task {
        Box::pin(tokio::time::sleep(duration))
    }
}

#[derive(Default)]
struct Registry {
    providers: Mutex<HashMap<u64, String>>,
    registrations: Mutex<HashMap<u64, u64>>,
    calls: Mutex<Vec<(String, u64, String)>>,
    components: Mutex<Vec<String>>,
}

fn encoded<T: serde::Serialize>(source: T) -> Value {
    Value::serialized(&source).unwrap()
}

impl Handler for Registry {
    fn request(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing, WireError>> {
        Box::pin(async move {
            match method.as_str() {
                "hello" => Ok(encoded(payload::Describe {
                    protocol: 1,
                    plugins: vec![payload::Plugin {
                        name: "registry".into(),
                        version: "1.0.0".into(),
                        sdk_version: "1.0.0".into(),
                        build_info: String::new(),
                        format: 2,
                        library: None,
                        components: vec![payload::Component {
                            name: "registry".into(),
                            inject: vec![],
                            provides: vec!["cordis.web@1.1".into()],
                            listens: vec![],
                            state_schema: String::new(),
                            state_version_min: 0,
                            state_version_max: 0,
                        }],
                    }],
                })
                .into()),
                "resolve" => Ok(Value::Null.into()),
                "apply" => {
                    let apply: payload::Apply = params.value().deserialized().unwrap();
                    let label: String = apply.config.deserialized().unwrap();
                    self.providers.lock().insert(apply.context, label);
                    peer.request(
                        "provide",
                        encoded(payload::Provide {
                            context: apply.context,
                            interface: "cordis.web@1.1".into(),
                            object: apply.context,
                            methods: vec![
                                "register".into(),
                                "activate".into(),
                                "unregister".into(),
                            ],
                            scoped: false,
                            refs: vec![apply.context],
                        }),
                        Duration::from_secs(2),
                    )
                    .await?;
                    Ok(Value::Null.into())
                }
                "invoke" => {
                    let invoke: payload::Invoke = params.value().deserialized().unwrap();
                    let label = self
                        .providers
                        .lock()
                        .get(&invoke.object)
                        .cloned()
                        .ok_or_else(|| WireError::new(4, "provider released"))?;
                    self.calls
                        .lock()
                        .push((label, invoke.object, invoke.method.clone()));
                    let result = match invoke.method.as_str() {
                        "register" => {
                            let component = invoke.args[0]
                                .as_map()
                                .unwrap()
                                .iter()
                                .find(|(key, _)| key.as_text() == Some("component"))
                                .unwrap()
                                .1
                                .as_text()
                                .unwrap()
                                .to_owned();
                            self.components.lock().push(component);
                            let sequence = invoke.args[0]
                                .as_map()
                                .unwrap()
                                .iter()
                                .find(|(key, _)| key.as_text() == Some("sequence"))
                                .unwrap()
                                .1
                                .as_integer()
                                .and_then(|value| u64::try_from(value).ok())
                                .unwrap();
                            assert!(
                                self.registrations
                                    .lock()
                                    .insert(sequence, invoke.object)
                                    .is_none()
                            );
                            Value::from(sequence)
                        }
                        "activate" => {
                            let sequence: u64 = invoke.args[0].deserialized().unwrap();
                            assert_eq!(
                                self.registrations.lock().get(&sequence),
                                Some(&invoke.object)
                            );
                            Value::Null
                        }
                        "unregister" => {
                            let sequence: u64 = invoke.args[0].deserialized().unwrap();
                            assert_eq!(
                                self.registrations.lock().remove(&sequence),
                                Some(invoke.object)
                            );
                            Value::Null
                        }
                        _ => return Err(WireError::new(3, "unknown registry method")),
                    };
                    Ok(encoded(payload::InvokeResult {
                        value: result,
                        refs: vec![],
                    })
                    .into())
                }
                _ => Err(WireError::new(
                    3,
                    format!("unknown fixture method {method}"),
                )),
            }
        })
    }

    fn notify(
        &self,
        _: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<(), WireError>> {
        Box::pin(async move {
            match method.as_str() {
                "release" => {
                    let release: payload::Release = params.value().deserialized().unwrap();
                    for object in release.objects {
                        self.providers.lock().remove(&object);
                    }
                }
                "release_context" => {}
                _ => return Err(WireError::new(3, "unknown fixture notification")),
            }
            Ok(())
        })
    }
}

fn instance(name: &str, config: &str) -> InstanceSpec {
    InstanceSpec {
        name: name.into(),
        group: "http".into(),
        plugin: "registry".into(),
        component: "registry".into(),
        config: Value::from(config),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn isolated_web_owner_registers_and_cleans_its_committed_http_provider() {
    verify_binding(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_stack_owner_uses_its_committed_backend_generation_and_http_provider() {
    verify_binding(true).await;
}

async fn verify_binding(full_stack: bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        let root = Context::new(Arc::new(Executor(Instant::now())));
        let manager = Arc::new(
            Manager::new(
                root.clone(),
                ManagerOptions {
                    confirm: Duration::ZERO,
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
        );
        let registry = Arc::new(Registry::default());
        let (core, host) = tokio::io::duplex(64 * 1024);
        let (read, write) = tokio::io::split(core);
        let core = Connection::attach(
            manager.reserve_connection_id(),
            read,
            write,
            ConnectionOptions::default(),
            manager.clone(),
        )
        .unwrap();
        let (read, write) = tokio::io::split(host);
        let host = Connection::attach(
            1,
            read,
            write,
            ConnectionOptions::default(),
            registry.clone(),
        )
        .unwrap();
        manager
            .attach_connection("http".into(), core)
            .await
            .unwrap();
        manager
            .mount(instance("global-http", "global"), None)
            .await
            .unwrap()
            .settled()
            .await
            .unwrap();
        let private = manager
            .context()
            .isolate_named("cordis.web@1", Realm::new());
        manager
            .mount_in(
                private.clone(),
                instance("private-http", "private-v1"),
                None,
            )
            .await
            .unwrap()
            .settled()
            .await
            .unwrap();
        let deployment = Deployment::new(manager.clone(), Arc::new(|_| false)).unwrap();
        let owner = deployment
            .mount(Spec {
                view: private,
                instance: "panel".into(),
                name: "panel".into(),
                version: "1.0.0".into(),
                assets: ".".into(),
                mount: "/panel".into(),
                entry: "index.js".into(),
                component: "panel".into(),
                parent: full_stack.then(|| "private-http".into()),
                config: Value::Null,
            })
            .unwrap();
        owner.settled().await.unwrap();
        assert_eq!(
            registry
                .calls
                .lock()
                .iter()
                .map(|(label, _, method)| (label.clone(), method.clone()))
                .collect::<Vec<_>>(),
            vec![
                ("private-v1".into(), "register".into()),
                ("private-v1".into(), "activate".into())
            ]
        );
        manager.dispose("global-http").await.unwrap();
        assert_eq!(owner.state(), FiberState::Active);
        let services = manager.context().service_count();
        const GENERATIONS: usize = 16;
        for generation in 2..=GENERATIONS {
            manager
                .update(
                    "private-http",
                    Value::from(format!("private-v{generation}")),
                )
                .await
                .unwrap();
            owner.settled().await.unwrap();
            assert_eq!(manager.context().service_count(), services);
            assert_eq!(registry.registrations.lock().len(), 1);
        }
        deployment.remove("panel").await.unwrap();
        let calls = registry.calls.lock().clone();
        let expected = (1..=GENERATIONS)
            .flat_map(|generation| {
                ["register", "activate", "unregister"]
                    .map(|method| (format!("private-v{generation}"), method.to_owned()))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            calls
                .iter()
                .map(|(label, _, method)| (label.clone(), method.clone()))
                .collect::<Vec<_>>(),
            expected
        );
        for generation in calls.chunks_exact(3) {
            assert_eq!(generation[0].1, generation[1].1);
            assert_eq!(generation[0].1, generation[2].1);
        }
        let objects = calls
            .iter()
            .map(|call| call.1)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(objects.len(), GENERATIONS);
        assert_eq!(
            *registry.components.lock(),
            vec![if full_stack { "registry/web" } else { "panel" }.to_owned(); GENERATIONS]
        );
        assert!(registry.registrations.lock().is_empty());
        manager.shutdown().await.unwrap();
        host.terminate("test complete");
        root.fiber().dispose().await.unwrap();
    })
    .await
    .expect("web binding lifecycle completes");
}
