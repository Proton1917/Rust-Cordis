use super::support;

use cordis_abi::*;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

struct ReleaseCounter(Arc<AtomicUsize>);
impl Drop for ReleaseCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn resource(env: Environment, released: Arc<AtomicUsize>) -> Object {
    let retained = ReleaseCounter(released);
    sync_callback(env, move |_, _, _| {
        let _retained = &retained;
        Ok(())
    })
    .unwrap()
}

struct Relay {
    env: Environment,
}

impl Relay {
    fn input(&self, bytes: &[u8]) -> Result<Object> {
        let [value]: [CborValue; 1] = decode_arguments(bytes)?
            .try_into()
            .map_err(|_| Error::new(2, "echo expects one resource"))?;
        self.env.host().import_resource(&value)
    }
    fn output(&self, object: Object) -> Result<Buf> {
        let mut encoder = CborEncoder::new(self.env);
        let value = encoder.export(object)?;
        encoder.finish(value)
    }
}

impl GenericService for Relay {
    fn invoke(&self, method: &str, args: &[u8]) -> Result<Buf> {
        match method {
            "echo" => self.output(self.input(args)?),
            "delayed" => Err(Error::new(2, "delayed uses the asynchronous entry")),
            _ => Err(Error::new(3, "unknown method")),
        }
    }
    fn invoke_async(&self, method: &str, args: &[u8]) -> Result<InvokeFuture> {
        if method == "echo" {
            let output = self.invoke(method, args)?;
            return make_invoke_future(std::future::ready(Ok(output)), self.env);
        }
        if method != "delayed" {
            return Err(Error::new(3, "unknown method"));
        }
        let object = self.input(args)?;
        let env = self.env;
        make_invoke_future(
            async move {
                let mut yielded = false;
                std::future::poll_fn(move |cx| {
                    if yielded {
                        Poll::Ready(())
                    } else {
                        yielded = true;
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                })
                .await;
                Relay { env }.output(object)
            },
            env,
        )
    }
}

struct WakeCount(AtomicUsize);
impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn payload(env: Environment, object: Object) -> Buf {
    let mut encoder = CborEncoder::new(env);
    let resource = encoder.export(object).unwrap();
    encoder.finish(CborValue::Array(vec![resource])).unwrap()
}

#[test]
fn generic_payload_owners_keep_resources_alive_through_sync_async_and_cancellation() {
    let producer = support::environment();
    let consumer = support::environment();
    let released = Arc::new(AtomicUsize::new(0));
    let proxy = generic_object(
        "resource-relay@1.0".into(),
        vec!["echo".into(), "delayed".into()],
        Arc::new(Relay { env: producer }),
        producer,
    )
    .unwrap();
    assert!(!proxy.has_typed_methods());
    assert_eq!(proxy.method_names().unwrap(), ["echo", "delayed"]);
    let input = payload(consumer, resource(producer, released.clone()));
    let output = proxy.invoke("echo", input.as_slice()).unwrap();
    drop(input);
    let value = decode_value(output.as_slice()).unwrap();
    let imported = consumer.host().import_resource(&value).unwrap();
    drop(output);
    assert_eq!(released.load(Ordering::SeqCst), 0);
    imported.call_sync(0, 0, &[]).unwrap();
    imported.release().unwrap();
    assert_eq!(released.load(Ordering::SeqCst), 1);

    let input = payload(consumer, resource(producer, released.clone()));
    let mut future = proxy.invoke_async("delayed", input.as_slice()).unwrap();
    drop(input);
    let notifications = Arc::new(WakeCount(AtomicUsize::new(0)));
    let waker = Waker::from(notifications.clone());
    let mut context = Context::from_waker(&waker);
    assert!(Pin::new(&mut future).poll(&mut context).is_pending());
    let output = match Pin::new(&mut future).poll(&mut context) {
        Poll::Ready(result) => result.unwrap(),
        Poll::Pending => panic!("second poll should complete the deferred resource"),
    };
    assert!(notifications.0.load(Ordering::SeqCst) > 0);
    let imported = consumer
        .host()
        .import_resource(&decode_value(output.as_slice()).unwrap())
        .unwrap();
    drop((future, output));
    assert_eq!(released.load(Ordering::SeqCst), 1);
    imported.call_sync(0, 0, &[]).unwrap();
    drop(imported);
    assert_eq!(released.load(Ordering::SeqCst), 2);

    let input = payload(consumer, resource(producer, released.clone()));
    let future = proxy.invoke_async("delayed", input.as_slice()).unwrap();
    drop((input, future));
    assert_eq!(released.load(Ordering::SeqCst), 3);
    let empty = CborEncoder::new(consumer)
        .finish(CborValue::Array(Vec::new()))
        .unwrap();
    assert_eq!(
        proxy
            .invoke("missing", empty.as_slice())
            .err()
            .unwrap()
            .code(),
        3
    );
    drop((empty, proxy));
    assert_eq!(support::counts(producer).active, 0);
    assert_eq!(support::counts(consumer).active, 0);
}
