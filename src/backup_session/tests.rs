use super::*;
use bitcoin::{
    hashes::{sha256, Hash},
    hex::FromHex,
    secp256k1::{ecdsa::Signature, Message, PublicKey, Secp256k1},
};
use prost::Message as ProstMessage;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{Mutex, Notify},
    task::JoinHandle,
};
use vss_client_ng::types::{
    ErrorCode, ErrorResponse, GetObjectRequest, GetObjectResponse, KeyValue,
};

type StatusResponse = Arc<Mutex<Option<(u16, Vec<u8>)>>>;

struct Fixture {
    url: String,
    objects: Arc<Mutex<HashMap<String, KeyValue>>>,
    auth_keys: Arc<Mutex<Vec<String>>>,
    requests: Arc<Mutex<Vec<GetObjectRequest>>>,
    status_response: StatusResponse,
    status_renewed: Arc<Mutex<Vec<bool>>>,
    status_requests: Arc<Mutex<Vec<String>>>,
    expire_tokens: Arc<AtomicBool>,
    refuse_auth: Arc<AtomicBool>,
    redirect_requests: Arc<AtomicUsize>,
    hold: Arc<Mutex<Option<String>>>,
    received: Arc<Notify>,
    release: Arc<Notify>,
    task: JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://localhost:{}", listener.local_addr().unwrap().port());
        let objects = Arc::new(Mutex::new(HashMap::<String, KeyValue>::new()));
        let auth_keys = Arc::new(Mutex::new(Vec::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let status_response = Arc::new(Mutex::new(None::<(u16, Vec<u8>)>));
        let status_requests = Arc::new(Mutex::new(Vec::new()));
        let expire_tokens = Arc::new(AtomicBool::new(false));
        let refuse_auth = Arc::new(AtomicBool::new(false));
        let redirect_requests = Arc::new(AtomicUsize::new(0));
        let expired = expire_tokens.clone();
        let refused = refuse_auth.clone();
        let redirected = redirect_requests.clone();
        let status_reply = status_response.clone();
        let status_reads = status_requests.clone();
        let status_renewed = Arc::new(Mutex::new(Vec::new()));
        let renewed_reads = status_renewed.clone();
        let hold = Arc::new(Mutex::new(None::<String>));
        let received = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (base, rows, keys, reads, held, arrived, resume) = (
            url.clone(),
            objects.clone(),
            auth_keys.clone(),
            requests.clone(),
            hold.clone(),
            received.clone(),
            release.clone(),
        );
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut data = Vec::new();
                let end = loop {
                    let mut chunk = [0; 4096];
                    let n = stream.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        break None;
                    }
                    data.extend_from_slice(&chunk[..n]);
                    if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(i + 4);
                    }
                };
                let Some(end) = end else {
                    continue;
                };
                let headers = std::str::from_utf8(&data[..end]).unwrap();
                let path = headers
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                let renewed_proof = headers.lines().any(|line| {
                    line.to_ascii_lowercase().starts_with("authorization:")
                        && line.contains(".eyJleHAiOjQxMDI0NDQ4MDB9.")
                });
                let principal = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("authorization").then(|| {
                        value
                            .trim()
                            .rsplit('.')
                            .next()
                            .unwrap()
                            .strip_suffix("AA")
                            .unwrap()
                            .to_owned()
                    })
                });
                if path == "/getObject" || path == "/getStoreMigrationState" {
                    assert!(headers.to_lowercase().contains("authorization: bearer "));
                }
                while data.len() < end + length {
                    let mut chunk = [0; 4096];
                    let n = stream.read(&mut chunk).await.unwrap();
                    assert_ne!(n, 0);
                    data.extend_from_slice(&chunk[..n]);
                }
                let (status, body) = if path == "/auth" {
                    if refused.load(Ordering::SeqCst) {
                        (401, b"auth unavailable".to_vec())
                    } else {
                        (
                            200,
                            format!("{base}/callback?k1={}", "01".repeat(32)).into_bytes(),
                        )
                    }
                } else if path.starts_with("/callback?") {
                    let query: HashMap<_, _> = reqwest::Url::parse(&format!("{base}{path}"))
                        .unwrap()
                        .query_pairs()
                        .into_owned()
                        .collect();
                    let public = PublicKey::from_str(&query["key"]).unwrap();
                    let signature =
                        Signature::from_der(&Vec::<u8>::from_hex(&query["sig"]).unwrap()).unwrap();
                    Secp256k1::verification_only()
                        .verify_ecdsa(&Message::from_digest([1; 32]), &signature, &public)
                        .unwrap();
                    keys.lock().await.push(query["key"].clone());
                    (
                        200,
                        format!(
                            r#"{{"token":"e30.{}.{}AA"}}"#,
                            if expired.load(Ordering::SeqCst) {
                                "eyJleHAiOjB9"
                            } else {
                                "eyJleHAiOjQxMDI0NDQ4MDB9"
                            },
                            query["key"]
                        )
                        .into_bytes(),
                    )
                } else if path == "/status-redirect" {
                    redirected.fetch_add(1, Ordering::SeqCst);
                    (200, vec![])
                } else if path == "/getStoreMigrationState" {
                    use crate::migration_status::wire;
                    let req = wire::Request::decode(&data[end..]).unwrap();
                    status_reads.lock().await.push(req.store_id.clone());
                    renewed_reads.lock().await.push(renewed_proof);
                    status_reply.lock().await.clone().unwrap_or_else(|| {
                        (
                            200,
                            wire::State {
                                effective_principal: principal.unwrap(),
                                store_id: req.store_id,
                                ..Default::default()
                            }
                            .encode_to_vec(),
                        )
                    })
                } else {
                    assert_eq!(path, "/getObject");
                    let req = GetObjectRequest::decode(&data[end..]).unwrap();
                    assert_eq!(req.store_id, "owned-store");
                    reads.lock().await.push(req.clone());
                    match rows.lock().await.get(&req.key).cloned() {
                        Some(value) => (
                            200,
                            GetObjectResponse { value: Some(value) }.encode_to_vec(),
                        ),
                        None => (
                            404,
                            ErrorResponse {
                                error_code: ErrorCode::NoSuchKeyException as i32,
                                message: "missing".into(),
                            }
                            .encode_to_vec(),
                        ),
                    }
                };
                let should_hold = held.lock().await.as_deref() == Some(&path);
                let storage_path = path == "/getObject" || path == "/getStoreMigrationState";
                if should_hold && !storage_path {
                    arrived.notify_one();
                    resume.notified().await;
                }
                let location = if status == 302 {
                    format!("Location: {base}/status-redirect\r\n")
                } else {
                    String::new()
                };
                let response = format!("HTTP/1.1 {status} Test\r\n{location}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
                let header_written = stream.write_all(response.as_bytes()).await;
                let prefix = if should_hold && storage_path {
                    body.len() / 2
                } else {
                    0
                };
                if prefix > 0 {
                    header_written.unwrap();
                    stream.write_all(&body[..prefix]).await.unwrap();
                    arrived.notify_one();
                    resume.notified().await;
                }
                let _ = stream.write_all(&body[prefix..]).await;
            }
        });
        Self {
            url,
            objects,
            auth_keys,
            requests,
            status_response,
            status_renewed,
            status_requests,
            expire_tokens,
            refuse_auth,
            redirect_requests,
            hold,
            received,
            release,
            task,
        }
    }

    async fn session(&self, seed: [u8; 64]) -> BackupReadSession {
        BackupReadSession::new(
            BackupReadConfig {
                base_url: self.url.clone(),
                store_id: "owned-store".into(),
                lnurl_auth_server_url: format!("{}/auth", self.url),
            },
            seed,
        )
        .await
        .unwrap()
    }

    async fn app_object(&self, session: &BackupReadSession) -> KeyValue {
        let key = session.app.build_key("WALLET");
        let value = session
            .app
            .storable_builder
            .build(
                b"complete archive".to_vec(),
                -1,
                &session.app.app_data_encryption_key,
                key.as_bytes(),
            )
            .encode_to_vec();
        let row = KeyValue {
            key,
            value,
            version: 2,
        };
        self.objects
            .lock()
            .await
            .insert(row.key.clone(), row.clone());
        row
    }

    async fn ldk_object(&self, session: &BackupReadSession, namespace: &LdkNamespace) -> KeyValue {
        let key = session.ldk.build_key("state", namespace);
        let value = session
            .ldk
            .storable_builder
            .build(
                b"ldk state".to_vec(),
                -1,
                &session.ldk.data_encryption_key,
                key.as_bytes(),
            )
            .encode_to_vec();
        let row = KeyValue {
            key,
            value,
            version: 3,
        };
        self.objects
            .lock()
            .await
            .insert(row.key.clone(), row.clone());
        row
    }
}

