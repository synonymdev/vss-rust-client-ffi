use super::errors::VssError;
use super::types::*;
use vss_client::client::VssClient as ExternalVssClient;
use vss_client::types::{
    KeyValue as ExternalKeyValue, 
    GetObjectRequest, PutObjectRequest, DeleteObjectRequest, ListKeyVersionsRequest
};
use vss_client::error::VssError as ExternalVssError;

#[derive(Clone)]
pub struct VssClient {
    inner: ExternalVssClient,
    store_id: String,
}

impl VssClient {
    /// Creates a new VSS client instance.
    ///
    /// # Parameters
    /// - `base_url`: The VSS server URL
    /// - `store_id`: The storage namespace identifier
    ///
    /// # Returns
    /// A new VssClient instance or VssError on failure
    pub async fn new(base_url: String, store_id: String) -> Result<Self, VssError> {
        let client = ExternalVssClient::new(&base_url);
        
        Ok(VssClient {
            inner: client,
            store_id,
        })
    }
    
    /// Stores a key-value pair. Server manages versioning automatically.
    ///
    /// # Parameters
    /// - `key`: The unique key identifier
    /// - `value`: The binary data to store
    ///
    /// # Returns
    /// VssItem with the stored data and assigned version
    pub async fn store(&self, key: String, value: Vec<u8>) -> Result<VssItem, VssError> {
        let request = PutObjectRequest {
            store_id: self.store_id.clone(),
            global_version: None,
            transaction_items: vec![ExternalKeyValue {
                key: key.clone(),
                version: 0, // New items start at version 0
                value: value.clone(),
            }],
            delete_items: vec![],
        };
        
        match self.inner.put_object(&request).await {
            Ok(_response) => {
                // Put operation succeeded, need to get the item to return its version
                let get_request = GetObjectRequest {
                    store_id: self.store_id.clone(),
                    key: key.clone(),
                };
                match self.inner.get_object(&get_request).await {
                    Ok(get_response) => {
                        if let Some(kv) = get_response.value {
                            Ok(VssItem {
                                key: kv.key,
                                value: kv.value,
                                version: kv.version,
                            })
                        } else {
                            Err(VssError::StoreError { error_details: "Item not found after put".to_string() })
                        }
                    },
                    Err(e) => Err(convert_error(e, "store_get")),
                }
            },
            Err(e) => Err(convert_error(e, "store")),
        }
    }
    
    /// Retrieves a value by key.
    ///
    /// # Parameters
    /// - `key`: The key to retrieve
    ///
    /// # Returns
    /// Some(VssItem) if found, None if key doesn't exist
    pub async fn get(&self, key: String) -> Result<Option<VssItem>, VssError> {
        let request = GetObjectRequest {
            store_id: self.store_id.clone(),
            key: key.clone(),
        };
        
        match self.inner.get_object(&request).await {
            Ok(response) => {
                if let Some(kv) = response.value {
                    Ok(Some(VssItem {
                        key: kv.key,
                        value: kv.value,
                        version: kv.version,
                    }))
                } else {
                    Ok(None)
                }
            },
            Err(ExternalVssError::NoSuchKeyError(_)) => Ok(None),
            Err(e) => Err(convert_error(e, "get")),
        }
    }
    
    /// Lists all items, optionally filtered by key prefix.
    ///
    /// # Parameters
    /// - `prefix`: Optional key prefix filter
    ///
    /// # Returns
    /// Vector of all matching VssItems with their data
    pub async fn list(&self, prefix: Option<String>) -> Result<Vec<VssItem>, VssError> {
        let request = ListKeyVersionsRequest {
            store_id: self.store_id.clone(),
            key_prefix: prefix,
            page_size: None,
            page_token: None,
        };
        
        match self.inner.list_key_versions(&request).await {
            Ok(list_response) => {
                let mut items = Vec::new();
                
                for key_version in list_response.key_versions {
                    let get_request = GetObjectRequest {
                        store_id: self.store_id.clone(),
                        key: key_version.key.clone(),
                    };
                    if let Ok(item_response) = self.inner.get_object(&get_request).await {
                        if let Some(kv) = item_response.value {
                            items.push(VssItem {
                                key: kv.key,
                                value: kv.value,
                                version: kv.version,
                            });
                        }
                    }
                }
                
                Ok(items)
            }
            Err(e) => Err(convert_error(e, "list")),
        }
    }
    
