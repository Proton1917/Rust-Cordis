use cordis_up::{
    Limits, Message, Value, WireError,
    rpc::{Connection, ConnectionOptions, Handler, HandlerFuture, Outgoing, Payload},
};
use std::{sync::Arc, time::Duration};
use tokio::{io::AsyncWriteExt, sync::mpsc, time::timeout};

struct Observe(mpsc::UnboundedSender<(usize, Value)>);
impl Handler for Observe {
    fn request(
        &self,
        _: Connection,
        _: String,
        _: Payload,
    ) -> HandlerFuture<'_, Result<Outgoing, WireError>> {
        Box::pin(async { Err(WireError::new(3, "unused request")) })
    }

    fn notify(
        &self,
        _: Connection,
        _: String,
        params: Payload,
    ) -> HandlerFuture<'_, Result<(), WireError>> {
        Box::pin(async move {
            self.0
                .send((params.wire_bytes(), params.value().clone()))
                .unwrap();
            Ok(())
        })
    }
}

#[tokio::test]
async fn admission_metadata_retains_original_body_size_instead_of_reencoding() {
    timeout(Duration::from_secs(2), async {
        let (mut writer, socket) = tokio::io::duplex(4096);
        let (input, output) = tokio::io::split(socket);
        let (sender, mut observed) = mpsc::unbounded_channel();
        let peer = Connection::attach(
            1,
            input,
            output,
            ConnectionOptions::default(),
            Arc::new(Observe(sender)),
        )
        .unwrap();
        let params = Value::Bytes(vec![7; 128]);
        let canonical = cordis_up::encode(
            &Message::Notification {
                method: "callback".into(),
                params: params.clone(),
            },
            &Limits::default(),
        )
        .unwrap();
        let offset = canonical.len() - 128 - 2;
        assert_eq!(canonical[offset..offset + 2], [0x58, 0x80]);
        // CBOR permits the same byte-string length in two integer widths.
        let mut wider = canonical.clone();
        wider.splice(offset..offset + 2, [0x59, 0x00, 0x80]);
        for body in [canonical, wider] {
            cordis_up::decode(&body, &Limits::default()).unwrap();
            writer.write_u32(body.len() as u32).await.unwrap();
            writer.write_all(&body).await.unwrap();
            let (size, value) = observed.recv().await.unwrap();
            assert_eq!(size, body.len());
            assert_eq!(value, params);
        }
        peer.terminate("test completed");
    })
    .await
    .expect("wire-size observation completes");
}
