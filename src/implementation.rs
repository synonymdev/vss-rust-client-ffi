use super::errors::VssError;
use super::types::*;
use bitcoin::bip32::{ChildNumber, Xpriv};
use bitcoin::hashes::{sha256, Hash, HashEngine, Hmac, HmacEngine};
use bitcoin::secp256k1::Secp256k1;
use bitcoin::Network;
use prost::Message;
use rand::RngCore;
use std::collections::HashMap;
use std::sync::Arc;
use vss_client::client::VssClient as ExternalVssClient;
use vss_client::error::VssError as ExternalVssError;
use vss_client::headers::{FixedHeaders, LnurlAuthToJwtProvider, VssHeaderProvider};
use vss_client::types::{
    DeleteObjectRequest, GetObjectRequest, KeyValue as ExternalKeyValue, ListKeyVersionsRequest,
    PutObjectRequest, Storable,
};
use vss_client::util::key_obfuscator::KeyObfuscator;
use vss_client::util::retry::{
    ExponentialBackoffRetryPolicy, FilteredRetryPolicy, JitteredRetryPolicy,
    MaxAttemptsRetryPolicy, MaxTotalDelayRetryPolicy, RetryPolicy,
};
use vss_client::util::storable_builder::{EntropySource, StorableBuilder};

const VSS_HARDENED_CHILD_INDEX: u32 = 877;
const VSS_LNURL_AUTH_HARDENED_CHILD_INDEX: u32 = 138;

type CustomRetryPolicy = FilteredRetryPolicy<
    JitteredRetryPolicy<
        MaxTotalDelayRetryPolicy<
            MaxAttemptsRetryPolicy<ExponentialBackoffRetryPolicy<ExternalVssError>>,
        >,
    >,
    Box<dyn Fn(&ExternalVssError) -> bool + 'static + Send + Sync>,
>;

/// A source for generating entropy/randomness using [`rand`].
pub(crate) struct RandEntropySource;

impl EntropySource for RandEntropySource {
    fn fill_bytes(&self, buffer: &mut [u8]) {
        rand::thread_rng().fill_bytes(buffer);
    }
}

#[derive(Clone)]
pub struct VssClient {
    inner: Arc<ExternalVssClient<CustomRetryPolicy>>,
    store_id: String,
    storable_builder: Arc<StorableBuilder<RandEntropySource>>,
    key_obfuscator: Option<Arc<KeyObfuscator>>,
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
        let header_provider = Arc::new(FixedHeaders::new(HashMap::new()));

