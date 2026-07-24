use super::errors::VssError;
use super::types::*;
use crate::implementation::{
    convert_error, derive_data_encryption_and_obfuscation_keys, RandEntropySource,
    VSS_HARDENED_CHILD_INDEX, VSS_LNURL_AUTH_HARDENED_CHILD_INDEX,
};
use bitcoin::bip32::{ChildNumber, Xpriv};
use bitcoin::secp256k1::Secp256k1;
use bitcoin::Network;
use prost::Message;
use std::collections::HashMap;
use std::sync::Arc;
use vss_client_ng::client::VssClient as ExternalVssClient;
use vss_client_ng::error::VssError as ExternalVssError;
use vss_client_ng::headers::LnurlAuthToJwtProvider;
use vss_client_ng::types::{
    DeleteObjectRequest, GetObjectRequest, KeyValue as ExternalKeyValue, ListKeyVersionsRequest,
    PutObjectRequest, Storable,
};
use vss_client_ng::util::key_obfuscator::KeyObfuscator;
use vss_client_ng::util::retry::{
    ExponentialBackoffRetryPolicy, FilteredRetryPolicy, JitteredRetryPolicy,
    MaxAttemptsRetryPolicy, MaxTotalDelayRetryPolicy, RetryPolicy,
};
use vss_client_ng::util::storable_builder::StorableBuilder;

type LdkRetryPolicy = FilteredRetryPolicy<
    JitteredRetryPolicy<
        MaxTotalDelayRetryPolicy<
            MaxAttemptsRetryPolicy<ExponentialBackoffRetryPolicy<ExternalVssError>>,
        >,
    >,
    Box<dyn Fn(&ExternalVssError) -> bool + 'static + Send + Sync>,
>;

/// A dedicated VSS client for LDK (Lightning Development Kit) operations.
///
/// Uses ONLY ldk-node's key derivation chain for obfuscation and encryption,
/// completely separate from the app backup client. This ensures correct
/// deobfuscation of keys stored by ldk-node's VssStore.
///
/// Key derivation (matching ldk-node exactly):
/// 1. Full 64-byte BIP39 seed -> Xpriv::new_master(Bitcoin, &seed)
/// 2. derive_priv(m/877') -> vss_xprv
/// 3. vss_xprv.private_key.secret_bytes() -> 32-byte vss_seed
/// 4. HKDF(vss_seed) -> (data_encryption_key, obfuscation_master_key)
/// 5. KeyObfuscator::new(obfuscation_master_key)
///
/// LNURL-auth: derived from truncated 32-byte seed path (m/877'/138')
/// to maintain the same server identity as the app client.
#[derive(Clone)]
pub struct LdkVssClient {
    inner: Arc<ExternalVssClient<LdkRetryPolicy>>,
    store_id: String,
    storable_builder: Arc<StorableBuilder<RandEntropySource>>,
    data_encryption_key: [u8; 32],
    key_obfuscator: Arc<KeyObfuscator>,
}

impl LdkVssClient {
    /// Creates a new dedicated LDK VSS client with LNURL-auth.
    pub async fn new_with_lnurl_auth(
        base_url: String,
        store_id: String,
        seed: [u8; 64],
        lnurl_auth_server_url: String,
    ) -> Result<Self, VssError> {
        let secp = Secp256k1::new();

        // LNURL-auth: from full 64-byte seed (same server identity as ldk-node)
        let auth_master_xprv =
            Xpriv::new_master(Network::Bitcoin, &seed).map_err(|e| VssError::ConnectionError {
                error_details: format!("Failed to create auth master key: {}", e),
            })?;
        let auth_vss_xprv = auth_master_xprv
            .derive_priv(
                &secp,
                &[ChildNumber::Hardened {
                    index: VSS_HARDENED_CHILD_INDEX,
                }],
            )
            .map_err(|e| VssError::ConnectionError {
                error_details: format!("Failed to derive auth VSS key: {}", e),
            })?;
        let lnurl_auth_xprv = auth_vss_xprv
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

        // LDK keys: from full 64-byte seed (matching ldk-node exactly)
        let ldk_master_xprv =
            Xpriv::new_master(Network::Bitcoin, &seed).map_err(|e| VssError::ConnectionError {
                error_details: format!("Failed to create LDK master key: {}", e),
            })?;
        let ldk_vss_xprv = ldk_master_xprv
            .derive_priv(
                &secp,
                &[ChildNumber::Hardened {
                    index: VSS_HARDENED_CHILD_INDEX,
                }],
            )
            .map_err(|e| VssError::ConnectionError {
                error_details: format!("Failed to derive LDK VSS key: {}", e),
            })?;
        let vss_seed: [u8; 32] = ldk_vss_xprv.private_key.secret_bytes();

        let (data_encryption_key, obfuscation_master_key) =
            derive_data_encryption_and_obfuscation_keys(&vss_seed);
        let key_obfuscator = Arc::new(KeyObfuscator::new(obfuscation_master_key));

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

        Ok(LdkVssClient {
            inner: Arc::new(client),
            store_id,
            storable_builder: Arc::new(StorableBuilder::new(RandEntropySource)),
            data_encryption_key,
            key_obfuscator,
        })
    }

