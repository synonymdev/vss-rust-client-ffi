#[cfg(test)]
mod tests {
    use super::super::*;

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
    async fn test_legacy_key_derivation_differs_from_new() {
        use bitcoin::bip32::{ChildNumber, Xpriv};
        use bitcoin::secp256k1::Secp256k1;
        use bitcoin::Network;
        use crate::implementation::derive_data_encryption_and_obfuscation_keys;

        let full_seed: [u8; 64] = [42u8; 64];
        let truncated_seed: [u8; 32] = full_seed[..32].try_into().unwrap();

        let secp = Secp256k1::new();

        let new_master = Xpriv::new_master(Network::Bitcoin, &full_seed).unwrap();
        let new_vss = new_master
            .derive_priv(&secp, &[ChildNumber::Hardened { index: 877 }])
            .unwrap();
        let new_seed_bytes: [u8; 32] = new_vss.private_key.secret_bytes();

        let old_master = Xpriv::new_master(Network::Bitcoin, &truncated_seed).unwrap();
        let old_vss = old_master
            .derive_priv(&secp, &[ChildNumber::Hardened { index: 877 }])
            .unwrap();
        let old_seed_bytes: [u8; 32] = old_vss.private_key.secret_bytes();

        assert_ne!(new_seed_bytes, old_seed_bytes);

        let (new_enc, new_obf) = derive_data_encryption_and_obfuscation_keys(&new_seed_bytes);
        let (old_enc, old_obf) = derive_data_encryption_and_obfuscation_keys(&old_seed_bytes);

        assert_ne!(new_enc, old_enc);
        assert_ne!(new_obf, old_obf);
    }

    #[tokio::test]
    async fn test_client_with_lnurl_auth_has_legacy_keys() {
        let seed = [42u8; 64];
        let client = VssClient::new_with_lnurl_auth(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string(),
            seed,
            "https://auth.example.com/lnurl".to_string(),
        )
        .await
        .unwrap();

        assert!(client.legacy_data_encryption_key.is_some());
        assert!(client.legacy_key_obfuscator.is_some());
        assert_ne!(
            client.data_encryption_key,
            client.legacy_data_encryption_key.unwrap()
        );
    }

    #[tokio::test]
    async fn test_client_without_auth_has_no_legacy_keys() {
        let client = VssClient::new(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string(),
        )
        .await
        .unwrap();

        assert!(client.legacy_data_encryption_key.is_none());
        assert!(client.legacy_key_obfuscator.is_none());
    }

    #[tokio::test]
    async fn test_extract_key_with_both_obfuscators() {
        let seed = [42u8; 64];
        let client = VssClient::new_with_lnurl_auth(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string(),
            seed,
            "https://auth.example.com/lnurl".to_string(),
        )
        .await
        .unwrap();

        let new_storage_key = client.build_key("test_key", None, None);
        let legacy_storage_key = client.build_legacy_key("test_key", None, None).unwrap();

        assert_ne!(new_storage_key, legacy_storage_key);

        // Both should deobfuscate to the original key
        assert_eq!(client.extract_key(&new_storage_key).unwrap(), "test_key");
        assert_eq!(client.extract_key(&legacy_storage_key).unwrap(), "test_key");
    }

    #[tokio::test]
    async fn test_extract_key_with_namespaces() {
        let seed = [42u8; 64];
        let client = VssClient::new_with_lnurl_auth(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string(),
            seed,
            "https://auth.example.com/lnurl".to_string(),
        )
        .await
        .unwrap();

        let ns1 = Some("");
        let ns2 = Some("");
        let new_key = client.build_key("graph", ns1, ns2);
        let legacy_key = client.build_legacy_key("graph", ns1, ns2).unwrap();

        assert_ne!(new_key, legacy_key);

        assert_eq!(client.extract_key(&new_key).unwrap(), "graph");
        assert_eq!(client.extract_key(&legacy_key).unwrap(), "graph");
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
