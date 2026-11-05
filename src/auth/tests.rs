use super::*;
use crate::{ConditionalKeyValue, ConditionalWriteError, VssClient};
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Notify,
    task::JoinHandle,
};

#[derive(Default)]
struct Observations {
    challenges: usize,
    signing_keys: Vec<String>,
    writes: usize,
}

struct Server {
    url: String,
    held: Arc<Notify>,
    release: Arc<Notify>,
    released: Arc<Notify>,
    observations: Arc<Mutex<Observations>>,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    async fn new(hold_callback: bool, hold_body: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://localhost:{}", listener.local_addr().unwrap().port());
        let held = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let released = Arc::new(Notify::new());
        let observations = Arc::new(Mutex::new(Observations::default()));
        let (signal, resume, drained, state) = (
            Arc::clone(&held),
            Arc::clone(&release),
            Arc::clone(&released),
            Arc::clone(&observations),
        );
        let challenge = format!("{url}/callback?k1={}", "00".repeat(32));
        let task = tokio::spawn(async move {
            let mut fault_pending = true;
            while let Ok((mut stream, _)) = listener.accept().await {
                let request = read_request(&mut stream).await;
                let path = request.split_whitespace().nth(1).unwrap();
                let callback = path.starts_with("/callback?");
                let body = {
                    let mut state = state.lock().unwrap();
                    if path == "/challenge" {
                        state.challenges += 1;
                        challenge.as_bytes().to_vec()
                    } else if callback {
                        let key = path.split('&').find(|v| v.starts_with("key=")).unwrap();
                        state.signing_keys.push(key.to_owned());
                        b"{\"token\":\"e30.e30.AA\"}".to_vec()
                    } else {
                        assert_eq!(path, "/putObjects");
                        assert!(request.contains("authorization: Bearer e30.e30.AA\r\n"));
                        state.writes += 1;
                        vec![]
                    }
                };
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if fault_pending && callback == hold_callback {
                    fault_pending = false;
                    if hold_body {
                        stream.write_all(headers.as_bytes()).await.unwrap();
                        stream.write_all(&body[..1]).await.unwrap();
                    }
                    signal.notify_one();
                    resume.notified().await;
                    // The expired client's response is still released; it must not dispatch VSS.
                    if !hold_body {
                        let _ = stream.write_all(headers.as_bytes()).await;
                    }
                    let _ = stream.write_all(&body[usize::from(hold_body)..]).await;
                    drained.notify_one();
                } else {
                    stream.write_all(headers.as_bytes()).await.unwrap();
                    stream.write_all(&body).await.unwrap();
                }
            }
        });
        Self {
            url,
            held,
            release,
            released,
            observations,
            task,
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = vec![];
    loop {
        let byte = stream.read_u8().await.unwrap();
        bytes.push(byte);
        if bytes.ends_with(b"\r\n\r\n") {
            let headers = String::from_utf8(bytes).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).await.unwrap();
            return headers;
        }
    }
}

fn update() -> Vec<ConditionalKeyValue> {
    vec![ConditionalKeyValue {
        key: "backup".into(),
        value: vec![1],
        expected_version: 0,
    }]
}

#[tokio::test]
async fn auth_timeout_rejects_before_dispatch_and_same_provider_can_retry() {
    // Both network legs can stall before headers or during a partially delivered body.
    for hold_callback in [false, true] {
        for hold_body in [false, true] {
            let server = Server::new(hold_callback, hold_body).await;
            let client = VssClient::new_with_lnurl_auth(
                server.url.clone(),
                "original-store".into(),
                [42; 64],
                format!("{}/challenge", server.url),
            )
            .await
            .unwrap();
            let original = client.clone();
            let attempt = tokio::spawn(async move { original.put_conditional(update()).await });
            tokio::time::timeout(Duration::from_secs(2), server.held.notified())
                .await
                .unwrap();
            tokio::time::pause();
            tokio::time::advance(AUTH_TIMEOUT + Duration::from_millis(1)).await;
            let outcome = tokio::time::timeout(Duration::from_secs(1), attempt)
                .await
                .unwrap()
                .unwrap();
            tokio::time::resume();
            assert!(matches!(outcome, Err(ConditionalWriteError::Rejected)));
            assert_eq!(server.observations.lock().unwrap().writes, 0);

            server.release.notify_one();
            tokio::time::timeout(Duration::from_secs(2), server.released.notified())
                .await
                .unwrap();
            assert_eq!(server.observations.lock().unwrap().writes, 0);
            // The same client recovers and then reuses its cached JWT without another auth leg.
            client.put_conditional(update()).await.unwrap();
            client.put_conditional(update()).await.unwrap();
            let state = server.observations.lock().unwrap();
            assert_eq!(state.writes, 2);
            assert_eq!(state.challenges, 2);
            assert_eq!(state.signing_keys.len(), if hold_callback { 2 } else { 1 });
            assert!(state.signing_keys.windows(2).all(|keys| keys[0] == keys[1]));
        }
    }
}
