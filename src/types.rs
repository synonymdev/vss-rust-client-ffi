use serde::{Deserialize, Serialize};

/// A decrypted backup object and the SHA-256 of its exact stored encrypted envelope.
/// This is a local read observation, not a signed server receipt or migration
/// authorization. Store IDs alone do not identify an authenticated principal.
/// Callers must retain their original wallet/endpoint identity and revalidate it
/// before using the observation. Do not log this record or its plaintext value.
#[derive(Clone)]
pub struct VssReadReceipt {
    pub key: String,
    pub value: Vec<u8>,
    pub store_id: String,
    pub storage_key: String,
    /// Lowercase, 64-character hexadecimal SHA-256 of exact stored envelope bytes.
    pub envelope_sha256: String,
    /// Actual outer server version, not the legacy envelope's embedded -1 sentinel.
    pub version: i64,
}

#[derive(Debug, Clone, uniffi::Record, Serialize, Deserialize)]
pub struct VssItem {
    pub key: String,
    pub value: Vec<u8>,
    pub version: i64,
}

#[derive(Debug, Clone, uniffi::Record, Serialize, Deserialize)]
pub struct KeyValue {
    pub key: String,
    pub value: Vec<u8>,
}

/// An encrypted write conditioned on the server's current key version.
/// Use zero to create a key that must not exist. Existing keys require their
/// positive version from `vss_get`; unconditional writes are not accepted.
#[derive(Debug, Clone, uniffi::Record, Serialize, Deserialize)]
pub struct ConditionalKeyValue {
    pub key: String,
    pub value: Vec<u8>,
    pub expected_version: i64,
}

#[derive(Debug, Clone, uniffi::Record, Serialize, Deserialize)]
pub struct ListKeyVersionsResponse {
    pub key_versions: Vec<KeyVersion>,
}

#[derive(Debug, Clone, uniffi::Record, Serialize, Deserialize)]
pub struct KeyVersion {
    pub key: String,
    pub version: i64,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum VssFilterType {
    Prefix,
    Exact,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum LdkNamespace {
    Default,
    Monitors,
    MonitorUpdates { monitor_id: String },
    ArchivedMonitors,
}

impl LdkNamespace {
    pub fn primary(&self) -> &str {
        match self {
            LdkNamespace::Default => "",
            LdkNamespace::Monitors => "monitors",
            LdkNamespace::MonitorUpdates { .. } => "monitor_updates",
            LdkNamespace::ArchivedMonitors => "archived_monitors",
        }
    }

    pub fn secondary(&self) -> &str {
        match self {
            LdkNamespace::MonitorUpdates { monitor_id } => monitor_id,
            _ => "",
        }
    }
}
