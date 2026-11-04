use super::*;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{Mutex, Notify},
    task::JoinHandle,
};
use vss_client_ng::{headers::FixedHeaders, types::KeyValue};

#[derive(Clone)]
struct Reply {
    status: u16,
    body: Vec<u8>,
    chunked: bool,
    hold: bool,
}

struct Server {
    url: String,
    reply: Arc<Mutex<Reply>>,
    requests: Arc<Mutex<Vec<GetObjectRequest>>>,
    count: Arc<AtomicUsize>,
    received: Arc<Notify>,
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
        let reply = Arc::new(Mutex::new(Reply {
            status: 200,
            body: vec![],
            chunked: false,
            hold: false,
        }));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let count = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (replies, captured, calls, signal, resume) = (
            reply.clone(),
            requests.clone(),
            count.clone(),
            received.clone(),
            release.clone(),
        );
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut bytes = Vec::new();
                let header_end = loop {
                    let mut buf = [0; 4096];
                    let n = stream.read(&mut buf).await.unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
                assert!(headers.starts_with("POST /getObject HTTP/1.1"));
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                while bytes.len() < header_end + length {
                    let mut buf = [0; 4096];
                    let n = stream.read(&mut buf).await.unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&buf[..n]);
                }
                captured
                    .lock()
                    .await
                    .push(GetObjectRequest::decode(&bytes[header_end..]).unwrap());
                calls.fetch_add(1, Ordering::SeqCst);
                let reply = replies.lock().await.clone();
                signal.notify_one();
                let framing = if reply.chunked {
                    "Transfer-Encoding: chunked".into()
                } else {
                    format!("Content-Length: {}", reply.body.len())
                };
                let header = format!("HTTP/1.1 {} Test\r\n{framing}\r\nLocation: /getObject\r\nConnection: close\r\n\r\n", reply.status);
                let _ = stream.write_all(header.as_bytes()).await;
                if reply.hold {
                    resume.notified().await;
                }
                let mut response = Vec::new();
                if reply.chunked {
                    for chunk in reply.body.chunks(13) {
                        response.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
                        response.extend_from_slice(chunk);
                        response.extend_from_slice(b"\r\n");
                    }
                    response.extend_from_slice(b"0\r\n\r\n");
                } else {
                    response.extend_from_slice(&reply.body);
                }
                // Oversized/cancelled reads may intentionally close before consuming the body.
                let _ = stream.write_all(&response).await;
            }
        });
        Self {
            url,
            reply,
            requests,
            count,
            received,
            release,
            task,
        }
    }

    async fn client(&self, seed: u8) -> VssClient {
        VssClient::new_with_header_provider(
            self.url.clone(),
            "app-store".into(),
            Arc::new(FixedHeaders::new(HashMap::new())),
            Some([seed; 32]),
        )
        .await
        .unwrap()
    }

    async fn object(&self, kv: KeyValue) {
        self.reply.lock().await.body = GetObjectResponse { value: Some(kv) }.encode_to_vec();
    }
}

fn encrypted(client: &VssClient, key: &str, embedded: i64, version: i64) -> KeyValue {
    let key = client.build_key(key);
    let value = client
        .storable_builder
        .build(
            b"complete snapshot".to_vec(),
            embedded,
            &client.app_data_encryption_key,
            key.as_bytes(),
        )
        .encode_to_vec();
    KeyValue {
        key,
        value,
        version,
    }
}

#[tokio::test]
async fn hashes_exact_envelope_including_unknown_protobuf_fields() {
    let server = Server::new().await;
    let client = server.client(42).await;
    let mut kv = encrypted(&client, "WALLET", 2, 2);
    let reencoded = kv.value.clone();
    kv.value.extend_from_slice(&[0xf8, 0x07, 0x2a]);
    let expected = sha256::Hash::hash(&kv.value).to_string();
    server.object(kv.clone()).await;
    let receipt = client
        .get_with_receipt("WALLET".into(), 4096)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.envelope_sha256, expected);
    assert_ne!(
        receipt.envelope_sha256,
        sha256::Hash::hash(&reencoded).to_string()
    );
    assert_eq!(receipt.value, b"complete snapshot");
    assert_eq!(receipt.storage_key, kv.key);
    assert_eq!(receipt.key, "WALLET");
    assert_eq!(receipt.store_id, "app-store");
    assert_eq!(receipt.version, 2);
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].key, receipt.storage_key);
    assert_eq!(requests[0].store_id, receipt.store_id);
}

