use super::*;
use crate::migration_status::wire;

const ID: &str = "9b1b6d9f-4a9e-40ca-8e77-c1d8e6dd48fb";

async fn prepared_fixture(coalesced: bool) -> (Fixture, BackupReadSession, PreparedMigration) {
    prepared_fixture_with_archive(coalesced, true).await
}

async fn prepared_fixture_with_archive(
    coalesced: bool,
    present: bool,
) -> (Fixture, BackupReadSession, PreparedMigration) {
    let fixture = Fixture::new().await;
    let session = fixture.session([42; 64]).await;
    if present {
        fixture.app_object(&session).await;
    }
    if coalesced {
        *fixture.default_principal.lock().await = Some("shared".into());
    }
    let app_status = session
        .read_migration_state(BackupDomain::App)
        .await
        .unwrap();
    let ldk_status = session
        .read_migration_state(BackupDomain::Ldk)
        .await
        .unwrap();
    let archive = session.read(app(), 4096).await.unwrap();
    let mut components = Vec::new();
    for key in ["snapshot/a", "snapshot/b"] {
        let storage_key = session.app.build_key(key);
        fixture.objects.lock().await.insert(
            storage_key.clone(),
            KeyValue {
                key: storage_key.clone(),
                version: 1,
                value: session
                    .app
                    .storable_builder
                    .build(
                        b"immutable component".to_vec(),
                        1,
                        &session.app.app_data_encryption_key,
                        storage_key.as_bytes(),
                    )
                    .encode_to_vec(),
            },
        );
        components.push(
            session
                .read(BackupReadTarget::App { key: key.into() }, 4096)
                .await
                .unwrap(),
        );
    }
    let prepared = session
        .prepare_migration(MigrationPreparation {
            migration_id: ID,
            app_status: &app_status,
            ldk_status: &ldk_status,
            archive: &archive,
            initial_head: MigrationHeadInput {
                key: "backup/head/v1",
                value: b"complete snapshot",
            },
            components: &components.iter().collect::<Vec<_>>(),
        })
        .unwrap();
    (fixture, session, prepared)
}

fn legacy(principal: &str) -> wire::State {
    wire::State {
        effective_principal: principal.into(),
        store_id: "owned-store".into(),
        ..Default::default()
    }
}

fn receipts(session: &BackupReadSession, prepared: &PreparedMigration) -> [wire::State; 2] {
    let content = session.prepared_migration(prepared).unwrap();
    [BackupDomain::App, BackupDomain::Ldk].map(|domain| {
        let store = content
            .stores()
            .iter()
            .find(|s| s.domains().contains(&domain))
            .unwrap();
        wire::State {
            effective_principal: store.effective_principal().into(),
            store_id: store.store_id().into(),
            epoch: 1,
            required_protocol: "bitkit-migration-v1".into(),
            migration_id: content.migration_id().into(),
            proposal_sha256: content.proposal_sha256().to_vec(),
            archive: store.archive().map(|a| wire::Archive {
                key: a.key.clone(),
                ciphertext_sha256: a.ciphertext_sha256.map(|d| d.to_vec()),
            }),
            initial_head: store.initial_head().map(|h| wire::Object {
                key: h.key().into(),
                ciphertext_sha256: sha256::Hash::hash(h.encrypted_value())
                    .to_byte_array()
                    .to_vec(),
            }),
            objects: store
                .objects()
                .iter()
                .map(|o| wire::Object {
                    key: o.key.clone(),
                    ciphertext_sha256: o.ciphertext_sha256.to_vec(),
                })
                .collect(),
        }
    })
}

async fn queue(fixture: &Fixture, states: &[wire::State; 2]) {
    fixture
        .status_queue
        .lock()
        .await
        .extend(states.iter().map(|s| (200, s.encode_to_vec())));
}

#[tokio::test]
async fn exact_receipts_observed_through_both_original_domains() {
    for coalesced in [false, true] {
        let (fixture, session, prepared) = prepared_fixture(coalesced).await;
        let states = receipts(&session, &prepared);
        let object_reads = fixture.requests.lock().await.len();
        queue(&fixture, &states).await;
        assert_eq!(
            session.reconcile_migration(&prepared).await.unwrap(),
            MigrationReconciliation::ObservedCommitted
        );
        assert!(matches!(
            session.prepared_migration(&prepared),
            Err(MigrationPreparationError::StalePolicy)
        ));
        let principals = fixture.status_principals.lock().await;
        assert_eq!(principals.len(), 4);
        assert_eq!(principals[0] == principals[1], coalesced);
        let signers = fixture.auth_keys.lock().await;
        assert_eq!(signers.len(), 4);
        assert_ne!(signers[0], signers[1]);
        assert_eq!(signers[..2], signers[2..]);
        drop(signers);
        assert_eq!(principals[..2], principals[2..]);
        assert_eq!(fixture.requests.lock().await.len(), object_reads);
        // A retained stale attempt remains usable only for another read-only comparison.
        drop(principals);
        queue(&fixture, &states).await;
        assert_eq!(
            session.reconcile_migration(&prepared).await.unwrap(),
            MigrationReconciliation::ObservedCommitted
        );
        assert!(!fixture.task.is_finished());
    }
}