use std::str::FromStr;
fn app() -> BackupReadTarget {
    BackupReadTarget::App {
        key: "WALLET".into(),
    }
}
fn ldk(namespace: LdkNamespace) -> BackupReadTarget {
    BackupReadTarget::Ldk {
        key: "state".into(),
        namespace,
    }
}

#[tokio::test]
async fn both_domains_authenticate_and_preserve_legacy_derivations() {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    let app_row = fixture.app_object(&session).await;
    let ldk_row = fixture.ldk_object(&session, &LdkNamespace::Monitors).await;
    let app_read = session.read(app(), 4096).await.unwrap();
    let ldk_read = session
        .read(ldk(LdkNamespace::Monitors), 4096)
        .await
        .unwrap();
    let a = session.receipt(&app_read).unwrap().unwrap();
    let l = session.receipt(&ldk_read).unwrap().unwrap();
    assert_eq!(a.value, b"complete archive");
    assert_eq!(l.value, b"ldk state");
    assert_eq!(
        a.envelope_sha256,
        sha256::Hash::hash(&app_row.value).to_string()
    );
    assert_eq!(
        l.envelope_sha256,
        sha256::Hash::hash(&ldk_row.value).to_string()
    );
    assert_eq!(a.storage_key, app_row.key);
    assert_eq!(l.storage_key, ldk_row.key);
    assert_eq!((a.version, l.version), (2, 3));
    let first_keys = fixture.auth_keys.lock().await.clone();
    assert_eq!(first_keys.len(), 2);
    assert_ne!(first_keys[0], first_keys[1]);
    let mut another_seed = [42; 64];
    another_seed[32..].fill(24);
    let other = fixture.session(another_seed).await;
    assert_eq!(
        session.app.app_data_encryption_key,
        other.app.app_data_encryption_key
    );
    assert_ne!(
        session.ldk.data_encryption_key,
        other.ldk.data_encryption_key
    );
    other.read(app(), 4096).await.unwrap();
    let missing = other.read(ldk(LdkNamespace::Monitors), 4096).await.unwrap();
    assert!(other.receipt(&missing).unwrap().is_none());
    let keys = fixture.auth_keys.lock().await;
    assert_eq!(keys[0], keys[2]);
    assert_ne!(keys[1], keys[3]);
}

