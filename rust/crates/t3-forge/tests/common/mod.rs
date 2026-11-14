use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};
pub struct Request {
    pub method: String,
    pub path: String,
    pub body: Value,
    pub headers: String,
}
pub struct Answer {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(String, String)>,
}
pub fn answer(value: Value) -> Answer {
    Answer {
        status: 200,
        body: value.to_string(),
        headers: vec![],
    }
}
pub async fn server(
    count: usize,
    respond: impl Fn(&Request) -> Answer + Send + 'static,
) -> (url::Url, JoinHandle<Vec<Request>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/2.0", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let task = tokio::spawn(async move {
        let mut requests = vec![];
        for _ in 0..count {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = vec![];
            let mut chunk = [0; 8192];
            let header_end = loop {
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(at) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break at + 4;
                }
            };
            let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
            let length = headers
                .lines()
                .find_map(|s| {
                    s.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|s| s.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            while bytes.len() < header_end + length {
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
            }
            let first: Vec<_> = headers.lines().next().unwrap().split_whitespace().collect();
            let request = Request {
                method: first[0].into(),
                path: first[1].into(),
                body: serde_json::from_slice(&bytes[header_end..header_end + length])
                    .unwrap_or(Value::Null),
                headers,
            };
            let answer = respond(&request);
            let extras = answer
                .headers
                .into_iter()
                .map(|(k, v)| format!("{k}: {v}\r\n"))
                .collect::<String>();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 {} Answer\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                        answer.status,
                        answer.body.len(),
                        extras,
                        answer.body
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            requests.push(request);
        }
        requests
    });
    (base, task)
}