    /// Retrieves a value by key using ldk-node's namespaced key format.
    pub async fn get(
        &self,
        key: String,
        namespace: &LdkNamespace,
    ) -> Result<Option<VssItem>, VssError> {
        let storage_key = self.build_key(&key, namespace);

        let request = GetObjectRequest {
            store_id: self.store_id.clone(),
            key: storage_key.clone(),
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
                        .deconstruct(storable, &self.data_encryption_key, storage_key.as_bytes())
                        .map_err(|e| VssError::GetError {
                            error_details: format!("Failed to decrypt data: {}", e),
                        })?;
                    Ok(Some(VssItem {
                        key,
                        value: decrypted_value,
                        version: kv.version,
                    }))
                } else {
                    Ok(None)
                }
            }
            Err(ExternalVssError::NoSuchKeyError(_)) => Ok(None),
            Err(e) => Err(convert_error(e, "ldk_get")),
        }
    }

    /// Stores a key-value pair using ldk-node's namespaced key format.
    pub async fn store(
        &self,
        key: String,
        value: Vec<u8>,
        namespace: &LdkNamespace,
    ) -> Result<VssItem, VssError> {
        let version = -1;
        let storage_key = self.build_key(&key, namespace);
        let storable = self.storable_builder.build(
            value.clone(),
            version,
            &self.data_encryption_key,
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
            Err(e) => Err(convert_error(e, "ldk_store")),
        }
    }

    /// Deletes a key-value pair using ldk-node's namespaced key format.
    pub async fn delete(&self, key: String, namespace: &LdkNamespace) -> Result<bool, VssError> {
        let storage_key = self.build_key(&key, namespace);
        let request = DeleteObjectRequest {
            store_id: self.store_id.clone(),
            key_value: Some(ExternalKeyValue {
                key: storage_key,
                version: -1,
                value: vec![],
            }),
        };
        match self.inner.delete_object(&request).await {
            Ok(_) => Ok(true),
            Err(ExternalVssError::NoSuchKeyError(_)) => Ok(false),
            Err(e) => Err(convert_error(e, "ldk_delete")),
        }
    }

    /// Lists keys in a specific namespace with client-side deobfuscation.
    pub async fn list_keys(&self, namespace: &LdkNamespace) -> Result<Vec<KeyVersion>, VssError> {
        let prefix = Some(self.build_prefix(namespace));
        self.list_key_versions_deobfuscated(prefix).await
    }

    /// Lists all LDK keys across all namespaces.
    /// Uses no server-side prefix filter, relying on client-side deobfuscation.
    pub async fn list_all_keys(&self) -> Result<Vec<KeyVersion>, VssError> {
        self.list_key_versions_deobfuscated(None).await
    }

    // --- Internal helpers ---

    /// Builds an obfuscated storage key matching ldk-node's V1 format.
    pub(crate) fn build_key(&self, key: &str, namespace: &LdkNamespace) -> String {
        let primary = namespace.primary();
        let secondary = namespace.secondary();
        let prefix = format!("{}#{}", primary, secondary);
        let obfuscated_prefix = self.key_obfuscator.obfuscate(&prefix);
        let obfuscated_key = self.key_obfuscator.obfuscate(key);
        format!("{}#{}", obfuscated_prefix, obfuscated_key)
    }

    /// Builds the obfuscated namespace prefix for server-side filtering.
    fn build_prefix(&self, namespace: &LdkNamespace) -> String {
        let primary = namespace.primary();
        let secondary = namespace.secondary();
        let prefix = format!("{}#{}", primary, secondary);
        self.key_obfuscator.obfuscate(&prefix)
    }

    /// Tries to deobfuscate a storage key (handles both V0 and V1 formats).
    fn try_deobfuscate(&self, storage_key: &str) -> Option<String> {
        // V0/V1 with namespace: prefix#obfuscated_key -> split on last '#'
        if let Some((_prefix, obfuscated_key)) = storage_key.rsplit_once('#') {
            if let Ok(key) = self.key_obfuscator.deobfuscate(obfuscated_key) {
                return Some(key);
            }
        }
        // V0/V1 default namespace: just obfuscated_key
        self.key_obfuscator.deobfuscate(storage_key).ok()
    }

    /// Lists key versions with client-side deobfuscation filtering.
    async fn list_key_versions_deobfuscated(
        &self,
        prefix: Option<String>,
    ) -> Result<Vec<KeyVersion>, VssError> {
        let results = self.list_all_key_versions_paginated(prefix).await?;
        let mut result = Vec::new();
        for kv in results {
            if let Some(original_key) = self.try_deobfuscate(&kv.key) {
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
                .map_err(|e| convert_error(e, "ldk_list_paginated"))?;

            all_results.extend(response.key_versions);

            match response.next_page_token {
                Some(ref token) if !token.is_empty() => page_token = Some(token.clone()),
                _ => break,
            }
        }

        Ok(all_results)
    }
}