#[tokio::test]
async fn identical_wallet_arguments_do_not_adopt_another_session_observation() {
    let fixture = Fixture::new().await;
    let first = fixture.session([42; 64]).await;
    let second = fixture.session([42; 64]).await;
    fixture.app_object(&first).await;
    let result = first.read(app(), 4096).await.unwrap();
    assert!(matches!(
        second.receipt(&result),
        Err(BackupReadError::DifferentOwner)
    ));
    assert!(first.receipt(&result).unwrap().is_some());
    first.revoke();
    assert!(matches!(
        first.receipt(&result),
        Err(BackupReadError::Revoked)
    ));
}

#[tokio::test]
async fn typed_absence_keeps_original_target_and_owner() {
    let fixture = Fixture::new().await;
    let first = fixture.session([42; 64]).await;
    let missing = first
        .read(
            ldk(LdkNamespace::MonitorUpdates {
                monitor_id: "original".into(),
            }),
            4096,
        )
        .await
        .unwrap();
    assert!(first.receipt(&missing).unwrap().is_none());
    assert!(
        matches!(missing.target(),BackupReadTarget::Ldk { key, namespace:LdkNamespace::MonitorUpdates { monitor_id } } if key=="state" && monitor_id=="original")
    );
    let replacement = fixture.session([24; 64]).await;
    assert!(matches!(
        replacement.receipt(&missing),
        Err(BackupReadError::DifferentOwner)
    ));
    first.revoke();
    assert!(matches!(
        first.receipt(&missing),
        Err(BackupReadError::Revoked)
    ));
}

#[tokio::test]
async fn revoked_session_never_starts_a_request() {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    session.revoke();
    session.revoke();
    assert!(matches!(
        session.read(app(), 4096).await,
        Err(BackupReadError::Revoked)
    ));
    assert!(fixture.auth_keys.lock().await.is_empty());
    assert!(fixture.requests.lock().await.is_empty());
}