    /// Lists keys and versions without retrieving values.
    ///
    /// # Parameters
    /// - `prefix`: Optional key prefix filter
    ///
    /// # Returns
    /// Vector of KeyVersion structs (more efficient than list())
    pub async fn list_keys(&self, prefix: Option<String>) -> Result<Vec<KeyVersion>, VssError> {
        let request = ListKeyVersionsRequest {
            store_id: self.store_id.clone(),
            key_prefix: prefix,
            page_size: None,
            page_token: None,
        };
        
        match self.inner.list_key_versions(&request).await {
            Ok(response) => Ok(response.key_versions.into_iter().map(|kv| KeyVersion {
                key: kv.key,
                version: kv.version,
            }).collect()),
            Err(e) => Err(convert_error(e, "list_keys")),
        }
    }
    
    /// Stores multiple key-value pairs in an atomic transaction.
    ///
    /// # Parameters
    /// - `items`: Vector of KeyValue pairs to store
    ///
    /// # Returns
    /// Vector of stored VssItems with assigned versions
    pub async fn put_with_key_prefix(&self, items: Vec<KeyValue>) -> Result<Vec<VssItem>, VssError> {
        let external_items: Vec<ExternalKeyValue> = items.into_iter()
            .map(|item| ExternalKeyValue {
                key: item.key,
                value: item.value,
                version: 0, // New items start at version 0
            })
            .collect();
        
        let keys_to_get: Vec<String> = external_items.iter().map(|item| item.key.clone()).collect();
        
        let request = PutObjectRequest {
            store_id: self.store_id.clone(),
            global_version: None,
            transaction_items: external_items,
            delete_items: vec![],
        };
        
        match self.inner.put_object(&request).await {
            Ok(_response) => {
                // Get all the items that were just stored
                let mut result_items = Vec::new();
                for key in keys_to_get {
                    let get_request = GetObjectRequest {
                        store_id: self.store_id.clone(),
                        key: key.clone(),
                    };
                    if let Ok(get_response) = self.inner.get_object(&get_request).await {
                        if let Some(kv) = get_response.value {
                            result_items.push(VssItem {
                                key: kv.key,
                                value: kv.value,
                                version: kv.version,
                            });
                        }
                    }
                }
                Ok(result_items)
            },
            Err(e) => Err(convert_error(e, "put_with_key_prefix")),
        }
    }
    
    /// Deletes a key-value pair.
    ///
    /// # Parameters
    /// - `key`: The key to delete
    ///
    /// # Returns
    /// true if deleted, false if key didn't exist
    pub async fn delete(&self, key: String) -> Result<bool, VssError> {
        // First get the current value to get the version
        let get_request = GetObjectRequest {
            store_id: self.store_id.clone(),
            key: key.clone(),
        };
        
        let current_value = match self.inner.get_object(&get_request).await {
            Ok(response) => response.value,
            Err(ExternalVssError::NoSuchKeyError(_)) => return Ok(false),
            Err(e) => return Err(convert_error(e, "delete")),
        };
        
        if let Some(kv) = current_value {
            let request = DeleteObjectRequest {
                store_id: self.store_id.clone(),
                key_value: Some(kv),
            };
            
            match self.inner.delete_object(&request).await {
                Ok(_) => Ok(true),
                Err(ExternalVssError::NoSuchKeyError(_)) => Ok(false),
                Err(e) => Err(convert_error(e, "delete")),
            }
        } else {
            Ok(false)
        }
    }
}

/// Converts external VSS errors to internal error types.
///
/// # Parameters
/// - `error`: The external VssError from the vss-client library
/// - `operation`: The operation that failed (for context)
///
/// # Returns
/// Internal VssError with appropriate error details
fn convert_error(error: ExternalVssError, _operation: &str) -> VssError {
    match error {
        ExternalVssError::NoSuchKeyError(msg) => VssError::GetError { error_details: format!("Not found: {}", msg) },
        ExternalVssError::InternalServerError(msg) => VssError::NetworkError { error_details: msg },
        ExternalVssError::InvalidRequestError(msg) => VssError::InvalidData { error_details: msg },
        ExternalVssError::InternalError(msg) => VssError::UnknownError { error_details: msg },
        ExternalVssError::ConflictError(msg) => VssError::StoreError { error_details: format!("Conflict: {}", msg) },
    }
}