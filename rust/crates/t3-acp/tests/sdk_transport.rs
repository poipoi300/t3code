#![allow(clippy::unwrap_used)]
use agent_client_protocol::{ByteStreams, Client};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

#[tokio::test]
async fn sdk_fits_tokio_pipes_but_rejects_t3_json_rpc_batches() {
    let (sdk, peer) = tokio::io::duplex(4096);
    let (sdk_reader, w) = tokio::io::split(sdk);
    let (r, mut w2) = tokio::io::split(peer);
    let transport = ByteStreams::new(w.compat_write(), sdk_reader.compat());
    let driver = tokio::spawn(Client.v1().connect_to(transport));
    let mut reader = BufReader::new(r);
    let mut batch =
        serde_json::to_vec(&json!([{"jsonrpc":"2.0","id":0,"method":"_vendor","params":{}}]))
            .unwrap();
    batch.push(b'\n');
    w2.write_all(&batch).await.unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert!(response.is_object());
    assert_eq!(response["error"]["code"], -32600);
    assert_eq!(response["id"], Value::Null);
    driver.abort();
    let _ = driver.await;
}
