use super::*;
use vss_client_ng::types::Storable;

const ID: &str = "9b1b6d9f-4a9e-40ca-8e77-c1d8e6dd48fb";

struct Evidence {
    app: MigrationObservation,
    ldk: MigrationObservation,
    archive: BackupReadObservation,
}

impl Evidence {
    async fn read(session: &BackupReadSession) -> Self {
        Self {
            app: session
                .read_migration_state(BackupDomain::App)
                .await
                .unwrap(),
            ldk: session
                .read_migration_state(BackupDomain::Ldk)
                .await
                .unwrap(),
            archive: session.read(app(), 4096).await.unwrap(),
        }
    }

    fn input<'a>(
        &'a self,
        components: &'a [&'a BackupReadObservation],
    ) -> MigrationPreparation<'a> {
        MigrationPreparation {
            migration_id: ID,
            app_status: &self.app,
            ldk_status: &self.ldk,
            archive: &self.archive,
            initial_head: MigrationHeadInput {
                key: "backup/head/v1",
                value: b"coherent app snapshot",
            },
            components,
        }
    }
}

fn assert_invalid(result: Result<PreparedMigration, MigrationPreparationError>) {
    assert!(matches!(
        result,
        Err(MigrationPreparationError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn preparation_retains_exact_archive_and_app_components_without_io() {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    let mut archive = fixture.app_object(&session).await;
    // Unknown protobuf fields must remain part of the exact archive digest.
    archive.value.extend_from_slice(&[0xf8, 0x07, 0x2a]);
    fixture
        .objects
        .lock()
        .await
        .insert(archive.key.clone(), archive.clone());
    let component_key = session.app.build_key("snapshot/component");
    let component = KeyValue {
        key: component_key.clone(),
        version: 1,
        value: session
            .app
            .storable_builder
            .build(
                b"immutable snapshot".to_vec(),
                1,
                &session.app.app_data_encryption_key,
                component_key.as_bytes(),
            )
            .encode_to_vec(),
    };
    fixture
        .objects
        .lock()
        .await
        .insert(component.key.clone(), component.clone());
    let evidence = Evidence::read(&session).await;
    let component_read = session
        .read(
            BackupReadTarget::App {
                key: "snapshot/component".into(),
            },
            4096,
        )
        .await
        .unwrap();
    let before = (
        fixture.requests.lock().await.len(),
        fixture.status_requests.lock().await.len(),
        fixture.auth_keys.lock().await.len(),
    );
    let prepared = session
        .prepare_migration(evidence.input(&[&component_read, &evidence.archive]))
        .unwrap();
    let content = session.prepared_migration(&prepared).unwrap();
    assert_eq!(content.migration_id(), ID);
    assert_eq!(content.stores().len(), 2);
    let app_store = content
        .stores()
        .iter()
        .find(|s| s.domains() == [BackupDomain::App])
        .unwrap();
    let ldk_store = content
        .stores()
        .iter()
        .find(|s| s.domains() == [BackupDomain::Ldk])
        .unwrap();
    assert_eq!(app_store.store_id(), ldk_store.store_id());
    assert_ne!(
        app_store.effective_principal(),
        ldk_store.effective_principal()
    );
    let preserved = app_store.archive().unwrap();
    assert_eq!(preserved.key, archive.key);
    assert_eq!(
        preserved.ciphertext_sha256,
        Some(sha256::Hash::hash(&archive.value).to_byte_array())
    );
    let archived_component = app_store
        .objects()
        .iter()
        .find(|o| o.key == archive.key)
        .unwrap();
    assert_eq!(
        archived_component.ciphertext_sha256,
        preserved.ciphertext_sha256.unwrap()
    );
    let snapshot = app_store
        .objects()
        .iter()
        .find(|o| o.key == component.key)
        .unwrap();
    assert_eq!(
        snapshot.ciphertext_sha256,
        sha256::Hash::hash(&component.value).to_byte_array()
    );
    assert!(ldk_store.objects().is_empty());
    assert!(ldk_store.archive().is_none() && ldk_store.initial_head().is_none());
    assert_eq!(
        content.proposal_sha256(),
        session
            .prepared_migration(&prepared)
            .unwrap()
            .proposal_sha256()
    );
    assert_eq!(
        before,
        (
            fixture.requests.lock().await.len(),
            fixture.status_requests.lock().await.len(),
            fixture.auth_keys.lock().await.len()
        )
    );
    assert_eq!(
        fixture.objects.lock().await[&archive.key].value,
        archive.value
    );
    assert_eq!(fixture.objects.lock().await.len(), 2);
    tokio::task::yield_now().await;
    assert!(!fixture.task.is_finished());
}

#[tokio::test]
async fn preparation_encrypts_head_once_with_original_app_key_and_aad() {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    let evidence = Evidence::read(&session).await;
    let prepared = session.prepare_migration(evidence.input(&[])).unwrap();
    let content = session.prepared_migration(&prepared).unwrap();
    let head = content
        .stores()
        .iter()
        .find_map(|s| s.initial_head())
        .unwrap();
    assert_eq!(head.key(), session.app.build_key("backup/head/v1"));
    let envelope = Storable::decode(head.encrypted_value()).unwrap();
    let (plain, version) = session
        .app
        .storable_builder
        .deconstruct(
            envelope.clone(),
            &session.app.app_data_encryption_key,
            head.key().as_bytes(),
        )
        .unwrap();
    assert_eq!(plain, b"coherent app snapshot");
    assert_eq!(version, 1);
    assert!(session
        .app
        .storable_builder
        .deconstruct(
            envelope.clone(),
            &session.ldk.data_encryption_key,
            head.key().as_bytes()
        )
        .is_err());
    assert!(session
        .app
        .storable_builder
        .deconstruct(envelope, &session.app.app_data_encryption_key, b"wrong key")
        .is_err());
    let repeated = session
        .prepared_migration(&prepared)
        .unwrap()
        .stores()
        .iter()
        .find_map(|s| s.initial_head())
        .unwrap();
    assert_eq!(head.encrypted_value(), repeated.encrypted_value());
    let mut independent_input = evidence.input(&[]);
    independent_input.migration_id = "9b1b6d9f-4a9e-40ca-8e77-c1d8e6dd48fa";
    let independent = session.prepare_migration(independent_input).unwrap();
    assert_ne!(
        content.proposal_sha256(),
        session
            .prepared_migration(&independent)
            .unwrap()
            .proposal_sha256()
    );
}

#[tokio::test]
async fn preparation_preserves_explicit_absent_archive_but_rejects_missing_component() {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    let evidence = Evidence::read(&session).await;
    let prepared = session.prepare_migration(evidence.input(&[])).unwrap();
    let archive = session
        .prepared_migration(&prepared)
        .unwrap()
        .stores()
        .iter()
        .find_map(|s| s.archive())
        .unwrap();
    assert_eq!(archive.key, session.app.build_key("WALLET"));
    assert!(archive.ciphertext_sha256.is_none());
    assert_invalid(session.prepare_migration(evidence.input(&[&evidence.archive])));
}

#[tokio::test]
async fn preparation_cannot_obtain_absence_from_an_unreadable_wallet() {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    let mut row = fixture.app_object(&session).await;
    row.value = b"invalid encrypted envelope".to_vec();
    fixture
        .objects
        .lock()
        .await
        .insert(row.key.clone(), row.clone());
    assert!(session.read(app(), 4096).await.is_err());
    fixture.objects.lock().await.remove(&row.key);
    let evidence = Evidence::read(&session).await;
    let prepared = session.prepare_migration(evidence.input(&[])).unwrap();
    assert!(session
        .prepared_migration(&prepared)
        .unwrap()
        .stores()
        .iter()
        .find_map(|s| s.archive())
        .unwrap()
        .ciphertext_sha256
        .is_none());
}

#[tokio::test]
async fn preparation_refuses_mutable_ldk_components_and_wrong_archive_domain() {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    fixture.ldk_object(&session, &LdkNamespace::Monitors).await;
    let evidence = Evidence::read(&session).await;
    let operational = session
        .read(ldk(LdkNamespace::Monitors), 4096)
        .await
        .unwrap();
    assert!(session.receipt(&operational).unwrap().is_some());
    assert_invalid(session.prepare_migration(evidence.input(&[&operational])));
    let mut wrong_archive = evidence.input(&[]);
    wrong_archive.archive = &operational;
    assert_invalid(session.prepare_migration(wrong_archive));
}

#[tokio::test]
async fn preparation_bounds_encrypted_head_key_and_value() {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    let evidence = Evidence::read(&session).await;
    let boundary = "k".repeat(406);
    let too_long = "k".repeat(407);
    let unbounded = "k".repeat(601);
    assert_eq!(session.app.build_key(&boundary).len(), 600);
    for key in [&too_long, &unbounded] {
        let mut input = evidence.input(&[]);
        input.initial_head.key = key;
        assert_invalid(session.prepare_migration(input));
    }
    let mut accepted = evidence.input(&[]);
    accepted.initial_head.key = &boundary;
    session.prepare_migration(accepted).unwrap();
    let value = vec![1; 10 * 1024 * 1024];
    let mut maximum_plaintext = evidence.input(&[]);
    maximum_plaintext.initial_head.value = &value;
    assert_invalid(session.prepare_migration(maximum_plaintext));
    let mut bounded = evidence.input(&[]);
    bounded.initial_head.value = &value[..value.len() - 1024];
    let prepared = session.prepare_migration(bounded).unwrap();
    let head = session
        .prepared_migration(&prepared)
        .unwrap()
        .stores()
        .iter()
        .find_map(|s| s.initial_head())
        .unwrap();
    assert!(head.encrypted_value().len() <= value.len());
}

#[tokio::test]
async fn preparation_refuses_replacement_owner_for_every_observation_and_result() {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    let replacement = fixture.session([42; 64]).await;
    let evidence = Evidence::read(&session).await;
    let other = Evidence::read(&replacement).await;
    let prepared = session.prepare_migration(evidence.input(&[])).unwrap();
    let mut wrong_status = evidence.input(&[]);
    wrong_status.ldk_status = &other.ldk;
    let mut wrong_archive = evidence.input(&[]);
    wrong_archive.archive = &other.archive;
    for input in [
        wrong_status,
        wrong_archive,
        evidence.input(&[&other.archive]),
    ] {
        assert!(matches!(
            session.prepare_migration(input),
            Err(MigrationPreparationError::Ownership(
                BackupReadError::DifferentOwner
            ))
        ));
    }
    assert!(matches!(
        replacement.prepared_migration(&prepared),
        Err(MigrationPreparationError::Ownership(
            BackupReadError::DifferentOwner
        ))
    ));
    session.revoke();
    assert!(matches!(
        session.prepare_migration(evidence.input(&[])),
        Err(MigrationPreparationError::Ownership(
            BackupReadError::Revoked
        ))
    ));
    assert!(matches!(
        session.prepared_migration(&prepared),
        Err(MigrationPreparationError::Ownership(
            BackupReadError::Revoked
        ))
    ));
}

#[tokio::test]
async fn preparation_refuses_wrong_domain_uuid_and_destructive_head() {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    fixture.app_object(&session).await;
    let evidence = Evidence::read(&session).await;
    let mut wrong_domain = evidence.input(&[]);
    wrong_domain.app_status = &evidence.ldk;
    assert_invalid(session.prepare_migration(wrong_domain));
    for id in [
        "",
        "9B1b6d9f-4a9e-40ca-8e77-c1d8e6dd48fb",
        "9b1b6d9f_4a9e-40ca-8e77-c1d8e6dd48fb",
        "9b1b6d9f-4a9e-40ca-8e77-c1d8e6dd48fg",
    ] {
        let mut input = evidence.input(&[]);
        input.migration_id = id;
        assert_invalid(session.prepare_migration(input));
    }
    for key in ["", "WALLET"] {
        let mut input = evidence.input(&[]);
        input.initial_head.key = key;
        assert_invalid(session.prepare_migration(input));
    }
    assert_invalid(
        session.prepare_migration(evidence.input(&[&evidence.archive, &evidence.archive])),
    );
}

#[tokio::test]
async fn preparation_rejects_changed_or_unavailable_policy_and_keeps_historical_readable() {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    let evidence = Evidence::read(&session).await;
    let prepared = session.prepare_migration(evidence.input(&[])).unwrap();
    *fixture.status_response.lock().await = Some((503, b"unavailable".to_vec()));
    assert!(session
        .read_migration_state(BackupDomain::Ldk)
        .await
        .is_err());
    assert!(session.migration_state(&evidence.ldk).is_ok());
    assert!(matches!(
        session.prepared_migration(&prepared),
        Err(MigrationPreparationError::StalePolicy)
    ));
    assert!(matches!(
        session.prepare_migration(evidence.input(&[])),
        Err(MigrationPreparationError::StalePolicy)
    ));
    *fixture.status_response.lock().await = None;
    let fresh = Evidence::read(&session).await;
    session.prepare_migration(fresh.input(&[])).unwrap();
}

#[tokio::test]
async fn preparation_pending_then_abandoned_status_cannot_reuse_previous_policy() {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    let evidence = Evidence::read(&session).await;
    let prepared = session.prepare_migration(evidence.input(&[])).unwrap();
    *fixture.hold.lock().await = Some("/getStoreMigrationState".into());
    let mut read = Box::pin(session.read_migration_state(BackupDomain::App));
    tokio::select! { _ = fixture.received.notified() => {}, _ = &mut read => panic!("Expected held body") }
    assert!(matches!(
        session.prepared_migration(&prepared),
        Err(MigrationPreparationError::StalePolicy)
    ));
    drop(read);
    fixture.release.notify_one();
    assert!(matches!(
        session.prepare_migration(evidence.input(&[])),
        Err(MigrationPreparationError::StalePolicy)
    ));
}

#[tokio::test]
async fn preparation_rejects_committed_policy_without_legacy_fallback() {
    use crate::migration_status::wire;
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    let mut evidence = Evidence::read(&session).await;
    *fixture.status_response.lock().await = Some((
        200,
        wire::State {
            epoch: 1,
            required_protocol: "bitkit-migration-v1".into(),
            migration_id: ID.into(),
            proposal_sha256: vec![4; 32],
            effective_principal: evidence.ldk.state.effective_principal.clone(),
            store_id: "owned-store".into(),
            ..Default::default()
        }
        .encode_to_vec(),
    ));
    evidence.ldk = session
        .read_migration_state(BackupDomain::Ldk)
        .await
        .unwrap();
    assert!(matches!(
        session.prepare_migration(evidence.input(&[])),
        Err(MigrationPreparationError::AlreadyMigrated)
    ));
}

#[tokio::test]
async fn preparation_coalesces_only_exact_physical_tuple_and_keeps_both_domains() {
    use crate::migration_status::wire;
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    *fixture.status_response.lock().await = Some((
        200,
        wire::State {
            effective_principal: "mapped-principal".into(),
            store_id: "owned-store".into(),
            ..Default::default()
        }
        .encode_to_vec(),
    ));
    let evidence = Evidence::read(&session).await;
    let prepared = session.prepare_migration(evidence.input(&[])).unwrap();
    let stores = session.prepared_migration(&prepared).unwrap().stores();
    assert_eq!(stores.len(), 1);
    assert_eq!(stores[0].domains(), &[BackupDomain::App, BackupDomain::Ldk]);
    fixture.ldk_object(&session, &LdkNamespace::Monitors).await;
    let operational = session
        .read(ldk(LdkNamespace::Monitors), 4096)
        .await
        .unwrap();
    assert_invalid(session.prepare_migration(evidence.input(&[&operational])));
}

#[tokio::test]
async fn preparation_rejects_legacy_unicode_identity_beyond_activation_byte_limit() {
    use crate::migration_status::wire;
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    *fixture.status_response.lock().await = Some((
        200,
        wire::State {
            effective_principal: "é".repeat(120),
            store_id: "owned-store".into(),
            ..Default::default()
        }
        .encode_to_vec(),
    ));
    let evidence = Evidence::read(&session).await;
    assert_invalid(session.prepare_migration(evidence.input(&[])));
}