#[tokio::test]
async fn both_legacy_observations_do_not_restore_preparation_access() {
    let (fixture, session, prepared) = prepared_fixture(false).await;
    assert_eq!(
        session.reconcile_migration(&prepared).await.unwrap(),
        MigrationReconciliation::ObservedUncommitted
    );
    assert_eq!(fixture.status_requests.lock().await.len(), 4);
    assert!(matches!(
        session.prepared_migration(&prepared),
        Err(MigrationPreparationError::StalePolicy)
    ));
}

#[tokio::test]
async fn mixed_committed_and_legacy_is_not_a_positive_outcome() {
    for legacy_domain in 0..2 {
        let (fixture, session, prepared) = prepared_fixture(false).await;
        let mut states = receipts(&session, &prepared);
        states[legacy_domain] = legacy(&states[legacy_domain].effective_principal);
        queue(&fixture, &states).await;
        assert!(matches!(
            session.reconcile_migration(&prepared).await,
            Err(MigrationReconciliationError::MixedOutcome)
        ));
    }
}

#[tokio::test]
async fn remapped_original_provider_never_counts_as_the_old_participant() {
    for coalesced in [false, true] {
        for index in 0..2 {
            let (fixture, session, prepared) = prepared_fixture(coalesced).await;
            let mut states = receipts(&session, &prepared);
            states[index].effective_principal = "replacement-principal".into();
            queue(&fixture, &states).await;
            assert!(matches!(session.reconcile_migration(&prepared).await,
                Err(MigrationReconciliationError::ParticipantChanged(domain)) if domain.index() == index));
        }
    }
}

#[tokio::test]
async fn every_initial_receipt_field_is_bound_to_the_retained_attempt() {
    let (fixture, session, prepared) = prepared_fixture(false).await;
    let original = receipts(&session, &prepared);
    let mutations: [fn(&mut wire::State); 13] = [
        |s| s.migration_id = "1b1b6d9f-4a9e-40ca-8e77-c1d8e6dd48fb".into(),
        |s| s.proposal_sha256[0] ^= 1,
        |s| s.archive = None,
        |s| s.archive.as_mut().unwrap().key = "foreign/archive".into(),
        |s| s.archive.as_mut().unwrap().ciphertext_sha256 = None,
        |s| {
            s.archive
                .as_mut()
                .unwrap()
                .ciphertext_sha256
                .as_mut()
                .unwrap()[0] ^= 1
        },
        |s| s.initial_head = None,
        |s| s.initial_head.as_mut().unwrap().key = "foreign/head".into(),
        |s| s.initial_head.as_mut().unwrap().ciphertext_sha256[0] ^= 1,
        |s| {
            s.objects.pop();
        },
        |s| {
            s.objects.push(wire::Object {
                key: "extra/object".into(),
                ciphertext_sha256: vec![7; 32],
            })
        },
        |s| s.objects[0].key = "foreign/component".into(),
        |s| s.objects[0].ciphertext_sha256[0] ^= 1,
    ];
    for mutate in mutations {
        let mut states = original.clone();
        mutate(&mut states[0]);
        queue(&fixture, &states).await;
        assert!(matches!(
            session.reconcile_migration(&prepared).await,
            Err(MigrationReconciliationError::ReceiptMismatch(
                BackupDomain::App
            ))
        ));
    }
    let mut states = original.clone();
    states[1].proposal_sha256[0] ^= 1;
    queue(&fixture, &states).await;
    assert!(matches!(
        session.reconcile_migration(&prepared).await,
        Err(MigrationReconciliationError::ReceiptMismatch(
            BackupDomain::Ldk
        ))
    ));
}

#[tokio::test]
async fn component_order_and_later_mutable_head_do_not_change_initial_receipt() {
    let (fixture, session, prepared) = prepared_fixture(false).await;
    let mut states = receipts(&session, &prepared);
    let head = session
        .prepared_migration(&prepared)
        .unwrap()
        .stores()
        .iter()
        .find_map(|s| s.initial_head())
        .unwrap();
    fixture.objects.lock().await.insert(
        head.key().into(),
        KeyValue {
            key: head.key().into(),
            version: 99,
            value: b"later head is not the immutable receipt".to_vec(),
        },
    );
    states[0].objects.reverse();
    let reads = fixture.requests.lock().await.len();
    queue(&fixture, &states).await;
    assert_eq!(
        session.reconcile_migration(&prepared).await.unwrap(),
        MigrationReconciliation::ObservedCommitted
    );
    assert_eq!(fixture.requests.lock().await.len(), reads);
}

