use super::*;
use std::collections::HashMap;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Mutex, Notify},
    task::JoinHandle,
};
use vss_client_ng::{
    headers::FixedHeaders,
    types::{ErrorCode, ErrorResponse, GetObjectRequest, GetObjectResponse, Storable},
};

#[derive(Clone, Copy, Default)]
enum Reply {
    #[default]
    Acknowledge,
    DropAfterApply,
    ErrorAfterApply,
    Redirect,
    HoldAfterApply,
    DelayAcknowledgement,
    MalformedAcknowledgement,
    RejectAuth,
    RejectInvalid,
}

#[derive(Default)]
struct Store {
    values: HashMap<(String, String), KeyValue>,
    writes: Vec<PutObjectRequest>,
    reply: Reply,
}

struct Server {
    url: String,
    store: Arc<Mutex<Store>>,
    applied: Arc<Notify>,
    release: Arc<Notify>,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let store = Arc::new(Mutex::new(Store::default()));
        let applied = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let resume = Arc::clone(&release);
        let (state, signal) = (Arc::clone(&store), Arc::clone(&applied));
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let (path, body) = read_request(&mut stream).await;
                let mut state = state.lock().await;
                if path.ends_with("/putObjects") {
                    let request = PutObjectRequest::decode(body.as_slice()).unwrap();
                    state.writes.push(request.clone());
                    if matches!(state.reply, Reply::RejectAuth | Reply::RejectInvalid) {
                        let (status, code) = if matches!(state.reply, Reply::RejectAuth) {
                            (401, ErrorCode::AuthException)
                        } else {
                            (400, ErrorCode::InvalidRequestException)
                        };
                        respond(&mut stream, status, error(code)).await;
                        continue;
                    }
                    if matches!(state.reply, Reply::Redirect) {
                        stream.write_all(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: /putObjects\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                        continue;
                    }
                    let conflict = request.transaction_items.iter().any(|kv| {
                        let version = state
                            .values
                            .get(&(request.store_id.clone(), kv.key.clone()))
                            .map_or(0, |v| v.version);
                        kv.version != -1 && kv.version != version
                    });
                    if conflict {
                        respond(&mut stream, 409, error(ErrorCode::ConflictException)).await;
                        continue;
                    }
                    for mut kv in request.transaction_items {
                        kv.version = if kv.version == -1 { 1 } else { kv.version + 1 };
                        state
                            .values
                            .insert((request.store_id.clone(), kv.key.clone()), kv);
                    }
                    let reply = state.reply;
                    drop(state);
                    signal.notify_one();
                    match reply {
                        Reply::DropAfterApply => {}
                        Reply::ErrorAfterApply => {
                            respond(&mut stream, 500, error(ErrorCode::InternalServerException))
                                .await
                        }
                        Reply::HoldAfterApply => {
                            // The canceled caller closes its request; allow later reconciliation reads.
                            let _ = stream.read(&mut [0]).await;
                        }
                        Reply::DelayAcknowledgement => {
                            resume.notified().await;
                            respond(&mut stream, 200, vec![]).await;
                        }
                        Reply::MalformedAcknowledgement => {
                            respond(&mut stream, 200, vec![0xff]).await
                        }
                        _ => respond(&mut stream, 200, vec![]).await,
                    }
                } else {
                    assert!(path.ends_with("/getObject"));
                    let request = GetObjectRequest::decode(body.as_slice()).unwrap();
                    let value = state.values.get(&(request.store_id, request.key)).cloned();
                    drop(state);
                    match value {
                        Some(value) => {
                            respond(
                                &mut stream,
                                200,
                                GetObjectResponse { value: Some(value) }.encode_to_vec(),
                            )
                            .await
                        }
                        None => {
                            respond(&mut stream, 404, error(ErrorCode::NoSuchKeyException)).await
                        }
                    }
                }
            }
        });
        Self {
            url,
            store,
            applied,
            release,
            task,
        }
    }

    async fn client(&self, namespace: &str) -> VssClient {
        VssClient::new_with_header_provider(
            self.url.clone(),
            namespace.into(),
            Arc::new(FixedHeaders::new(HashMap::new())),
            Some([42; 32]),
        )
        .await
        .unwrap()
    }
}

