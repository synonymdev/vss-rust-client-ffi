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
use vss_client_ng::client::VssClient as ExternalVssClient;
use vss_client_ng::error::VssError as ExternalVssError;
use vss_client_ng::headers::{FixedHeaders, LnurlAuthToJwtProvider, VssHeaderProvider};
use vss_client_ng::types::{
    DeleteObjectRequest, GetObjectRequest, KeyValue as ExternalKeyValue, ListKeyVersionsRequest,
    PutObjectRequest, Storable,
};
use vss_client_ng::util::key_obfuscator::KeyObfuscator;
use vss_client_ng::util::retry::{
    ExponentialBackoffRetryPolicy, FilteredRetryPolicy, JitteredRetryPolicy,
    MaxAttemptsRetryPolicy, MaxTotalDelayRetryPolicy, RetryPolicy,
};
use vss_client_ng::util::storable_builder::{EntropySource, StorableBuilder};
use bip39::Mnemonic;
use std::str::FromStr;

pub(crate) const VSS_HARDENED_CHILD_INDEX: u32 = 877;
pub(crate) const VSS_LNURL_AUTH_HARDENED_CHILD_INDEX: u32 = 138;
const VSS_STORE_ID_HARDENED_CHILD_INDEX: u32 = 118;
const VSS_STORE_ID_HASH_LENGTH: usize = 36;