#[tokio::test]
async fn unavailable_malformed_unauthorized_and_wrong_store_are_read_errors() {
    for code in [302, 401, 404, 503, 200] {
        let (fixture, session, prepared) = prepared_fixture(false).await;
        let states = receipts(&session, &prepared);
        fixture
            .status_queue
            .lock()
            .await
            .extend([(200, states[0].encode_to_vec()), (code, vec![0xff])]);
        assert!(matches!(
            session.reconcile_migration(&prepared).await,
            Err(MigrationReconciliationError::Read {
                domain: BackupDomain::Ldk,
                ..
            })
        ));
        assert_eq!(fixture.status_requests.lock().await.len(), 4);
        assert_eq!(fixture.redirect_requests.load(Ordering::SeqCst), 0);
        assert!(matches!(
            session.prepared_migration(&prepared),
            Err(MigrationPreparationError::StalePolicy)
        ));
    }
    let (fixture, session, prepared) = prepared_fixture(false).await;
    let mut states = receipts(&session, &prepared);
    states[1].store_id = "other-store".into();
    queue(&fixture, &states).await;
    assert!(matches!(
        session.reconcile_migration(&prepared).await,
        Err(MigrationReconciliationError::Read {
            domain: BackupDomain::Ldk,
            ..
        })
    ));
}

#[tokio::test]
async fn replacement_and_dropped_owner_cannot_adopt_a_prepared_uuid() {
    let (fixture, session, prepared) = prepared_fixture(false).await;
    let replacement = fixture.session([42; 64]).await;
    for dropped in [false, true] {
        if dropped {
            session.revoke();
        }
        assert!(matches!(
            replacement.reconcile_migration(&prepared).await,
            Err(MigrationReconciliationError::Ownership(
                BackupReadError::DifferentOwner
            ))
        ));
    }
    drop(session);
    assert!(matches!(
        replacement.reconcile_migration(&prepared).await,
        Err(MigrationReconciliationError::Ownership(
            BackupReadError::DifferentOwner
        ))
    ));
    assert_eq!(fixture.status_requests.lock().await.len(), 2);
}

#[tokio::test]
async fn revocation_discards_a_held_receipt_and_prevents_second_read() {
    let (fixture, session, prepared) = prepared_fixture(false).await;
    queue(&fixture, &receipts(&session, &prepared)).await;
    *fixture.hold.lock().await = Some("/getStoreMigrationState".into());
    let session = Arc::new(session);
    let owner = session.clone();
    let mut task = tokio::spawn(async move { owner.reconcile_migration(&prepared).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::select! {
            _ = fixture.received.notified() => {},
            result = &mut task => panic!("operation ended before held boundary: {result:?}"),
        }
    })
    .await
    .expect("held receipt boundary");
    session.revoke();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap(),
        Err(MigrationReconciliationError::Ownership(
            BackupReadError::Revoked
        ))
    ));
    assert_eq!(fixture.status_requests.lock().await.len(), 3);
    fixture.release.notify_one();
}

#[tokio::test]
async fn abandoned_reconciliation_does_not_restore_or_schedule_continuation() {
    let (fixture, session, prepared) = prepared_fixture(false).await;
    queue(&fixture, &receipts(&session, &prepared)).await;
    *fixture.hold.lock().await = Some("/getStoreMigrationState".into());
    let mut future = Box::pin(session.reconcile_migration(&prepared));
    tokio::select! {
        biased;
        _ = fixture.received.notified() => {},
        _ = &mut future => panic!("held read completed"),
    }
    drop(future);
    fixture.release.notify_one();
    assert!(matches!(
        session.prepared_migration(&prepared),
        Err(MigrationPreparationError::StalePolicy)
    ));
    assert_eq!(fixture.status_requests.lock().await.len(), 3);
}