#[tokio::test]
async fn revoke_wakes_pending_credentials_and_storage_without_replay() {
    for held in ["/auth", "/getObject"] {
        let fixture = Fixture::new().await;
        let session = fixture.session([42; 64]).await;
        fixture.app_object(&session).await;
        *fixture.hold.lock().await = Some(held.into());
        let read = session.read(app(), 4096);
        tokio::pin!(read);
        tokio::select! { _=fixture.received.notified()=>{}, _=&mut read=>panic!("read completed before held boundary") }
        session.revoke();
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(1), read)
                .await
                .unwrap(),
            Err(BackupReadError::Revoked)
        ));
        fixture.release.notify_one();
        assert!(matches!(
            session.read(app(), 4096).await,
            Err(BackupReadError::Revoked)
        ));
        assert_eq!(
            fixture.requests.lock().await.len(),
            usize::from(held == "/getObject")
        );
    }
}

#[tokio::test]
async fn dropping_one_pending_read_does_not_revoke_or_replay_the_session() {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    fixture.app_object(&session).await;
    *fixture.hold.lock().await = Some("/getObject".into());
    {
        let read = session.read(app(), 4096);
        tokio::pin!(read);
        tokio::select! { _=fixture.received.notified()=>{}, _=&mut read=>panic!("unexpected completion") }
    }
    *fixture.hold.lock().await = None;
    fixture.release.notify_one();
    let observation = session.read(app(), 4096).await.unwrap();
    assert!(session.receipt(&observation).unwrap().is_some());
    assert_eq!(fixture.requests.lock().await.len(), 2);
}

#[tokio::test]
async fn singleton_replacement_cannot_retarget_the_owned_pair() {
    let _lock = crate::FFI_TEST_LOCK.lock().await;
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    fixture.app_object(&session).await;
    fixture.ldk_object(&session, &LdkNamespace::Default).await;
    let other_fixture = Fixture::new().await;
    let other = other_fixture.session([24; 64]).await;
    *crate::get_vss_client().lock().unwrap() = Some(session.app.clone());
    *crate::get_ldk_client().lock().unwrap() = Some(session.ldk.clone());
    *fixture.hold.lock().await = Some("/auth".into());
    let pending = session.read(app(), 4096);
    tokio::pin!(pending);
    tokio::select! {
        _ = fixture.received.notified() => {},
        _ = &mut pending => panic!("original read completed before replacement"),
    }
    *crate::get_vss_client().lock().unwrap() = Some(other.app.clone());
    *crate::get_ldk_client().lock().unwrap() = Some(other.ldk.clone());
    *fixture.hold.lock().await = None;
    fixture.release.notify_one();
    let original = pending.await.unwrap();
    assert_eq!(
        session.receipt(&original).unwrap().unwrap().value,
        b"complete archive"
    );
    let original_ldk = session
        .read(ldk(LdkNamespace::Default), 4096)
        .await
        .unwrap();
    assert_eq!(
        session.receipt(&original_ldk).unwrap().unwrap().value,
        b"ldk state"
    );
    assert_eq!(fixture.requests.lock().await.len(), 2);
    assert!(other_fixture.requests.lock().await.is_empty());
    assert!(other_fixture.auth_keys.lock().await.is_empty());
    assert_eq!(fixture.auth_keys.lock().await.len(), 2);
    crate::vss_shutdown_client();
    crate::vss_shutdown_ldk_client();
}

#[tokio::test]
async fn ldk_receipt_refuses_cross_namespace_aad_wrong_key_and_response_limit() {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    let row = fixture.ldk_object(&session, &LdkNamespace::Monitors).await;
    let wrong_key = session
        .ldk
        .build_key("state", &LdkNamespace::ArchivedMonitors);
    fixture
        .objects
        .lock()
        .await
        .insert(wrong_key.clone(), row.clone());
    assert!(matches!(
        session
            .read(ldk(LdkNamespace::ArchivedMonitors), 4096)
            .await,
        Err(BackupReadError::Read(VssError::InvalidData { .. }))
    ));
    fixture.objects.lock().await.insert(
        wrong_key.clone(),
        KeyValue {
            key: wrong_key,
            ..row
        },
    );
    assert!(matches!(
        session
            .read(ldk(LdkNamespace::ArchivedMonitors), 4096)
            .await,
        Err(BackupReadError::Read(VssError::InvalidData { .. }))
    ));
    assert!(matches!(
        session.read(ldk(LdkNamespace::Monitors), 1).await,
        Err(BackupReadError::Read(VssError::InvalidData { .. }))
    ));
    assert_eq!(fixture.requests.lock().await.len(), 3);
}

