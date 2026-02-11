#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::types::LdkNamespace;

    // Unit tests for client creation and basic functionality
    //
    // For integration tests with a real VSS server, you can create a separate test file
    // and update the constants to point to your VSS server instance.

    const MOCK_BASE_URL: &str = "https://vss.example.com";
    const TEST_STORE_ID: &str = "test-store-rust-ffi";

    #[tokio::test]
    async fn test_vss_client_creation() {
        let result = VssClient::new(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string()
        ).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_vss_client_creation_empty_base_url() {
        let result = VssClient::new(
            "".to_string(),
            TEST_STORE_ID.to_string(),
        ).await;

        // Should still create client successfully, errors happen on actual operations
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_vss_client_creation_empty_store_id() {
        let result = VssClient::new(
            MOCK_BASE_URL.to_string(),
            "".to_string(),
        ).await;

        // Should still create client successfully, errors happen on actual operations
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_vss_client_creation_with_lnurl_auth() {
        let seed = [42u8; 64]; // Test seed (full BIP39 seed)
        let result = VssClient::new_with_lnurl_auth(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string(),
            seed,
            "https://auth.example.com/lnurl".to_string()
        ).await;

        // Should create client successfully (auth errors happen on actual requests)
        assert!(result.is_ok());
    }

    #[test]
    fn test_error_types() {
        use crate::VssError;

        // Test that our error types can be created
        let connection_err = VssError::ConnectionError {
            error_details: "Test connection error".to_string()
        };
        let store_err = VssError::StoreError {
            error_details: "Test store error".to_string()
        };
        let get_err = VssError::GetError {
            error_details: "Test get error".to_string()
        };

        // Test error display
        assert!(format!("{}", connection_err).contains("Test connection error"));
        assert!(format!("{}", store_err).contains("Test store error"));
        assert!(format!("{}", get_err).contains("Test get error"));
    }

    #[test]
    fn test_vss_derive_store_id() {
        use crate::vss_derive_store_id;

        let prefix = "test".to_string();
        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string();

        // Test deterministic output
        let store_id1 = vss_derive_store_id(prefix.clone(), mnemonic.clone(), None).unwrap();
        let store_id2 = vss_derive_store_id(prefix.clone(), mnemonic.clone(), None).unwrap();
        assert_eq!(store_id1, store_id2);
        assert!(store_id1.starts_with("test_"));

        // Test passphrase handling
        let with_passphrase = vss_derive_store_id(prefix.clone(), mnemonic.clone(), Some("pass".to_string())).unwrap();
        assert_ne!(store_id1, with_passphrase);

        // Test invalid mnemonic
        assert!(vss_derive_store_id(prefix, "invalid".to_string(), None).is_err());
    }

    #[tokio::test]
    async fn test_key_derivation_app_differs_from_ldk() {
        use bitcoin::bip32::{ChildNumber, Xpriv};
        use bitcoin::secp256k1::Secp256k1;
        use bitcoin::Network;
        use crate::implementation::derive_data_encryption_and_obfuscation_keys;

        let full_seed: [u8; 64] = [42u8; 64];
        let truncated_seed: [u8; 32] = full_seed[..32].try_into().unwrap();

        let secp = Secp256k1::new();

        // LDK keys: derived from full 64-byte seed
        let ldk_master = Xpriv::new_master(Network::Bitcoin, &full_seed).unwrap();
        let ldk_vss = ldk_master
            .derive_priv(&secp, &[ChildNumber::Hardened { index: 877 }])
            .unwrap();
        let ldk_seed_bytes: [u8; 32] = ldk_vss.private_key.secret_bytes();

        // App keys: derived from truncated 32-byte seed
        let app_master = Xpriv::new_master(Network::Bitcoin, &truncated_seed).unwrap();
        let app_vss = app_master
            .derive_priv(&secp, &[ChildNumber::Hardened { index: 877 }])
            .unwrap();
        let app_seed_bytes: [u8; 32] = app_vss.private_key.secret_bytes();

        assert_ne!(ldk_seed_bytes, app_seed_bytes);

        let (ldk_enc, ldk_obf) = derive_data_encryption_and_obfuscation_keys(&ldk_seed_bytes);
        let (app_enc, app_obf) = derive_data_encryption_and_obfuscation_keys(&app_seed_bytes);

        assert_ne!(ldk_enc, app_enc);
        assert_ne!(ldk_obf, app_obf);
    }

    #[tokio::test]
    async fn test_client_with_lnurl_auth_has_separate_keys() {
        let seed = [42u8; 64];
        let client = VssClient::new_with_lnurl_auth(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string(),
            seed,
            "https://auth.example.com/lnurl".to_string(),
        )
        .await
        .unwrap();

        assert!(client.app_key_obfuscator.is_some());
        assert!(client.ldk_key_obfuscator.is_some());
        assert_ne!(
            client.app_data_encryption_key,
            client.ldk_data_encryption_key
        );
    }

    #[tokio::test]
    async fn test_client_without_auth_has_no_keys() {
        let client = VssClient::new(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string(),
        )
        .await
        .unwrap();

        assert!(client.app_key_obfuscator.is_none());
        assert!(client.ldk_key_obfuscator.is_none());
    }

    #[tokio::test]
    async fn test_build_and_extract_app_key() {
        let seed = [42u8; 64];
        let client = VssClient::new_with_lnurl_auth(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string(),
            seed,
            "https://auth.example.com/lnurl".to_string(),
        )
        .await
        .unwrap();

        let app_storage_key = client.build_key("test_key");

        // App key should be obfuscated
        assert_ne!(app_storage_key, "test_key");

        // Should deobfuscate back to original
        assert_eq!(client.extract_key(&app_storage_key).unwrap(), "test_key");
    }

    #[tokio::test]
    async fn test_build_and_extract_ldk_key() {
        let seed = [42u8; 64];
        let client = VssClient::new_with_lnurl_auth(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string(),
            seed,
            "https://auth.example.com/lnurl".to_string(),
        )
        .await
        .unwrap();

        let ldk_storage_key = client.build_key_ldk("graph", &LdkNamespace::Default);

        // LDK key should be obfuscated with namespace prefix
        assert_ne!(ldk_storage_key, "graph");

        // Should deobfuscate back to original
        assert_eq!(client.extract_key_ldk(&ldk_storage_key).unwrap(), "graph");
    }

    #[tokio::test]
    async fn test_build_prefix_ldk_is_prefix_of_build_key_ldk() {
        let seed = [42u8; 64];
        let client = VssClient::new_with_lnurl_auth(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string(),
            seed,
            "https://auth.example.com/lnurl".to_string(),
        )
        .await
        .unwrap();

        for namespace in &[
            LdkNamespace::Default,
            LdkNamespace::Monitors,
            LdkNamespace::ArchivedMonitors,
            LdkNamespace::MonitorUpdates { monitor_id: "abc123".to_string() },
        ] {
            let prefix = client.build_prefix_ldk(namespace);
            let full_key = client.build_key_ldk("some_key", namespace);

            // The prefix must be a string prefix of any full key in the same namespace
            assert!(
                full_key.starts_with(&prefix),
                "prefix {:?} is not a prefix of key {:?} for namespace {:?}",
                prefix, full_key, namespace
            );
        }
    }

    #[tokio::test]
    async fn test_app_and_ldk_keys_are_different() {
        let seed = [42u8; 64];
        let client = VssClient::new_with_lnurl_auth(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string(),
            seed,
            "https://auth.example.com/lnurl".to_string(),
        )
        .await
        .unwrap();

        let app_storage_key = client.build_key("test_key");
        let ldk_storage_key = client.build_key_ldk("test_key", &LdkNamespace::Default);

        // App and LDK storage keys must differ (different obfuscators + format)
        assert_ne!(app_storage_key, ldk_storage_key);
    }

    #[test]
    fn test_store_id_unchanged() {
        use crate::vss_derive_store_id;

        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string();

        // derive_vss_store_id always uses truncated 32-byte seed,
        // so the store_id is the same regardless of the client key fix
        let id1 = vss_derive_store_id("test".to_string(), mnemonic.clone(), None).unwrap();
        let id2 = vss_derive_store_id("test".to_string(), mnemonic.clone(), None).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_types_creation() {
        use crate::{VssItem, KeyValue, KeyVersion};

        // Test creating VssItem
        let item = VssItem {
            key: "test-key".to_string(),
            value: vec![1, 2, 3, 4],
            version: 1,
        };
        assert_eq!(item.key, "test-key");
        assert_eq!(item.value, vec![1, 2, 3, 4]);
        assert_eq!(item.version, 1);

        // Test creating KeyValue
        let kv = KeyValue {
            key: "kv-key".to_string(),
            value: vec![5, 6, 7, 8],
        };
        assert_eq!(kv.key, "kv-key");
        assert_eq!(kv.value, vec![5, 6, 7, 8]);

        // Test creating KeyVersion
        let key_version = KeyVersion {
            key: "version-key".to_string(),
            version: 42,
        };
        assert_eq!(key_version.key, "version-key");
        assert_eq!(key_version.version, 42);
    }

    /*
    // Integration tests would go here - these require a live VSS server
    // To run integration tests:
    // 1. Start a VSS server or get access to one
    // 2. Update INTEGRATION_BASE_URL and INTEGRATION_STORE_ID below
    // 3. Uncomment the tests and run with: cargo test --ignored

    const INTEGRATION_BASE_URL: &str = "https://your-vss-server.com";
    const INTEGRATION_STORE_ID: &str = "your-store-id";

    #[tokio::test]
    #[ignore = "requires live VSS server"]
    async fn integration_test_store_and_get() {
        let client = VssClient::new(
            INTEGRATION_BASE_URL.to_string(),
            INTEGRATION_STORE_ID.to_string(),
        ).await.expect("Failed to create client");

        let key = format!("integration-test-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis());
        let value = b"integration-test-value".to_vec();

        let stored = client.store(key.clone(), value.clone()).await
            .expect("Failed to store item");

        assert_eq!(stored.key, key);
        assert_eq!(stored.value, value);

        let retrieved = client.get(key).await
            .expect("Failed to get item")
            .expect("Item should exist");

        assert_eq!(retrieved.value, value);
    }
    */
}
