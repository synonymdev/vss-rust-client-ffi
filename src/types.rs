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