#[tokio::test]
async fn migration_observations_bind_original_domain_and_owner() {
    let fixture = Fixture::new().await;
    let session = fixture.session([7; 64]).await;
    let app = session
        .read_migration_state(BackupDomain::App)
        .await
        .unwrap();
    let ldk = session
        .read_migration_state(BackupDomain::Ldk)
        .await
        .unwrap();
    assert_eq!(app.domain(), BackupDomain::App);
    assert_eq!(ldk.domain(), BackupDomain::Ldk);
    let a = session.migration_state(&app).unwrap();
    let l = session.migration_state(&ldk).unwrap();
    assert_eq!(a.store_id, l.store_id);
    assert_ne!(a.effective_principal, l.effective_principal);
    let keys = fixture.auth_keys.lock().await;
    assert_eq!(a.effective_principal, keys[0]);
    assert_eq!(l.effective_principal, keys[1]);
    assert!(matches!(a.policy, crate::MigrationPolicy::Legacy));
    assert!(matches!(l.policy, crate::MigrationPolicy::Legacy));
    drop(keys);
    let replacement = fixture.session([7; 64]).await;
    assert!(matches!(
        replacement.migration_state(&app),
        Err(BackupReadError::DifferentOwner)
    ));
    session.revoke();
    assert!(matches!(
        session.migration_state(&ldk),
        Err(BackupReadError::Revoked)
    ));
    assert!(matches!(
        session.read_migration_state(BackupDomain::App).await,
        Err(BackupReadError::Revoked)
    ));
    assert_eq!(fixture.status_requests.lock().await.len(), 2);
}

#[tokio::test]
async fn migration_unavailable_or_wrong_store_is_never_legacy() {
    use crate::migration_status::wire;
    let fixture = Fixture::new().await;
    let session = fixture.session([8; 64]).await;
    for (status, body) in [
        (
            404,
            ErrorResponse {
                error_code: ErrorCode::NoSuchKeyException as i32,
                message: "missing".into(),
            }
            .encode_to_vec(),
        ),
        (401, vec![]),
        (403, vec![]),
        (502, vec![]),
        (302, vec![]),
        (204, vec![]),
        (200, vec![]),
        (
            200,
            wire::State {
                effective_principal: "principal".into(),
                store_id: "wrong".into(),
                ..Default::default()
            }
            .encode_to_vec(),
        ),
        (200, vec![0; 65537]),
    ] {
        *fixture.status_response.lock().await = Some((status, body));
        assert!(session
            .read_migration_state(BackupDomain::App)
            .await
            .is_err());
    }
    assert_eq!(fixture.status_requests.lock().await.len(), 9);
    assert_eq!(fixture.redirect_requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn migration_revoke_abandons_credentials_and_partial_body() {
    for path in ["/auth", "/getStoreMigrationState"] {
        let fixture = Fixture::new().await;
        *fixture.hold.lock().await = Some(path.into());
        let session = fixture.session([9; 64]).await;
        let read = session.read_migration_state(BackupDomain::Ldk);
        tokio::pin!(read);
        tokio::select! {
            _ = fixture.received.notified() => {},
            _ = &mut read => panic!("read completed before controlled boundary"),
            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => panic!("fixture boundary not reached"),
        }
        session.revoke();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), &mut read)
            .await
            .unwrap();
        assert!(matches!(result, Err(BackupReadError::Revoked)));
        fixture.release.notify_one();
        assert_eq!(
            fixture.status_requests.lock().await.len(),
            usize::from(path != "/auth")
        );
    }
}

#[tokio::test]
async fn migration_reads_retain_original_clients_during_global_replacement() {
    let _guard = crate::FFI_TEST_LOCK.lock().await;
    let original = Fixture::new().await;
    let replacement = Fixture::new().await;
    let session = original.session([10; 64]).await;
    *original.hold.lock().await = Some("/auth".into());
    let read = session.read_migration_state(BackupDomain::App);
    tokio::pin!(read);
    tokio::select! {
        _ = original.received.notified() => {},
        _ = &mut read => panic!("original credential challenge must remain held"),
        _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => panic!("fixture boundary not reached"),
    }
    let other = replacement.session([11; 64]).await;
    let old_app = crate::get_vss_client()
        .lock()
        .unwrap()
        .replace(other.app.clone());
    let old_ldk = crate::get_ldk_client()
        .lock()
        .unwrap()
        .replace(other.ldk.clone());
    *original.hold.lock().await = None;
    original.release.notify_one();
    let app = read.await.unwrap();
    let ldk = session
        .read_migration_state(BackupDomain::Ldk)
        .await
        .unwrap();
    assert_eq!(
        session.migration_state(&app).unwrap().effective_principal,
        original.auth_keys.lock().await[0]
    );
    assert_eq!(
        session.migration_state(&ldk).unwrap().effective_principal,
        original.auth_keys.lock().await[1]
    );
    assert_eq!(original.status_requests.lock().await.len(), 2);
    assert!(replacement.auth_keys.lock().await.is_empty());
    assert!(replacement.status_requests.lock().await.is_empty());
    *crate::get_vss_client().lock().unwrap() = old_app;
    *crate::get_ldk_client().lock().unwrap() = old_ldk;
}

