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
        let seed = [42u8; 32]; // Test seed
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
