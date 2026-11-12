use cordis_up::{
    Value, WireError,
    rpc::{Connection, ConnectionOptions, Handler, HandlerFuture, Outgoing, Payload},
};
use std::{sync::Arc, time::Duration};

struct Arithmetic;

impl Handler for Arithmetic {
    fn request(
        &self,
        _: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing, WireError>> {
        Box::pin(async move {
            match method.as_str() {
                "bytes" => {
                    let length = usize::try_from(
                        params
                            .value()
                            .as_integer()
                            .ok_or_else(|| WireError::new(2, "length requires integer"))?,
                    )
                    .map_err(|problem| WireError::new(2, problem.to_string()))?;
                    Ok(Value::Bytes(vec![3; length]).into())
                }
                "sum" => {
                    let values = params
                        .value()
                        .as_array()
                        .ok_or_else(|| WireError::new(2, "sum requires array"))?;
                    let sum = values.iter().try_fold(0_u64, |sum, value| {
                        let value = value
                            .as_integer()
                            .and_then(|value| u64::try_from(value).ok())
                            .ok_or_else(|| WireError::new(2, "sum requires u64"))?;
                        sum.checked_add(value)
                            .ok_or_else(|| WireError::new(2, "sum exceeds u64"))
                    })?;
                    Ok(Value::Integer(sum.into()).into())
                }
                _ => Err(WireError::new(3, "unknown method")),
            }
        })
    }
    fn notify(
        &self,
        _: Connection,
        _: String,
        _: Payload,
    ) -> HandlerFuture<'_, Result<(), WireError>> {
        Box::pin(async { Err(WireError::new(3, "unknown notification")) })
    }
}

// CP-2、CP-3：本地编码容量错误保持接收连接，响应错误也通过同一条真实字节管道报告。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_encoding_limit_preserves_the_connection() {
    let (first, second) = tokio::io::duplex(4096);
    let (first_read, first_write) = tokio::io::split(first);
    let (second_read, second_write) = tokio::io::split(second);
    let mut options = ConnectionOptions::default();
    options.limits.max_frame = 1024;
    let first = Connection::attach(
        1,
        first_read,
        first_write,
        options.clone(),
        Arc::new(Arithmetic),
    )
    .unwrap();
    let second =
        Connection::attach(2, second_read, second_write, options, Arc::new(Arithmetic)).unwrap();

    let error = first
        .try_notify("data", Value::Bytes(vec![5; 2048]))
        .unwrap_err();
    assert!(
        first.is_alive(),
        "local encoding failure closed a healthy peer"
    );
    assert_eq!(error.code, 8);
    let error = first
        .request(
            "bytes",
            Value::Integer(2048_u64.into()),
            Duration::from_secs(2),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, 8);
    assert!(first.is_alive() && second.is_alive());
    let sum = first
        .request(
            "sum",
            Value::Array(vec![3_u64.into(), 4_u64.into()]),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
    assert_eq!(
        sum.value()
            .as_integer()
            .and_then(|value| u64::try_from(value).ok()),
        Some(7)
    );
    first.terminate("test completed");
    second.closed().await;
}
