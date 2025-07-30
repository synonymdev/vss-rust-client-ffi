use serde::{Deserialize, Serialize};

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