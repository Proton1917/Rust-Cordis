use cordis_up::{
    Value, WireError,
    rpc::{Connection, ConnectionOptions, Handler, HandlerFuture, Outgoing, Payload},
};
use std::{sync::Arc, time::Duration};

struct Arithmetic;

impl Handler for Arithmetic {
    fn request(
        &self,
        peer: Connection,
        method: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing, WireError>> {
        Box::pin(async move {
            match method.as_str() {
                "hello" => Ok(Value::Map(vec![("protocol".into(), 1_u64.into())]).into()),
                "invoke" => {
                    let fields = params
                        .value()
                        .as_map()
                        .ok_or_else(|| WireError::new(2, "invoke requires fields"))?;
                    if fields.iter().any(|(key, value)| {
                        key.as_text() == Some("method") && value.as_text() == Some("relay")
                    }) {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        let reply = peer
                            .request(
                                "invoke",
                                Value::Map(vec![("method".into(), "leaf".into())]),
                                Duration::from_secs(5),
                            )
                            .await?;
                        return Ok(reply.value().clone().into());
                    }
                    let budget = fields.iter().find_map(|(key, value)| {
                        (key.as_text() == Some("budget_ms")).then_some(value)
                    });
                    Ok(budget.cloned().unwrap_or(Value::Null).into())
                }
                "close" => Ok(Outgoing::new(Value::Null)
                    .after_write(move || peer.terminate("response delivered"))),
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

#[tokio::test]
async fn negotiated_invoke_forwards_remaining_budget_to_a_nested_peer() {
    let (caller, provider) = tokio::io::duplex(4096);
    let (read, write) = tokio::io::split(caller);
    let caller = Connection::attach(
        1,
        read,
        write,
        ConnectionOptions::default(),
        Arc::new(Arithmetic),
    )
    .unwrap();
    let (read, write) = tokio::io::split(provider);
    let provider = Connection::attach(
        2,
        read,
        write,
        ConnectionOptions::default(),
        Arc::new(Arithmetic),
    )
    .unwrap();
    let invoke = || Value::Map(vec![("method".into(), "relay".into())]);
    let deadline = Duration::from_secs(1);
    let legacy = caller.request("invoke", invoke(), deadline).await.unwrap();
    assert_eq!(legacy.value(), &Value::Null);

    caller
        .request("hello", Value::Null, deadline)
        .await
        .unwrap();
    let nested = caller.request("invoke", invoke(), deadline).await.unwrap();
    let remaining = u64::try_from(nested.value().as_integer().unwrap()).unwrap();
    assert!(
        (1..=980).contains(&remaining),
        "跨过 20 ms 的嵌套调用收到 {remaining} ms"
    );
    caller.terminate("test completed");
    provider.terminate("test completed");
}

// 写端已满时，排队请求到期只结束该调用。随后重新开放管道，已有消息仍然到达。
#[tokio::test]
async fn timeout_before_enqueue_preserves_the_connection() {
    use tokio::io::AsyncReadExt;
    let (mut reader, server) = tokio::io::duplex(1);
    let (read, write) = tokio::io::split(server);
    let peer = Connection::attach(
        1,
        read,
        write,
        ConnectionOptions {
            queue_capacity: 1,
            ..Default::default()
        },
        Arc::new(Arithmetic),
    )
    .unwrap();
    for value in [1_u64, 2] {
        peer.notify("tick", Value::from(value)).await.unwrap();
    }
    let expired = peer
        .request("sum", Value::Array(vec![]), Duration::from_millis(20))
        .await;
    assert_eq!(expired.err().unwrap().code, 6);
    assert!(peer.is_alive());
    tokio::time::timeout(Duration::from_secs(3), async {
        for value in [1_u64, 2] {
            let size = reader.read_u32().await.unwrap() as usize;
            let mut body = vec![0; size];
            reader.read_exact(&mut body).await.unwrap();
            assert!(matches!(
                cordis_up::decode(&body, &cordis_up::Limits::default()).unwrap(),
                cordis_up::Message::Notification { method, params }
                    if method == "tick" && params == Value::from(value)
            ));
        }
    })
    .await
    .expect("已排队的通知应在读取方恢复后送达");
    peer.terminate("test completed");
}

// 自管理操作的关闭动作在完整响应写出之后执行。
#[tokio::test]
async fn after_write_runs_after_the_complete_response() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut client, server) = tokio::io::duplex(64);
    let (read, write) = tokio::io::split(server);
    let connection = Connection::attach(
        1,
        read,
        write,
        ConnectionOptions::default(),
        Arc::new(Arithmetic),
    )
    .unwrap();
    let request = cordis_up::encode(
        &cordis_up::Message::Request {
            id: 1,
            method: "close".into(),
            params: Value::Null,
        },
        &cordis_up::Limits::default(),
    )
    .unwrap();
    client
        .write_all(&(request.len() as u32).to_be_bytes())
        .await
        .unwrap();
    client.write_all(&request).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    let length = u32::from_be_bytes(response[..4].try_into().unwrap()) as usize;
    assert_eq!(response.len(), length + 4);
    assert!(matches!(
        cordis_up::decode(&response[4..], &cordis_up::Limits::default()).unwrap(),
        cordis_up::Message::Response {
            id: 1,
            result: Ok(Value::Null)
        }
    ));
    assert!(!connection.is_alive());
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
    assert!(
        error
            .message
            .starts_with("通知 data 的参数编码失败：UP/1 容量超限：max_frame"),
        "{}",
        error.message
    );
    let invoke = Value::Map(vec![
        (Value::Text("method".into()), Value::Text("bundle".into())),
        (Value::Text("args".into()), Value::Bytes(vec![6; 2048])),
    ]);
    let error = first
        .request("invoke", invoke, Duration::from_secs(2))
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, 8);
    assert!(
        error.message.starts_with(
            "invoke（方法 bundle） 的参数编码失败：UP/1 容量超限：max_frame 超过上限 1024"
        ),
        "{}",
        error.message
    );
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
    assert!(
        error
            .message
            .starts_with("bytes 的返回值编码失败：UP/1 容量超限：max_frame"),
        "对端返回值超限时调用方应得到方向与容量说明：{}",
        error.message
    );
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