        Self::new_with_header_provider(base_url, store_id, header_provider, None).await
    }

    /// Creates a new VSS client instance with LNURL-auth.
    ///
    /// # Parameters
    /// - `base_url`: The VSS server URL
    /// - `store_id`: The storage namespace identifier
    /// - `seed`: The seed bytes for key derivation (32 bytes)
    /// - `lnurl_auth_server_url`: The LNURL-auth server URL
    ///
    /// # Returns
    /// A new VssClient instance or VssError on failure
    pub async fn new_with_lnurl_auth(
        base_url: String,
        store_id: String,
        seed: [u8; 32],
        lnurl_auth_server_url: String,
    ) -> Result<Self, VssError> {
        let secp = Secp256k1::new();
        let master_xprv =
            Xpriv::new_master(Network::Bitcoin, &seed).map_err(|e| VssError::ConnectionError {
                error_details: format!("Failed to create master key: {}", e),
            })?;

        let vss_xprv = master_xprv
            .derive_priv(
                &secp,
                &[ChildNumber::Hardened {
                    index: VSS_HARDENED_CHILD_INDEX,
                }],
            )
            .map_err(|e| VssError::ConnectionError {
                error_details: format!("Failed to derive VSS key: {}", e),
            })?;

        let lnurl_auth_xprv = vss_xprv
            .derive_priv(
                &secp,
                &[ChildNumber::Hardened {
                    index: VSS_LNURL_AUTH_HARDENED_CHILD_INDEX,
                }],
            )
            .map_err(|e| VssError::ConnectionError {
                error_details: format!("Failed to derive LNURL-auth key: {}", e),
            })?;

        let lnurl_auth_jwt_provider =
            LnurlAuthToJwtProvider::new(lnurl_auth_xprv, lnurl_auth_server_url, HashMap::new())
                .map_err(|e| VssError::ConnectionError {
                    error_details: format!("Failed to create LNURL-auth provider: {}", e),
                })?;

        let header_provider = Arc::new(lnurl_auth_jwt_provider);

        let vss_seed_bytes: [u8; 32] = vss_xprv.private_key.secret_bytes();

        Self::new_with_header_provider(base_url, store_id, header_provider, Some(vss_seed_bytes))
            .await
    }

    /// Internal method to create a client with any header provider
    async fn new_with_header_provider(
        base_url: String,
        store_id: String,
        header_provider: Arc<dyn VssHeaderProvider>,
        vss_seed: Option<[u8; 32]>,
    ) -> Result<Self, VssError> {
        let retry_policy = ExponentialBackoffRetryPolicy::new(std::time::Duration::from_millis(10))
            .with_max_attempts(10)
            .with_max_total_delay(std::time::Duration::from_secs(15))
            .with_max_jitter(std::time::Duration::from_millis(10))
            .skip_retry_on_error(Box::new(|e: &ExternalVssError| {
                matches!(
                    e,
                    ExternalVssError::NoSuchKeyError(..)
                        | ExternalVssError::InvalidRequestError(..)
                        | ExternalVssError::ConflictError(..)
                )
            }) as _);

        let client = ExternalVssClient::new_with_headers(base_url, retry_policy, header_provider);

        let (storable_builder, key_obfuscator) = if let Some(seed) = vss_seed {
            let (data_encryption_key, obfuscation_master_key) =
                derive_data_encryption_and_obfuscation_keys(&seed);
            let builder = Arc::new(StorableBuilder::new(data_encryption_key, RandEntropySource));
            let obfuscator = Some(Arc::new(KeyObfuscator::new(obfuscation_master_key)));
            (builder, obfuscator)
        } else {
            let zero_key = [0u8; 32];
            let builder = Arc::new(StorableBuilder::new(zero_key, RandEntropySource));
            (builder, None)
        };

        Ok(VssClient {
            inner: Arc::new(client),
            store_id,
            storable_builder,
            key_obfuscator,
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
        let version = -1;
        let storable = self.storable_builder.build(value.clone(), version);
        let encrypted_value = storable.encode_to_vec();

        let request = PutObjectRequest {
            store_id: self.store_id.clone(),
            global_version: None,
            transaction_items: vec![ExternalKeyValue {
                key: self.build_key(&key),
                version,
                value: encrypted_value,
            }],
            delete_items: vec![],
        };

        match self.inner.put_object(&request).await {
            Ok(_response) => {
                Ok(VssItem {
                    key: key.clone(),
                    value,
                    version: -1,
                })
            }
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
            key: self.build_key(&key),
        };

        match self.inner.get_object(&request).await {
            Ok(response) => {
                if let Some(kv) = response.value {
                    let storable =
                        Storable::decode(&kv.value[..]).map_err(|e| VssError::GetError {
                            error_details: format!("Failed to decode storable: {}", e),
                        })?;

                    let (decrypted_value, _) = self
                        .storable_builder
                        .deconstruct(storable)
                        .map_err(|e| VssError::GetError {
                            error_details: format!("Failed to decrypt data: {}", e),
                        })?;

                    Ok(Some(VssItem {
                        key: key.clone(),
                        value: decrypted_value,
                        version: kv.version,
                    }))
                } else {
                    Ok(None)
                }
            }
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
            key_prefix: prefix.as_ref().map(|p| self.build_key(p)),
            page_size: None,
            page_token: None,
        };

        match self.inner.list_key_versions(&request).await {
            Ok(list_response) => {
                let mut items = Vec::new();

                for key_version in list_response.key_versions {
                    let original_key = self.extract_key(&key_version.key)?;

                    if let Ok(Some(item)) = self.get(original_key).await {
                        items.push(item);
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
            key_prefix: prefix.as_ref().map(|p| self.build_key(p)),
            page_size: None,
            page_token: None,
        };

        match self.inner.list_key_versions(&request).await {
            Ok(response) => {
                let mut result = Vec::new();
                for kv in response.key_versions {
                    let original_key = self.extract_key(&kv.key)?;

                    result.push(KeyVersion {
                        key: original_key,
                        version: kv.version,
                    });
                }
                Ok(result)
            }
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
    pub async fn put_with_key_prefix(
        &self,
        items: Vec<KeyValue>,
    ) -> Result<Vec<VssItem>, VssError> {
        let version = -1;
        let external_items: Vec<ExternalKeyValue> = items
            .iter()
            .map(|item| {
                let storable = self.storable_builder.build(item.value.clone(), version);
                ExternalKeyValue {
                    key: self.build_key(&item.key),
                    value: storable.encode_to_vec(),
                    version,
                }
            })
            .collect();

        let request = PutObjectRequest {
            store_id: self.store_id.clone(),
            global_version: None,
            transaction_items: external_items,
            delete_items: vec![],
        };

        match self.inner.put_object(&request).await {
            Ok(_response) => {
                Ok(items
                    .into_iter()
                    .map(|item| VssItem {
                        key: item.key,
                        value: item.value,
                        version: -1,
                    })
                    .collect())
            }
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
        let request = DeleteObjectRequest {
            store_id: self.store_id.clone(),
            key_value: Some(ExternalKeyValue {
                key: self.build_key(&key),
                version: -1,
                value: vec![],
            }),
        };

        match self.inner.delete_object(&request).await {
            Ok(_) => Ok(true),
            Err(ExternalVssError::NoSuchKeyError(_)) => Ok(false),
            Err(e) => Err(convert_error(e, "delete")),
        }
    }

    /// Converts a user key to storage key (obfuscated if encryption is enabled)
    fn build_key(&self, key: &str) -> String {
        if let Some(ref obfuscator) = self.key_obfuscator {
            obfuscator.obfuscate(key)
        } else {
            key.to_string()
        }
    }

    /// Converts a storage key back to user key (deobfuscated if encryption is enabled)
    fn extract_key(&self, storage_key: &str) -> Result<String, VssError> {
        if let Some(ref obfuscator) = self.key_obfuscator {
            obfuscator.deobfuscate(storage_key).map_err(|e| VssError::ListError {
                error_details: format!("Failed to deobfuscate key: {}", e),
            })
        } else {
            Ok(storage_key.to_string())
        }
    }
}

/// Derives data encryption and obfuscation keys from VSS seed
fn derive_data_encryption_and_obfuscation_keys(vss_seed: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let hkdf = |initial_key_material: &[u8], salt: &[u8]| -> [u8; 32] {
        let mut engine = HmacEngine::<sha256::Hash>::new(salt);
        engine.input(initial_key_material);
        Hmac::from_engine(engine).to_byte_array()
    };

    let prk = hkdf(vss_seed, b"pseudo_random_key");
    let k1 = hkdf(&prk, b"data_encryption_key");
    let k2 = hkdf(&prk, &[&k1[..], b"obfuscation_key"].concat());
    (k1, k2)
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
        ExternalVssError::AuthError(msg) => VssError::AuthError { error_details: msg },
    }
}