#[tokio::test]
async fn overlapping_status_read_supersedes_the_reconciliation_pair() {
    let (fixture, session, prepared) = prepared_fixture(false).await;
    queue(&fixture, &receipts(&session, &prepared)).await;
    fixture.status_hold_at.store(4, Ordering::SeqCst);
    let mut reconciliation = Box::pin(session.reconcile_migration(&prepared));
    tokio::select! {
        biased;
        _ = fixture.received.notified() => {},
        _ = &mut reconciliation => panic!("second read did not hold"),
    }
    let mut newer = Box::pin(session.read_migration_state(BackupDomain::App));
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(newer.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(newer);
    fixture.release.notify_one();
    assert!(matches!(
        reconciliation.await,
        Err(MigrationReconciliationError::SupersededObservation)
    ));
}

#[tokio::test]
async fn explicit_archive_absence_is_a_bound_condition() {
    let (fixture, session, prepared) = prepared_fixture_with_archive(false, false).await;
    let mut states = receipts(&session, &prepared);
    assert!(states[0]
        .archive
        .as_ref()
        .unwrap()
        .ciphertext_sha256
        .is_none());
    queue(&fixture, &states).await;
    assert_eq!(
        session.reconcile_migration(&prepared).await.unwrap(),
        MigrationReconciliation::ObservedCommitted
    );
    states[0].archive.as_mut().unwrap().ciphertext_sha256 = Some(vec![7; 32]);
    queue(&fixture, &states).await;
    assert!(matches!(
        session.reconcile_migration(&prepared).await,
        Err(MigrationReconciliationError::ReceiptMismatch(
            BackupDomain::App
        ))
    ));
}

#[tokio::test]
async fn ldk_receipt_cannot_add_unprepared_archive_head_or_components() {
    let (fixture, session, prepared) = prepared_fixture(false).await;
    let original = receipts(&session, &prepared);
    for field in 0..3 {
        let mut states = original.clone();
        match field {
            0 => states[1].archive = states[0].archive.clone(),
            1 => states[1].initial_head = states[0].initial_head.clone(),
            _ => states[1].objects = states[0].objects.clone(),
        }
        queue(&fixture, &states).await;
        assert!(matches!(
            session.reconcile_migration(&prepared).await,
            Err(MigrationReconciliationError::ReceiptMismatch(
                BackupDomain::Ldk
            ))
        ));
    }
}

#[tokio::test]
async fn revocation_during_second_read_discards_the_matching_first_receipt() {
    let (fixture, session, prepared) = prepared_fixture(false).await;
    queue(&fixture, &receipts(&session, &prepared)).await;
    fixture.status_hold_at.store(4, Ordering::SeqCst);
    let mut future = Box::pin(session.reconcile_migration(&prepared));
    tokio::select! {
        biased;
        _ = fixture.received.notified() => {},
        _ = &mut future => panic!("second receipt did not hold"),
    }
    session.revoke();
    assert!(matches!(
        future.await,
        Err(MigrationReconciliationError::Ownership(
            BackupReadError::Revoked
        ))
    ));
    fixture.release.notify_one();
}

#[tokio::test]
async fn remapped_legacy_participant_is_not_observed_uncommitted() {
    for coalesced in [false, true] {
        for index in 0..2 {
            let (fixture, session, prepared) = prepared_fixture(coalesced).await;
            let committed = receipts(&session, &prepared);
            let mut states = committed.map(|s| legacy(&s.effective_principal));
            states[index].effective_principal = "remapped".into();
            queue(&fixture, &states).await;
            assert!(matches!(session.reconcile_migration(&prepared).await,
                Err(MigrationReconciliationError::ParticipantChanged(domain)) if domain.index() == index));
        }
    }
}

#[tokio::test]
async fn issuer_remap_is_observed_even_while_old_tokens_remain_valid() {
    for coalesced in [false, true] {
        for index in 0..2 {
            let (fixture, session, prepared) = prepared_fixture(coalesced).await;
            let states = receipts(&session, &prepared);
            let signer = fixture.auth_keys.lock().await[index].clone();
            fixture
                .issued_principals
                .lock()
                .await
                .insert(signer, "remapped".into());
            fixture.status_uses_token.store(true, Ordering::SeqCst);
            // Control: the normal provider still has the original unexpired JWT.
            let domain = [BackupDomain::App, BackupDomain::Ldk][index];
            let cached = session.read_migration_state(domain).await.unwrap();
            assert_eq!(
                session
                    .migration_state(&cached)
                    .unwrap()
                    .effective_principal,
                states[index].effective_principal
            );
            // Reconciliation must freshly exercise the same signer, not that cached JWT.
            queue(&fixture, &states).await;
            assert!(matches!(session.reconcile_migration(&prepared).await,
                Err(MigrationReconciliationError::ParticipantChanged(observed)) if observed == domain));
            assert_eq!(fixture.auth_keys.lock().await.len(), 4);
        }
    }
}

#[tokio::test]
async fn issuer_unavailable_does_not_fall_back_to_valid_cached_credentials() {
    let (fixture, session, prepared) = prepared_fixture(false).await;
    fixture.refuse_auth.store(true, Ordering::SeqCst);
    let cached = session
        .read_migration_state(BackupDomain::App)
        .await
        .unwrap();
    assert!(matches!(
        session.migration_state(&cached).unwrap().policy,
        crate::MigrationPolicy::Legacy
    ));
    assert!(matches!(
        session.reconcile_migration(&prepared).await,
        Err(MigrationReconciliationError::Read {
            domain: BackupDomain::App,
            ..
        })
    ));
    assert_eq!(fixture.status_requests.lock().await.len(), 3);
}
