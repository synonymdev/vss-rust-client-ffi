use super::*;

const ID: &str = "9b1b6d9f-4a9e-40ca-8e77-c1d8e6dd48fb";

fn vector_store(principal: &str) -> PreparedMigrationStore {
    PreparedMigrationStore {
        domains: vec![BackupDomain::App],
        effective_principal: principal.into(),
        store_id: "wallet".into(),
        archive: Some(MigrationArchive {
            key: "WALLET".into(),
            ciphertext_sha256: None,
        }),
        initial_head: Some(PreparedMigrationHead {
            key: "HEAD".into(),
            encrypted_value: b"encrypted".to_vec(),
        }),
        objects: vec![],
    }
}

#[test]
fn matches_frozen_rust_java_server_vectors() {
    // Independent values from the preserved server migration_tests.rs / Java vectors.
    for (principals, expected) in [
        (
            ["app", "ldk"],
            "6dd7b9e48c13a00e3e8c6eec6ce1e84c4982416334bcc0b2421ffef0b3697d05",
        ),
        (
            ["\u{e000}", "\u{10000}"],
            "36a2158d68c2f784d7fc2f21d3a5e6b65d41c0c0306df48e834885edd5b28375",
        ),
    ] {
        let stores = principals.map(vector_store);
        assert_eq!(
            digest(ID, &stores),
            expected.parse::<sha256::Hash>().unwrap().to_byte_array()
        );
    }
}

#[test]
fn production_sort_matches_present_archive_component_unicode_vector() {
    let app = MigrationState {
        effective_principal: "\u{10000}".into(),
        store_id: "wallet".into(),
        policy: MigrationPolicy::Legacy,
    };
    let ldk = MigrationState {
        effective_principal: "\u{e000}".into(),
        store_id: "wallet".into(),
        policy: MigrationPolicy::Legacy,
    };
    let mut stores = physical_stores(&app, &ldk).unwrap();
    let index = store_index(&stores, BackupDomain::App);
    stores[index].archive = Some(MigrationArchive {
        key: "WALLET".into(),
        ciphertext_sha256: Some([7; 32]),
    });
    stores[index].initial_head = Some(PreparedMigrationHead {
        key: "HEAD".into(),
        encrypted_value: b"encrypted".to_vec(),
    });
    stores[index].objects = vec![
        MigrationObject {
            key: "component-\u{10000}".into(),
            ciphertext_sha256: [5; 32],
        },
        MigrationObject {
            key: "component-\u{e000}".into(),
            ciphertext_sha256: [3; 32],
        },
    ];
    validate_objects(&mut stores).unwrap();
    assert_eq!(stores[0].effective_principal, "\u{e000}");
    assert_eq!(stores[index].objects[0].key, "component-\u{e000}");
    // Independently encoded from the preserved server contract: 305 preimage bytes.
    let expected = "e2edca39e0c7f6b96cdf4f06fb8bedc6a1a219229505d8628b482ee5afa72250"
        .parse::<sha256::Hash>()
        .unwrap();
    assert_eq!(digest(ID, &stores), expected.to_byte_array());
}

#[test]
fn canonical_digest_binds_every_immutable_field() {
    let mut stores = [vector_store("app"), vector_store("ldk")];
    let initial = digest(ID, &stores);
    stores[1].archive.as_mut().unwrap().ciphertext_sha256 = Some([7; 32]);
    let present = digest(ID, &stores);
    assert_ne!(initial, present);
    stores[1].objects.push(MigrationObject {
        key: "component".into(),
        ciphertext_sha256: [3; 32],
    });
    let component = digest(ID, &stores);
    assert_ne!(present, component);
    stores[1].objects[0].ciphertext_sha256[0] ^= 1;
    assert_ne!(component, digest(ID, &stores));
    let before_head = digest(ID, &stores);
    stores[0]
        .initial_head
        .as_mut()
        .unwrap()
        .encrypted_value
        .push(0);
    assert_ne!(before_head, digest(ID, &stores));
    let before_principal = digest(ID, &stores);
    stores[0].effective_principal.push('x');
    assert_ne!(before_principal, digest(ID, &stores));
    let before_store = digest(ID, &stores);
    stores[0].store_id.push('x');
    assert_ne!(before_store, digest(ID, &stores));
    assert_ne!(
        digest(ID, &stores),
        digest("9b1b6d9f-4a9e-40ca-8e77-c1d8e6dd48fa", &stores)
    );
}

#[test]
fn rejects_duplicate_and_conflicting_physical_keys() {
    let mut store = vector_store("app");
    store.objects.push(MigrationObject {
        key: "WALLET".into(),
        ciphertext_sha256: [3; 32],
    });
    assert!(validate_objects(std::slice::from_mut(&mut store)).is_err());
    store.archive.as_mut().unwrap().ciphertext_sha256 = Some([3; 32]);
    validate_objects(std::slice::from_mut(&mut store)).unwrap();
    store.objects.push(MigrationObject {
        key: "WALLET".into(),
        ciphertext_sha256: [3; 32],
    });
    assert!(validate_objects(std::slice::from_mut(&mut store)).is_err());
    store.objects.pop();
    store.objects[0].key = "HEAD".into();
    assert!(validate_objects(std::slice::from_mut(&mut store)).is_err());
    store.objects.clear();
    store.initial_head.as_mut().unwrap().key = "WALLET".into();
    assert!(validate_objects(std::slice::from_mut(&mut store)).is_err());
}

#[test]
fn component_limit_is_per_physical_store() {
    let mut store = vector_store("app");
    store.objects = (0..32)
        .map(|n| MigrationObject {
            key: format!("key{n}"),
            ciphertext_sha256: [3; 32],
        })
        .collect();
    validate_objects(std::slice::from_mut(&mut store)).unwrap();
    store.objects.push(MigrationObject {
        key: "extra".into(),
        ciphertext_sha256: [3; 32],
    });
    assert!(validate_objects(std::slice::from_mut(&mut store)).is_err());
}