#[tokio::test]
async fn migration_credential_renewal_keeps_signer_and_reports_mapping_change() {
    use crate::migration_status::wire;
    let fixture = Fixture::new().await;
    fixture.expire_tokens.store(true, Ordering::SeqCst);
    let session = fixture.session([12; 64]).await;
    let original = session
        .read_migration_state(BackupDomain::App)
        .await
        .unwrap();
    let original_principal = session
        .migration_state(&original)
        .unwrap()
        .effective_principal
        .clone();
    fixture.expire_tokens.store(false, Ordering::SeqCst);
    *fixture.status_response.lock().await = Some((
        200,
        wire::State {
            effective_principal: "changed-mapping".into(),
            store_id: "owned-store".into(),
            ..Default::default()
        }
        .encode_to_vec(),
    ));
    let renewed = session
        .read_migration_state(BackupDomain::App)
        .await
        .unwrap();
    let keys = fixture.auth_keys.lock().await;
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0], keys[1]);
    assert_eq!(*fixture.status_renewed.lock().await, vec![false, true]);
    assert_eq!(
        session
            .migration_state(&original)
            .unwrap()
            .effective_principal,
        original_principal
    );
    assert_eq!(
        session
            .migration_state(&renewed)
            .unwrap()
            .effective_principal,
        "changed-mapping"
    );
    assert_eq!(fixture.status_requests.lock().await.len(), 2);
}

#[tokio::test]
async fn migration_credentials_refused_never_dispatch_storage() {
    let fixture = Fixture::new().await;
    fixture.refuse_auth.store(true, Ordering::SeqCst);
    let session = fixture.session([13; 64]).await;
    assert!(session
        .read_migration_state(BackupDomain::App)
        .await
        .is_err());
    assert!(fixture.status_requests.lock().await.is_empty());
}

#[tokio::test]
async fn migration_deadline_covers_held_credentials_and_body_without_replay() {
    for path in ["/auth", "/getStoreMigrationState"] {
        let fixture = Fixture::new().await;
        *fixture.hold.lock().await = Some(path.into());
        let session = fixture.session([14; 64]).await;
        let read = session.read_migration_state(BackupDomain::App);
        tokio::pin!(read);
        tokio::select! {
            _ = fixture.received.notified() => {},
            _ = &mut read => panic!("deadline boundary not reached"),
            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => panic!("fixture boundary not reached"),
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(12), &mut read)
            .await
            .unwrap();
        assert!(matches!(
            result,
            Err(BackupReadError::Read(VssError::NetworkError { .. }))
        ));
        assert_eq!(
            fixture.status_requests.lock().await.len(),
            usize::from(path != "/auth")
        );
    }
}

#[tokio::test]
async fn migration_caller_drop_does_not_revoke_or_replay() {
    let fixture = Fixture::new().await;
    *fixture.hold.lock().await = Some("/getStoreMigrationState".into());
    let session = fixture.session([15; 64]).await;
    {
        let read = session.read_migration_state(BackupDomain::App);
        tokio::pin!(read);
        tokio::select! {
            _ = fixture.received.notified() => {},
            _ = &mut read => panic!("body not held"),
            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => panic!("fixture boundary not reached"),
        }
    }
    *fixture.hold.lock().await = None;
    fixture.release.notify_one();
    // Only this explicit second operation may issue another request.
    let observed = session
        .read_migration_state(BackupDomain::App)
        .await
        .unwrap();
    assert!(matches!(
        session.migration_state(&observed).unwrap().policy,
        crate::MigrationPolicy::Legacy
    ));
    assert_eq!(fixture.status_requests.lock().await.len(), 2);
}
