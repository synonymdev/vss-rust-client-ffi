mod errors;
#[cfg(test)]
mod ffi_tests;
mod implementation;
mod tests;
mod types;

pub use errors::*;
pub use implementation::{VssClient, derive_vss_store_id};
pub use types::*;

uniffi::setup_scaffolding!();

use bip39::Mnemonic;
use once_cell::sync::OnceCell;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;

static RUNTIME: OnceCell<Runtime> = OnceCell::new();
static VSS_CLIENT: OnceCell<Arc<Mutex<Option<VssClient>>>> = OnceCell::new();

// Helper macro to handle async execution in both test and production environments
macro_rules! execute_async {
    ($async_block:expr) => {{
        if tokio::runtime::Handle::try_current().is_ok() {
            // We're already in an async context (e.g., during tests)
            $async_block.await
        } else {
            // Normal case - use our runtime
            let rt = ensure_runtime();
            rt.block_on($async_block)
        }
    }};
}

fn ensure_runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime")
    })
}

fn get_vss_client() -> &'static Arc<Mutex<Option<VssClient>>> {
    VSS_CLIENT.get_or_init(|| Arc::new(Mutex::new(None)))
}

fn try_get_client() -> Result<VssClient, VssError> {
    let storage = get_vss_client();
    let guard = storage.lock().unwrap();
    guard
        .as_ref()
        .ok_or(VssError::ConnectionError {
            error_details: "VSS client not initialized. Call vss_new_client() first.".to_string(),
        })
        .map(|c| c.clone())
}

/// Creates a new VSS (Versioned Storage Service) client without authentication.
///
/// This function establishes a connection to a VSS server and initializes
/// the global client for subsequent VSS operations.
///
/// # Parameters
/// - `base_url`: The base URL of the VSS server (e.g., "https://vss.example.com")
/// - `store_id`: A unique identifier for the storage namespace/keyspace
///
/// # Returns
/// Ok(()) if the client was created successfully, or a VssError if the client creation fails.
///
/// # Example
/// ```
/// vss_new_client(
///     "https://vss.example.com".to_string(),
///     "my-app-store".to_string()
/// ).await?;
/// ```
#[uniffi::export]
pub async fn vss_new_client(base_url: String, store_id: String) -> Result<(), VssError> {
    execute_async!(async move {
        let client = VssClient::new(base_url, store_id).await?;

        let storage = get_vss_client();
        let mut guard = storage.lock().unwrap();
        *guard = Some(client);
        drop(guard);

        Ok(())
    })
}

/// Creates a new VSS (Versioned Storage Service) client with LNURL-auth using a BIP39 mnemonic.
///
/// This function establishes a connection to a VSS server using LNURL-auth
/// for authentication.
///
/// # Parameters
/// - `base_url`: The base URL of the VSS server
/// - `store_id`: A unique identifier for the storage namespace/keyspace
/// - `mnemonic`: BIP39 mnemonic phrase (12 or 24 words)
/// - `passphrase`: Optional BIP39 passphrase
/// - `lnurl_auth_server_url`: The LNURL-auth server URL for authentication
///
/// # Returns
/// Ok(()) if the client was created successfully, or a VssError if the client creation fails.
///
/// # Example
/// ```
/// vss_new_client_with_lnurl_auth(
///     "https://vss.example.com".to_string(),
///     "my-app-store".to_string(),
///     "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string(),
///     None,
///     "https://auth.example.com/lnurl".to_string()
/// ).await?;
/// ```
#[uniffi::export]
pub async fn vss_new_client_with_lnurl_auth(
    base_url: String,
    store_id: String,
    mnemonic: String,
    passphrase: Option<String>,
    lnurl_auth_server_url: String,
) -> Result<(), VssError> {
    execute_async!(async move {
        let mnemonic = Mnemonic::from_str(&mnemonic).map_err(|e| VssError::ConnectionError {
            error_details: format!("Invalid mnemonic: {}", e),
        })?;

        let seed = match passphrase {
            Some(passphrase) => mnemonic.to_seed(&passphrase),
            None => mnemonic.to_seed(""),
        };
        let seed_array: [u8; 32] =
            seed[..32]
                .try_into()
                .map_err(|_| VssError::ConnectionError {
                    error_details: "Failed to extract seed from mnemonic".to_string(),
                })?;

        let client =
            VssClient::new_with_lnurl_auth(base_url, store_id, seed_array, lnurl_auth_server_url)
                .await?;

        let storage = get_vss_client();
        let mut guard = storage.lock().unwrap();
        *guard = Some(client);
        drop(guard);

        Ok(())
    })
}

/// Stores a key-value pair in the VSS server.
///
/// This function writes data to the VSS server. The server automatically
/// manages versioning, incrementing the version number with each update.
///
/// # Parameters
/// - `key`: The unique key identifier for the data
/// - `value`: The binary data to store
///
/// # Returns
/// A VssItem containing the stored key, value, and version number,
/// or a VssError if the operation fails.
///
/// # Example
/// ```
/// let item = vss_store(
///     "user-settings".to_string(),
///     vec![1, 2, 3, 4]
/// ).await?;
/// println!("Stored at version: {}", item.version);
/// ```
#[uniffi::export]
pub async fn vss_store(
    key: String,
    value: Vec<u8>
) -> Result<VssItem, VssError> {
    execute_async!(async move {
        let client = try_get_client()?;
        client.store(key, value).await
    })
}

