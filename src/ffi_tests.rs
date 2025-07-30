#[cfg(test)]
mod ffi_tests {
    use crate::*;
    
    // Unit tests for the FFI interface
    const MOCK_BASE_URL: &str = "https://vss.example.com";
    const TEST_STORE_ID: &str = "test-store-ffi";
    
    #[tokio::test]
    async fn test_ffi_client_lifecycle() {
        // Test that we can create and shutdown client without errors
        let result = vss_new_client(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string()
        ).await;
        
        assert!(result.is_ok());
        
        // Shutdown client
        vss_shutdown_client();
    }
    
    #[tokio::test]
    async fn test_ffi_client() {
        let result = vss_new_client(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string()
        ).await;
        
        assert!(result.is_ok());
        vss_shutdown_client();
    }
    
    #[tokio::test]
    async fn test_ffi_error_no_client() {
        // Don't initialize client, should get error
        let result = vss_get("any-key".to_string()).await;
        
        assert!(result.is_err());
        match result {
            Err(VssError::ConnectionError { error_details }) => {
                assert!(error_details.contains("not initialized"));
            }
            _ => panic!("Expected ConnectionError for uninitialized client"),
        }
    }
    
    #[tokio::test]
    async fn test_ffi_client_reinitialize() {
        // Test that we can create, shutdown, and recreate client
        vss_new_client(
            MOCK_BASE_URL.to_string(),
            TEST_STORE_ID.to_string()
        ).await.expect("Failed to create first client");
        
        vss_shutdown_client();
        
        // Should be able to create again
        let result = vss_new_client(
            MOCK_BASE_URL.to_string(),
            format!("{}-2", TEST_STORE_ID)
        ).await;
        
        assert!(result.is_ok());
        vss_shutdown_client();
    }
    
    /*
    // Integration tests for FFI functions would go here
    // These require a live VSS server - see tests.rs for setup instructions
    
    #[tokio::test]
    #[ignore = "requires live VSS server"]
    async fn integration_test_ffi_store_and_get() {
        vss_new_client(
            "https://your-vss-server.com".to_string(),
            "your-store-id".to_string(),
            None
        ).await.expect("Failed to create client");
        
        let key = format!("ffi-test-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis());
        let value = b"ffi-test-value".to_vec();
        
        let stored = vss_store(key.clone(), value.clone()).await
            .expect("Failed to store item");
        
        assert_eq!(stored.key, key);
        assert_eq!(stored.value, value);
        
        let retrieved = vss_get(key).await
            .expect("Failed to get item")
            .expect("Item should exist");
        
        assert_eq!(retrieved.value, value);
        
        vss_shutdown_client();
    }
    */
}