async fn read_request(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut bytes = vec![];
    let header_end = loop {
        let mut chunk = [0; 4096];
        let count = stream.read(&mut chunk).await.unwrap();
        assert_ne!(count, 0);
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
    let path = headers.split_whitespace().nth(1).unwrap().to_owned();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap();
    while bytes.len() < header_end + length {
        let mut chunk = [0; 4096];
        let count = stream.read(&mut chunk).await.unwrap();
        assert_ne!(count, 0);
        bytes.extend_from_slice(&chunk[..count]);
    }
    (path, bytes[header_end..header_end + length].to_vec())
}

async fn respond(stream: &mut TcpStream, status: u16, body: Vec<u8>) {
    let header = format!(
        "HTTP/1.1 {status} Response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await.unwrap();
    stream.write_all(&body).await.unwrap();
}

fn error(code: ErrorCode) -> Vec<u8> {
    ErrorResponse {
        error_code: code as i32,
        message: "controlled failure".into(),
    }
    .encode_to_vec()
}

fn item(key: &str, value: &[u8], expected_version: i64) -> ConditionalKeyValue {
    ConditionalKeyValue {
        key: key.into(),
        value: value.into(),
        expected_version,
    }
}

#[tokio::test]
async fn conditional_transaction_preserves_encryption_versions_and_atomic_conflicts() {
    let server = Server::new().await;
    let client = server.client("wallet").await;
    let keys = client
        .put_conditional(vec![
            item("head", b"first", 0),
            item("snapshot", b"complete", 0),
        ])
        .await
        .unwrap();
    assert!(keys.iter().all(|kv| kv.version == 1));
    let saved = client.get("head".into()).await.unwrap().unwrap();
    assert_eq!((saved.value, saved.version), (b"first".to_vec(), 1));
    let wire = server.store.lock().await.writes[0].clone();
    let kv = &wire.transaction_items[0];
    assert_ne!(kv.key, "head");
    let (plaintext, version) = client
        .storable_builder
        .deconstruct(
            Storable::decode(kv.value.as_slice()).unwrap(),
            &client.app_data_encryption_key,
            kv.key.as_bytes(),
        )
        .unwrap();
    assert_eq!((plaintext, version), (b"first".to_vec(), 1));
    assert!(client
        .storable_builder
        .deconstruct(
            Storable::decode(kv.value.as_slice()).unwrap(),
            &client.app_data_encryption_key,
            b"wrong key"
        )
        .is_err());
    assert!(matches!(
        client
            .put_conditional(vec![
                item("head", b"second", 1),
                item("snapshot", b"stale", 0)
            ])
            .await,
        Err(ConditionalWriteError::Conflict)
    ));
    assert_eq!(
        client.get("head".into()).await.unwrap().unwrap().value,
        b"first"
    );
    let ack = client
        .put_conditional(vec![item("head", b"second", 1)])
        .await
        .unwrap();
    assert_eq!(ack[0].version, 2);
    assert_eq!(client.get("head".into()).await.unwrap().unwrap().version, 2);
}

#[tokio::test]
async fn invalid_transaction_is_rejected_before_io() {
    let server = Server::new().await;
    let client = server.client("wallet").await;
    for items in [
        vec![],
        vec![item("", b"", 0)],
        vec![item("a", b"a", 0), item("a", b"b", 1)],
        vec![item("a", b"a", -1)],
        vec![item("a", b"a", i64::MAX)],
    ] {
        assert!(matches!(
            client.put_conditional(items).await,
            Err(ConditionalWriteError::InvalidInput { .. })
        ));
    }
    assert!(server.store.lock().await.writes.is_empty());
}

#[tokio::test]
async fn uncertain_outcomes_never_replay_or_follow_redirects() {
    for reply in [
        Reply::DropAfterApply,
        Reply::ErrorAfterApply,
        Reply::Redirect,
        Reply::MalformedAcknowledgement,
    ] {
        let server = Server::new().await;
        server.store.lock().await.reply = reply;
        let client = server.client("wallet").await;
        assert!(matches!(
            client
                .put_conditional(vec![item("head", b"candidate", 0)])
                .await,
            Err(ConditionalWriteError::OutcomeUnknown)
        ));
        assert_eq!(server.store.lock().await.writes.len(), 1);
        let restored = server
            .client("wallet")
            .await
            .get("head".into())
            .await
            .unwrap();
        if matches!(reply, Reply::Redirect) {
            assert!(restored.is_none());
        } else {
            let restored = restored.unwrap();
            assert_eq!(
                (restored.value, restored.version),
                (b"candidate".to_vec(), 1)
            );
        }
    }
}

#[tokio::test]
async fn server_rejection_is_not_an_unknown_commit() {
    for reply in [Reply::RejectAuth, Reply::RejectInvalid] {
        let server = Server::new().await;
        server.store.lock().await.reply = reply;
        let client = server.client("wallet").await;
        assert!(matches!(
            client
                .put_conditional(vec![item("head", b"candidate", 0)])
                .await,
            Err(ConditionalWriteError::Rejected)
        ));
        let store = server.store.lock().await;
        assert_eq!(store.writes.len(), 1);
        assert!(store.values.is_empty());
    }
}

#[tokio::test]
async fn exported_write_keeps_original_client_when_singleton_is_replaced() {
    let _serial = crate::FFI_TEST_LOCK.lock().await;
    struct ResetClient;
    impl Drop for ResetClient {
        fn drop(&mut self) {
            crate::vss_shutdown_client();
        }
    }
    let _reset = ResetClient;
    let server = Server::new().await;
    let original = server.client("original").await;
    let replacement = server.client("replacement").await;
    *crate::get_vss_client().lock().unwrap() = Some(original.clone());
    server.store.lock().await.reply = Reply::DelayAcknowledgement;
    let pending = tokio::spawn(crate::vss_put_conditional(vec![item(
        "head",
        b"original",
        0,
    )]));
    tokio::time::timeout(Duration::from_secs(3), server.applied.notified())
        .await
        .unwrap();
    *crate::get_vss_client().lock().unwrap() = Some(replacement);
    server.release.notify_one();
    let ack = pending.await.unwrap().unwrap();
    assert_eq!(ack[0].version, 1);
    assert!(crate::vss_get("head".into()).await.unwrap().is_none());
    assert_eq!(
        original.get("head".into()).await.unwrap().unwrap().value,
        b"original"
    );
    assert_eq!(server.store.lock().await.writes.len(), 1);
}

#[tokio::test]
async fn competing_writers_have_one_winner_and_keep_their_namespace() {
    let server = Server::new().await;
    let original = server.client("original").await;
    let competing = original.clone();
    let replacement = server.client("replacement").await;
    let (one, two) = tokio::join!(
        original.put_conditional(vec![item("head", b"one", 0)]),
        competing.put_conditional(vec![item("head", b"two", 0)]),
    );
    assert_ne!(one.is_ok(), two.is_ok());
    assert!(matches!(
        one.err().or(two.err()),
        Some(ConditionalWriteError::Conflict)
    ));
    assert!(replacement.get("head".into()).await.unwrap().is_none());
    assert_eq!(server.store.lock().await.writes.len(), 2);
}

#[tokio::test]
async fn canceled_write_is_reconciled_after_client_restart() {
    let server = Server::new().await;
    server.store.lock().await.reply = Reply::HoldAfterApply;
    let client = server.client("wallet").await;
    let pending = tokio::spawn(async move {
        client
            .put_conditional(vec![item("head", b"retained", 0)])
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), server.applied.notified())
        .await
        .unwrap();
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    let restored = tokio::time::timeout(
        Duration::from_secs(3),
        server.client("wallet").await.get("head".into()),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert_eq!(
        (restored.value, restored.version),
        (b"retained".to_vec(), 1)
    );
    assert_eq!(server.store.lock().await.writes.len(), 1);
}

#[tokio::test]
async fn unconditional_legacy_writer_demonstrates_version_aba_limit() {
    let server = Server::new().await;
    let client = server.client("wallet").await;
    client
        .put_conditional(vec![item("legacy", b"A", 0)])
        .await
        .unwrap();
    client.store("legacy".into(), b"B".to_vec()).await.unwrap();
    assert_eq!(
        client.get("legacy".into()).await.unwrap().unwrap().version,
        1
    );
    client
        .put_conditional(vec![item("legacy", b"C", 1)])
        .await
        .unwrap();
    assert_eq!(
        client.get("legacy".into()).await.unwrap().unwrap().value,
        b"C"
    );
}