/// Retrieves a value by key from the VSS server.
///
/// This function fetches the current version of the data associated with the given key.
/// Returns None if the key does not exist.
///
/// # Parameters
/// - `key`: The key to retrieve
///
/// # Returns
/// An Option containing the VssItem if found, None if the key doesn't exist,
/// or a VssError if the operation fails.
///
/// # Example
/// ```
/// match vss_get("user-settings".to_string()).await? {
///     Some(item) => println!("Found data with version: {}", item.version),
///     None => println!("Key not found")
/// }
/// ```
#[uniffi::export]
pub async fn vss_get(
    key: String
) -> Result<Option<VssItem>, VssError> {
    execute_async!(async move {
        let client = try_get_client()?;
        client.get(key).await
    })
}

/// Lists all items in the store, optionally filtered by key prefix.
///
/// This function retrieves both keys and their associated values/versions.
/// It's useful for browsing stored data but can be expensive for large datasets.
///
/// # Parameters
/// - `prefix`: Optional key prefix filter (e.g., "user/" to get all user keys)
///             If None or empty, returns all items
///
/// # Returns
/// A vector of VssItems containing all matching key-value pairs,
/// or a VssError if the operation fails.
///
/// # Example
/// ```
/// // List all items with keys starting with "config/"
/// let items = vss_list(Some("config/".to_string())).await?;
/// for item in items {
///     println!("Key: {}, Version: {}", item.key, item.version);
/// }
/// ```
#[uniffi::export]
pub async fn vss_list(
    prefix: Option<String>
) -> Result<Vec<VssItem>, VssError> {
    execute_async!(async move {
        let client = try_get_client()?;
        client.list(prefix).await
    })
}

/// Lists keys and their versions without retrieving the actual values.
///
/// This function is more efficient than `vss_list` when you only need to know
/// what keys exist and their versions, without downloading the actual data.
///
/// # Parameters
/// - `prefix`: Optional key prefix filter (e.g., "user/" to get all user keys)
///             If None or empty, returns all keys
///
/// # Returns
/// A vector of KeyVersion structs containing key names and version numbers,
/// or a VssError if the operation fails.
///
/// # Example
/// ```
/// // List all keys starting with "temp/"
/// let keys = vss_list_keys(Some("temp/".to_string())).await?;
/// for kv in keys {
///     println!("Key: {} is at version: {}", kv.key, kv.version);
/// }
/// ```
#[uniffi::export]
pub async fn vss_list_keys(
    prefix: Option<String>
) -> Result<Vec<KeyVersion>, VssError> {
    execute_async!(async move {
        let client = try_get_client()?;
        client.list_keys(prefix).await
    })
}

/// Stores multiple key-value pairs in a single atomic transaction.
///
/// This function allows batch storage of multiple items. All items will be
/// stored together or the entire operation will fail, ensuring data consistency.
///
/// # Parameters
/// - `items`: A vector of KeyValue pairs to store
///
/// # Returns
/// A vector of VssItems representing the stored data with their assigned versions,
/// or a VssError if the operation fails.
///
/// # Example
/// ```
/// let items_to_store = vec![
///     KeyValue { key: "config/theme".to_string(), value: vec![1, 0] },
///     KeyValue { key: "config/lang".to_string(), value: vec![2, 0] },
/// ];
/// let stored_items = vss_put_with_key_prefix(items_to_store).await?;
/// println!("Stored {} items", stored_items.len());
/// ```
#[uniffi::export]
pub async fn vss_put_with_key_prefix(
    items: Vec<KeyValue>
) -> Result<Vec<VssItem>, VssError> {
    execute_async!(async move {
        let client = try_get_client()?;
        client.put_with_key_prefix(items).await
    })
}

/// Deletes a key-value pair from the VSS server.
///
/// This function removes the specified key and its associated data from storage.
/// The operation is idempotent - deleting a non-existent key will not cause an error.
///
/// # Parameters
/// - `key`: The key to delete
///
/// # Returns
/// `true` if the key was found and deleted, `false` if the key didn't exist,
/// or a VssError if the operation fails.
///
/// # Example
/// ```
/// let was_deleted = vss_delete("temp-data".to_string()).await?;
/// if was_deleted {
///     println!("Key was successfully deleted");
/// } else {
///     println!("Key did not exist");
/// }
/// ```
#[uniffi::export]
pub async fn vss_delete(
    key: String
) -> Result<bool, VssError> {
    execute_async!(async move {
        let client = try_get_client()?;
        client.delete(key).await
    })
}

/// Derives a deterministic VSS store ID from a mnemonic and optional passphrase.
///
/// This function creates a consistent store ID that can be used across devices for the same wallet.
/// The store ID is derived using BIP32 key derivation at a specific VSS path, ensuring it's
/// cryptographically secure and deterministic.
///
/// # Parameters
/// - `prefix`: A prefix to include in the store ID (e.g., "bitkit_v1_regtest")
/// - `mnemonic`: BIP39 mnemonic phrase (12 or 24 words)
/// - `passphrase`: Optional BIP39 passphrase
///
/// # Returns
/// A deterministic store ID string that combines the prefix with a derived identifier.
///
/// # Example
/// ```
/// let store_id = vss_derive_store_id(
///     "bitkit_v1_regtest".to_string(),
///     "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_string(),
///     None
/// )?;
/// println!("Store ID: {}", store_id);
/// ```
#[uniffi::export]
pub fn vss_derive_store_id(
    prefix: String,
    mnemonic: String,
    passphrase: Option<String>,
) -> Result<String, VssError> {
    derive_vss_store_id(prefix, mnemonic, passphrase)
}

/// Shuts down the VSS client and clears the global client state.
///
/// This function is optional but recommended for clean shutdown in applications
/// that want to explicitly release resources.
///
/// # Example
/// ```
/// vss_shutdown_client();
/// ```
#[uniffi::export]
pub fn vss_shutdown_client() {
    if let Some(client_storage) = VSS_CLIENT.get() {
        let mut guard = client_storage.lock().unwrap();
        *guard = None;
    }
}