/// Derives a deterministic VSS store ID from a mnemonic and optional passphrase.
///
/// # Parameters
/// - `prefix`: A prefix to include in the store ID
/// - `mnemonic`: BIP39 mnemonic phrase (12 or 24 words)
/// - `passphrase`: Optional BIP39 passphrase
///
/// # Returns
/// A store ID string or VssError on failure
pub fn derive_vss_store_id(
    prefix: String,
    mnemonic: String,
    passphrase: Option<String>,
) -> Result<String, VssError> {
    let mnemonic = Mnemonic::from_str(&mnemonic).map_err(|e| VssError::ConnectionError {
        error_details: format!("Invalid mnemonic: {}", e),
    })?;

    let seed = match passphrase {
        Some(passphrase) => mnemonic.to_seed(&passphrase),
        None => mnemonic.to_seed(""),
    };
    let seed_array: [u8; 32] = seed[..32]
        .try_into()
        .map_err(|_| VssError::ConnectionError {
            error_details: "Failed to extract seed from mnemonic".to_string(),
        })?;

    let secp = Secp256k1::new();
    let master_xprv = Xpriv::new_master(Network::Bitcoin, &seed_array).map_err(|e| {
        VssError::ConnectionError {
            error_details: format!("Failed to create master key: {}", e),
        }
    })?;

    let vss_store_id_xprv = master_xprv
        .derive_priv(
            &secp,
            &[
                ChildNumber::Hardened { index: VSS_HARDENED_CHILD_INDEX },
                ChildNumber::Hardened { index: VSS_STORE_ID_HARDENED_CHILD_INDEX },
            ],
        )
        .map_err(|e| VssError::ConnectionError {
            error_details: format!("Failed to derive VSS store ID key: {}", e),
        })?;

    let store_id_key = vss_store_id_xprv.private_key.secret_bytes();
    let hash = sha256::Hash::hash(&store_id_key);
    let hash_hex = hash.to_string();

    let store_id_suffix = &hash_hex[..VSS_STORE_ID_HASH_LENGTH];
    let store_id = format!("{}_{}", prefix, store_id_suffix);

    Ok(store_id)
}

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
    pub(crate) app_data_encryption_key: [u8; 32],
    pub(crate) app_key_obfuscator: Option<Arc<KeyObfuscator>>,
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
    /// Uses the truncated 32-byte seed for app backup keys (backward compatible)
    /// and the full 64-byte seed for LDK backup keys (matching ldk-node).
    ///
    /// # Parameters
    /// - `base_url`: The VSS server URL
    /// - `store_id`: The storage namespace identifier
    /// - `seed`: The full BIP39 seed bytes for key derivation (64 bytes)
    /// - `lnurl_auth_server_url`: The LNURL-auth server URL
    ///
    /// # Returns
    /// A new VssClient instance or VssError on failure
    pub async fn new_with_lnurl_auth(
        base_url: String,
        store_id: String,
        seed: [u8; 64],
        lnurl_auth_server_url: String,
    ) -> Result<Self, VssError> {
        let secp = Secp256k1::new();

        // Derive app keys from truncated 32-byte seed (backward compatible with v0.4.0)
        let truncated_seed: [u8; 32] = seed[..32].try_into().unwrap();
        let app_master_xprv =
            Xpriv::new_master(Network::Bitcoin, &truncated_seed).map_err(|e| {
                VssError::ConnectionError {
                    error_details: format!("Failed to create app master key: {}", e),
                }
            })?;
        let app_vss_xprv = app_master_xprv
            .derive_priv(
                &secp,
                &[ChildNumber::Hardened {
                    index: VSS_HARDENED_CHILD_INDEX,
                }],
            )
            .map_err(|e| VssError::ConnectionError {
                error_details: format!("Failed to derive app VSS key: {}", e),
            })?;
        let app_vss_seed_bytes: [u8; 32] = app_vss_xprv.private_key.secret_bytes();

        // LNURL auth from app path (matches v0.4.0 server identity)
        let lnurl_auth_xprv = app_vss_xprv
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

        Self::new_with_header_provider(
            base_url,
            store_id,
            header_provider,
            Some(app_vss_seed_bytes),
        )
        .await
    }

    /// Internal method to create a client with any header provider
    async fn new_with_header_provider(
        base_url: String,
        store_id: String,
        header_provider: Arc<dyn VssHeaderProvider>,
        app_vss_seed: Option<[u8; 32]>,
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

        let storable_builder = Arc::new(StorableBuilder::new(RandEntropySource));

        let (app_data_encryption_key, app_key_obfuscator) = if let Some(seed) = app_vss_seed {
            let (dek, obfuscation_master_key) =
                derive_data_encryption_and_obfuscation_keys(&seed);
            (dek, Some(Arc::new(KeyObfuscator::new(obfuscation_master_key))))
        } else {
            ([0u8; 32], None)
        };

        Ok(VssClient {
            inner: Arc::new(client),
            store_id,
            storable_builder,
            app_data_encryption_key,
            app_key_obfuscator,
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
        let storage_key = self.build_key(&key);
        let storable = self.storable_builder.build(
            value.clone(),
            version,
            &self.app_data_encryption_key,
            storage_key.as_bytes(),
        );

        let request = PutObjectRequest {
            store_id: self.store_id.clone(),
            global_version: None,
            transaction_items: vec![ExternalKeyValue {
                key: storage_key,
                version,
                value: storable.encode_to_vec(),
            }],
            delete_items: vec![],
        };

        match self.inner.put_object(&request).await {
            Ok(_) => Ok(VssItem {
                key,
                value,
                version: -1,
            }),
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
        let storage_key = self.build_key(&key);

        if let Some((value, version)) =
            self.try_get_raw(&storage_key, &self.app_data_encryption_key).await?
        {
            return Ok(Some(VssItem { key, value, version }));
        }

        Ok(None)
    }

    /// Lists all items, optionally filtered by key prefix.
    ///
    /// # Parameters
    /// - `prefix`: Optional key prefix filter
    ///
    /// # Returns
    /// Vector of all matching VssItems with their data
    pub async fn list(&self, prefix: Option<String>) -> Result<Vec<VssItem>, VssError> {
        let keys = self.list_keys(prefix).await?;
        let mut items = Vec::new();
        for kv in keys {
            if let Ok(Some(item)) = self.get(kv.key).await {
                items.push(item);
            }
        }
        Ok(items)
    }

    /// Lists keys and versions without retrieving values.
    ///
    /// # Parameters
    /// - `prefix`: Optional key prefix filter
    ///
    /// # Returns
    /// Vector of KeyVersion structs (more efficient than list())
    pub async fn list_keys(&self, prefix: Option<String>) -> Result<Vec<KeyVersion>, VssError> {
        let storage_prefix = prefix.as_ref().map(|p| self.build_key(p));
        self.list_key_versions(storage_prefix, &self.app_key_obfuscator).await
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
                let storage_key = self.build_key(&item.key);
                let storable = self.storable_builder.build(
                    item.value.clone(),
                    version,
                    &self.app_data_encryption_key,
                    storage_key.as_bytes(),
                );
                ExternalKeyValue {
                    key: storage_key,
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
            Ok(_) => Ok(items
                .into_iter()
                .map(|item| VssItem {
                    key: item.key,
                    value: item.value,
                    version: -1,
                })
                .collect()),
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
        let storage_key = self.build_key(&key);
        self.delete_by_storage_key(&storage_key).await
    }

    // --- Key obfuscation helpers ---

    /// Tries to deobfuscate a storage key using the given obfuscator.
    fn try_deobfuscate(obfuscator: &KeyObfuscator, storage_key: &str) -> Option<String> {
        if let Some((_prefix, obfuscated_key)) = storage_key.rsplit_once('#') {
            if let Ok(key) = obfuscator.deobfuscate(obfuscated_key) {
                return Some(key);
            }
        }
        obfuscator.deobfuscate(storage_key).ok()
    }

    /// Deobfuscates a storage key using the given obfuscator.
    fn deobfuscate_key(
        key_obfuscator: &Option<Arc<KeyObfuscator>>,
        storage_key: &str,
    ) -> Result<String, VssError> {
        if let Some(ref obfuscator) = key_obfuscator {
            Self::try_deobfuscate(obfuscator, storage_key)
                .ok_or_else(|| VssError::ListError {
                    error_details: "Failed to deobfuscate key".to_string(),
                })
        } else {
            Ok(storage_key.to_string())
        }
    }

    /// Converts a user key to storage key using the app obfuscator.
    pub(crate) fn build_key(&self, key: &str) -> String {
        if let Some(ref obfuscator) = self.app_key_obfuscator {
            obfuscator.obfuscate(key)
        } else {
            key.to_string()
        }
    }

    #[cfg(test)]
    pub(crate) fn extract_key(&self, storage_key: &str) -> Result<String, VssError> {
        Self::deobfuscate_key(&self.app_key_obfuscator, storage_key)
    }

    // --- Low-level helpers ---

    /// Gets and decrypts a value using explicit storage key and encryption key.
    async fn try_get_raw(
        &self,
        storage_key: &str,
        data_encryption_key: &[u8; 32],
    ) -> Result<Option<(Vec<u8>, i64)>, VssError> {
        let request = GetObjectRequest {
            store_id: self.store_id.clone(),
            key: storage_key.to_string(),
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
                        .deconstruct(storable, data_encryption_key, storage_key.as_bytes())
                        .map_err(|e| VssError::GetError {
                            error_details: format!("Failed to decrypt data: {}", e),
                        })?;
                    Ok(Some((decrypted_value, kv.version)))
                } else {
                    Ok(None)
                }
            }
            Err(ExternalVssError::NoSuchKeyError(_)) => Ok(None),
            Err(e) => Err(convert_error(e, "try_get_raw")),
        }
    }

    /// Deletes a value by raw storage key.
    async fn delete_by_storage_key(&self, storage_key: &str) -> Result<bool, VssError> {
        let request = DeleteObjectRequest {
            store_id: self.store_id.clone(),
            key_value: Some(ExternalKeyValue {
                key: storage_key.to_string(),
                version: -1,
                value: vec![],
            }),
        };
        match self.inner.delete_object(&request).await {
            Ok(_) => Ok(true),
            Err(ExternalVssError::NoSuchKeyError(_)) => Ok(false),
            Err(e) => Err(convert_error(e, "delete_by_storage_key")),
        }
    }

    /// Lists key versions for a single prefix using the given obfuscator.
    async fn list_key_versions(
        &self,
        prefix: Option<String>,
        key_obfuscator: &Option<Arc<KeyObfuscator>>,
    ) -> Result<Vec<KeyVersion>, VssError> {
        let results = self.list_all_key_versions_paginated(prefix).await?;
        let mut result = Vec::new();
        for kv in results {
            if let Ok(original_key) = Self::deobfuscate_key(key_obfuscator, &kv.key) {
                result.push(KeyVersion {
                    key: original_key,
                    version: kv.version,
                });
            }
        }
        Ok(result)
    }

    /// Fetches all key versions across pages using pagination.
    async fn list_all_key_versions_paginated(
        &self,
        prefix: Option<String>,
    ) -> Result<Vec<ExternalKeyValue>, VssError> {
        let mut all_results = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let request = ListKeyVersionsRequest {
                store_id: self.store_id.clone(),
                key_prefix: prefix.clone(),
                page_size: Some(100),
                page_token: page_token.clone(),
            };
            let response = self
                .inner
                .list_key_versions(&request)
                .await
                .map_err(|e| convert_error(e, "list_paginated"))?;

            all_results.extend(response.key_versions);

            match response.next_page_token {
                Some(ref token) if !token.is_empty() => page_token = Some(token.clone()),
                _ => break,
            }
        }

        Ok(all_results)
    }
}

/// Derives data encryption and obfuscation keys from VSS seed
pub(crate) fn derive_data_encryption_and_obfuscation_keys(vss_seed: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
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
pub(crate) fn convert_error(error: ExternalVssError, _operation: &str) -> VssError {
    match error {
        ExternalVssError::NoSuchKeyError(msg) => VssError::GetError { error_details: format!("Not found: {}", msg) },
        ExternalVssError::InternalServerError(msg) => VssError::NetworkError { error_details: msg },
        ExternalVssError::InvalidRequestError(msg) => VssError::InvalidData { error_details: msg },
        ExternalVssError::InternalError(msg) => VssError::UnknownError { error_details: msg },
        ExternalVssError::ConflictError(msg) => VssError::StoreError { error_details: format!("Conflict: {}", msg) },
        ExternalVssError::AuthError(msg) => VssError::AuthError { error_details: msg },
    }
}