#[tokio::test]
async fn accepts_legacy_unconditional_envelope_without_rewriting_it() {
    let server = Server::new().await;
    let client = server.client(42).await;
    server.object(encrypted(&client, "WALLET", -1, 7)).await;
    let receipt = client
        .get_with_receipt("WALLET".into(), 4096)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.version, 7);
    assert_eq!(receipt.value, b"complete snapshot");
}

#[tokio::test]
async fn only_explicit_missing_key_is_absence() {
    let server = Server::new().await;
    let client = server.client(42).await;
    let missing = ErrorResponse {
        error_code: ErrorCode::NoSuchKeyException as i32,
        message: "missing".into(),
    }
    .encode_to_vec();
    for (status, body) in [
        (200, vec![]),
        (404, b"not found".to_vec()),
        (401, missing.clone()),
        (500, missing.clone()),
        (200, vec![0xff]),
        (307, vec![]),
    ] {
        *server.reply.lock().await = Reply {
            status,
            body,
            chunked: false,
            hold: false,
        };
        let before = server.count.load(Ordering::SeqCst);
        assert!(client
            .get_with_receipt("WALLET".into(), 4096)
            .await
            .is_err());
        assert_eq!(server.count.load(Ordering::SeqCst), before + 1);
    }
    *server.reply.lock().await = Reply {
        status: 404,
        body: missing,
        chunked: false,
        hold: false,
    };
    assert!(client
        .get_with_receipt("WALLET".into(), 4096)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn rejects_wrong_key_owner_and_invalid_versions() {
    let server = Server::new().await;
    let client = server.client(42).await;
    let foreign = server.client(43).await;
    let mut wrong_owner = encrypted(&foreign, "WALLET", 1, 1);
    wrong_owner.key = client.build_key("WALLET");
    let mut wrong_aad = encrypted(&client, "OTHER", 1, 1);
    wrong_aad.key = client.build_key("WALLET");
    for kv in [
        wrong_owner,
        wrong_aad,
        encrypted(&client, "OTHER", 1, 1),
        encrypted(&client, "WALLET", 1, 0),
        encrypted(&client, "WALLET", 1, 2),
        encrypted(&client, "WALLET", -2, 1),
    ] {
        server.object(kv).await;
        assert!(client
            .get_with_receipt("WALLET".into(), 4096)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn bounds_declared_and_chunked_responses_before_decoding() {
    let server = Server::new().await;
    let client = server.client(42).await;
    server.object(encrypted(&client, "WALLET", 1, 1)).await;
    for chunked in [false, true] {
        server.reply.lock().await.chunked = chunked;
        assert!(matches!(
            client.get_with_receipt("WALLET".into(), 16).await,
            Err(VssError::InvalidData { .. })
        ));
    }
    let before = server.count.load(Ordering::SeqCst);
    for limit in [0, MAX_RESPONSE_BYTES + 1] {
        assert!(client
            .get_with_receipt("WALLET".into(), limit)
            .await
            .is_err());
    }
    assert!(client.get_with_receipt(String::new(), 4096).await.is_err());
    assert_eq!(server.count.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn replacing_global_client_does_not_retarget_inflight_receipt() {
    let _guard = crate::FFI_TEST_LOCK.lock().await;
    let server = Server::new().await;
    let client = server.client(42).await;
    server.object(encrypted(&client, "WALLET", 1, 1)).await;
    server.reply.lock().await.hold = true;
    *crate::get_vss_client().lock().unwrap() = Some(client);
    let read = tokio::spawn(crate::vss_get_with_receipt("WALLET".into(), 4096));
    tokio::time::timeout(Duration::from_secs(3), server.received.notified())
        .await
        .unwrap();
    let replacement = server.client(43).await;
    *crate::get_vss_client().lock().unwrap() = Some(replacement);
    server.release.notify_one();
    let receipt = read.await.unwrap().unwrap().unwrap();
    assert_eq!(receipt.value, b"complete snapshot");
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
    crate::vss_shutdown_client();
}

#[tokio::test]
async fn rejects_malformed_encryption_metadata_and_modified_ciphertext() {
    let server = Server::new().await;
    let client = server.client(42).await;
    let original = encrypted(&client, "WALLET", 1, 1);
    for variant in 0..5 {
        let mut kv = original.clone();
        let mut envelope = Storable::decode(kv.value.as_slice()).unwrap();
        match variant {
            0 => envelope.encryption_metadata = None,
            1 => envelope.encryption_metadata.as_mut().unwrap().nonce.clear(),
            2 => envelope.encryption_metadata.as_mut().unwrap().tag.clear(),
            3 => envelope.encryption_metadata.as_mut().unwrap().cipher_format = "unknown".into(),
            _ => envelope.data[0] ^= 1,
        }
        kv.value = envelope.encode_to_vec();
        server.object(kv).await;
        assert!(client
            .get_with_receipt("WALLET".into(), 4096)
            .await
            .is_err());
    }
}

#[test]
fn credential_failures_retain_auth_network_distinction_without_details() {
    let secret = "private provider diagnostic".to_owned();
    let auth = credential_error(VssHeaderProviderError::AuthorizationError {
        error: secret.clone(),
    });
    let network = credential_error(VssHeaderProviderError::RequestError {
        error: secret.clone(),
    });
    let malformed = credential_error(VssHeaderProviderError::InvalidData {
        error: secret.clone(),
    });
    assert!(matches!(auth, VssError::AuthError { .. }));
    assert!(matches!(network, VssError::NetworkError { .. }));
    assert!(matches!(malformed, VssError::InvalidData { .. }));
    for error in [auth, network, malformed] {
        assert!(!error.to_string().contains(&secret));
    }
}

#[test]
fn malformed_error_codes_never_become_absence() {
    let wrapped = [0x08, 0x84, 0x80, 0x80, 0x80, 0x10];
    assert_eq!(
        ErrorResponse::decode(wrapped.as_slice())
            .unwrap()
            .error_code,
        ErrorCode::NoSuchKeyException as i32
    );
    for bytes in [
        vec![0x08, 0x84, 0x80, 0x80, 0x80, 0x10], // 2^32 + 4 narrows to 4 in the legacy enum.
        vec![0x08, 4, 0x08, 1],
        vec![0x08, 1, 0x08, 4],
        vec![0x08, 4, 0x08, 4],
        vec![0x0a, 1, 4], // Packed encoding is invalid for this singular enum.
        vec![0x08, 4, 0x12, 1, 0xff], // Malformed UTF-8 message.
    ] {
        assert!(decode_response(reqwest::StatusCode::NOT_FOUND, &bytes).is_err());
    }
}

struct PendingCredentials;

#[async_trait::async_trait]
impl VssHeaderProvider for PendingCredentials {
    async fn get_headers(
        &self,
        _: &[u8],
    ) -> Result<HashMap<String, String>, VssHeaderProviderError> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn deadline_includes_credentials_and_held_storage_response() {
    let server = Server::new().await;
    let pending = VssClient::new_with_header_provider(
        server.url.clone(),
        "original".into(),
        Arc::new(PendingCredentials),
        Some([42; 32]),
    )
    .await
    .unwrap();
    let client = server.client(42).await;
    server.object(encrypted(&client, "WALLET", 1, 1)).await;
    server.reply.lock().await.hold = true;
    let results = tokio::time::timeout(Duration::from_secs(12), async {
        tokio::join!(
            pending.get_with_receipt("WALLET".into(), 4096),
            client.get_with_receipt("WALLET".into(), 4096)
        )
    })
    .await
    .expect("The combined credential/read deadline must bound both reads");
    assert!(matches!(results.0, Err(VssError::NetworkError { .. })));
    assert!(matches!(results.1, Err(VssError::NetworkError { .. })));
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
    server.release.notify_one();
}
