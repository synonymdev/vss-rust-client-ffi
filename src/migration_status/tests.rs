use super::*;

fn legacy() -> wire::State {
    wire::State {
        effective_principal: "principal".into(),
        store_id: "store".into(),
        ..Default::default()
    }
}
fn committed_state() -> wire::State {
    wire::State {
        epoch: 1,
        required_protocol: PROTOCOL.into(),
        migration_id: "00000000-0000-4000-8000-000000000001".into(),
        proposal_sha256: vec![1; 32],
        archive: Some(wire::Archive {
            key: "old-wallet".into(),
            ciphertext_sha256: Some(vec![2; 32]),
        }),
        initial_head: Some(wire::Object {
            key: "head".into(),
            ciphertext_sha256: vec![3; 32],
        }),
        objects: vec![wire::Object {
            key: "part".into(),
            ciphertext_sha256: vec![4; 32],
        }],
        ..legacy()
    }
}
fn read(state: wire::State) -> Result<MigrationState, VssError> {
    decode(&state.encode_to_vec(), "store")
}

#[test]
fn explicit_legacy_identity_is_required() {
    let state = read(legacy()).unwrap();
    assert_eq!(state.effective_principal, "principal");
    assert!(matches!(state.policy, MigrationPolicy::Legacy));
    assert!(read(wire::State::default()).is_err());
    for s in [
        wire::State {
            store_id: "another".into(),
            ..legacy()
        },
        wire::State {
            effective_principal: String::new(),
            ..legacy()
        },
        wire::State {
            effective_principal: "x".repeat(121),
            ..legacy()
        },
        wire::State {
            migration_id: "anything".into(),
            ..legacy()
        },
        wire::State {
            required_protocol: PROTOCOL.into(),
            ..legacy()
        },
        wire::State {
            proposal_sha256: vec![1; 32],
            ..legacy()
        },
        wire::State {
            archive: committed_state().archive,
            ..legacy()
        },
        wire::State {
            initial_head: committed_state().initial_head,
            ..legacy()
        },
        wire::State {
            objects: committed_state().objects,
            ..legacy()
        },
    ] {
        assert!(read(s).is_err());
    }
}

#[test]
fn committed_state_retains_exact_conditions_and_allows_ldk_only_tuple() {
    let MigrationPolicy::Committed(state) = read(committed_state()).unwrap().policy else {
        panic!("expected committed");
    };
    assert_eq!(state.proposal_sha256, [1; 32]);
    assert_eq!(state.archive.unwrap().ciphertext_sha256, Some([2; 32]));
    assert_eq!(state.initial_head.unwrap().ciphertext_sha256, [3; 32]);
    assert_eq!(state.objects[0].ciphertext_sha256, [4; 32]);
    let mut absent = committed_state();
    absent.archive.as_mut().unwrap().ciphertext_sha256 = None;
    let MigrationPolicy::Committed(absent) = read(absent).unwrap().policy else {
        panic!("expected committed");
    };
    assert!(absent.archive.unwrap().ciphertext_sha256.is_none());
    let mut ldk = committed_state();
    ldk.archive = None;
    ldk.initial_head = None;
    ldk.objects.clear();
    assert!(read(ldk).is_ok());
}

#[test]
fn ambiguous_committed_shapes_are_rejected() {
    let mutations: &[fn(&mut wire::State)] = &[
        (|s| s.epoch = -1),
        (|s| s.epoch = 2),
        (|s| s.required_protocol = "future-v2".into()),
        (|s| s.required_protocol.clear()),
        (|s| s.migration_id = "A".repeat(36)),
        (|s| s.migration_id.clear()),
        (|s| s.proposal_sha256.pop().map(|_| ()).unwrap()),
        (|s| s.archive.as_mut().unwrap().ciphertext_sha256 = Some(vec![])),
        (|s| s.initial_head.as_mut().unwrap().ciphertext_sha256.clear()),
        (|s| s.objects[0].ciphertext_sha256.push(1)),
        (|s| s.objects.push(s.objects[0].clone())),
        (|s| {
            s.objects = (0..33)
                .map(|i| wire::Object {
                    key: format!("part-{i}"),
                    ciphertext_sha256: vec![4; 32],
                })
                .collect()
        }),
        (|s| s.archive.as_mut().unwrap().key = "head".into()),
        (|s| s.objects[0].key = "head".into()),
        (|s| s.objects[0].key = "old-wallet".into()),
        (|s| s.objects[0].key = "global_version".into()),
        (|s| s.objects[0].key = "vss_global_version".into()),
        (|s| s.objects[0].key.clear()),
        (|s| s.objects[0].key = "x".repeat(601)),
    ];
    for (i, mutate) in mutations.iter().enumerate() {
        let mut s = committed_state();
        mutate(&mut s);
        assert!(read(s).is_err(), "mutation {i} admitted");
    }
    let mut same = committed_state();
    same.objects[0] = wire::Object {
        key: "old-wallet".into(),
        ciphertext_sha256: vec![2; 32],
    };
    assert!(read(same).is_ok());
}

#[test]
fn duplicate_unknown_overflow_and_truncated_wire_fields_fail_closed() {
    let base = legacy().encode_to_vec();
    for suffix in [
        vec![0x42, 1, b'x'],
        vec![0x4a, 5, b's', b't', b'o', b'r', b'e'],
        vec![0x08, 0x00, 0x08, 0x00],
        vec![0x50, 0],
        vec![0x08, 0x80, 0x80, 0x80, 0x80, 0x10],
        vec![
            0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01,
        ],
        vec![0x42, 0xff],
        vec![0x08, 0x80],
        vec![0x0a, 0],
    ] {
        let mut bytes = base.clone();
        bytes.extend(suffix);
        assert!(decode(&bytes, "store").is_err());
    }
    let good = committed_state().encode_to_vec();
    for n in 0..good.len() {
        assert!(decode(&good[..n], "store").is_err(), "truncated at {n}");
    }
    // Prost would merge these nested singular keys and keep the last digest.
    let mut state = committed_state();
    state.archive = None;
    let mut control = state.encode_to_vec();
    control.extend([0x2a, 3, 0x0a, 1, b'b']);
    assert!(decode(&control, "store").is_ok());
    let mut bytes = state.encode_to_vec();
    bytes.extend([0x2a, 6, 0x0a, 1, b'a', 0x0a, 1, b'b']);
    assert!(decode(&bytes, "store").is_err());
    let mut max = committed_state();
    max.objects = (0..32)
        .map(|i| wire::Object {
            key: format!("part-{i}"),
            ciphertext_sha256: vec![4; 32],
        })
        .collect();
    assert!(read(max).is_ok());
}

#[test]
fn legacy_identities_preserve_character_bounded_storage_names() {
    let identity = "é".repeat(120);
    let state = wire::State {
        effective_principal: identity.clone(),
        store_id: identity.clone(),
        ..Default::default()
    };
    assert!(decode(&state.encode_to_vec(), &identity).is_ok());
    let mut committed = committed_state();
    committed.effective_principal = identity;
    assert!(read(committed).is_err());
}
