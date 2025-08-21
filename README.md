# VSS Rust Client FFI

Cross-platform FFI bindings for the [VSS (Versioned Storage Service) Rust Client](https://github.com/lightningdevkit/vss-server), providing a simple interface for mobile applications to interact with VSS servers.

## Installation

### Prerequisites

- Rust 1.70+
- `uniffi-bindgen` for generating bindings

### Building

```bash
# Basic build
cargo build --release

# Generate Swift bindings for iOS
./build_ios.sh

# Generate Kotlin bindings for Android  
./build_android.sh

# Generate Python bindings
./build_python.sh
```

## Usage Examples

### Swift (iOS)

```swift
import vss_rust_client_ffi

let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
let passphrase: String? = nil
let storeId = try vssDeriveStoreId(
    prefix: "bitkit_v1_regtest",
    mnemonic: mnemonic,
    passphrase: passphrase
)

try await vssNewClientWithLnurlAuth(
    baseUrl: "https://vss.example.com",
    storeId: storeId,
    mnemonic: mnemonic,
    passphrase: passphrase,
    lnurlAuthServerUrl: "https://auth.example.com/lnurl"
)

// Store data
let item = try await vssStore(
    key: "user-settings",
    value: "{'theme': 'dark'}".data(using: .utf8)!
)
print("Stored at version: \(item.version)")

// Retrieve data
if let item = try await vssGet(key: "user-settings") {
    let data = String(data: item.value, encoding: .utf8)
    print("Retrieved: \(data)")
}

// List all items with prefix
let items = try await vssList(prefix: "user/")
for item in items {
    print("Key: \(item.key), Version: \(item.version)")
}

// Delete data
let wasDeleted = try await vssDelete(key: "user-settings")
print("Deleted: \(wasDeleted)")

// Clean shutdown (optional)
vssShutdownClient()
```

### Python

```python
from vss_rust_client_ffi import *

mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
passphrase = None
store_id = vss_derive_store_id(
    prefix="bitkit_v1_regtest",
    mnemonic=mnemonic,
    passphrase=passphrase
)

await vss_new_client_with_lnurl_auth(
    "https://vss.example.com",
    store_id,
    mnemonic,
    passphrase,
    "https://auth.example.com/lnurl"
)

# Store data
item = await vss_store("user-settings", b"{'theme': 'dark'}")
print(f"Stored at version: {item.version}")

# Retrieve data  
item = await vss_get("user-settings")
if item:
    print(f"Retrieved: {item.value.decode()}")

# List keys only (more efficient)
keys = await vss_list_keys("user/")
for kv in keys:
    print(f"Key: {kv.key}, Version: {kv.version}")

# Batch store multiple items
items_to_store = [
    KeyValue(key="config/theme", value=b"dark"),
    KeyValue(key="config/lang", value=b"en")
]
stored_items = await vss_put_with_key_prefix(items_to_store)
print(f"Stored {len(stored_items)} items")

# Delete data
was_deleted = await vss_delete("user-settings")
print(f"Deleted: {was_deleted}")

# Clean shutdown (optional)
vss_shutdown_client()
```

### Kotlin (Android)

```kotlin
import uniffi.vss_rust_client_ffi.*

val mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
val passphrase: String? = null
val storeId = vssDeriveStoreId(
    prefix = "bitkit_v1_regtest",
    mnemonic = mnemonic,
    passphrase = passphrase
)

vssNewClientWithLnurlAuth(
    baseUrl = "https://vss.example.com",
    storeId = storeId,
    mnemonic = mnemonic,
    passphrase = passphrase,
    lnurlAuthServerUrl = "https://auth.example.com/lnurl"
)

// Store data
val item = vssStore(
    key = "user-settings", 
    value = "{'theme': 'dark'}".toByteArray()
)
println("Stored at version: ${item.version}")

// Retrieve data
val retrievedItem = vssGet("user-settings")
retrievedItem?.let {
    println("Retrieved: ${String(it.value)}")
}

// List all items
val items = vssList(prefix = null)
items.forEach { item ->
    println("Key: ${item.key}, Version: ${item.version}")
}

// Clean shutdown (optional)
vssShutdownClient()
```

## API Reference

### Client Management

#### `vssNewClient(baseUrl: String, storeId: String) -> Void`
Initialize the global VSS client connection without authentication.

- `baseUrl`: VSS server URL (e.g., "https://vss.example.com")
- `storeId`: Unique identifier for your storage namespace  

#### `vssNewClientWithLnurlAuth(baseUrl: String, storeId: String, mnemonic: String, passphrase: String?, lnurlAuthServerUrl: String) -> Void`
Initialize the global VSS client connection with LNURL-auth authentication. Provides automatic JWT token management and data encryption.

- `baseUrl`: VSS server URL (e.g., "https://vss.example.com")
- `storeId`: Unique identifier for your storage namespace
- `mnemonic`: BIP39 mnemonic phrase (12 or 24 words)
- `passphrase`: Optional BIP39 passphrase (pass `null` if none)
- `lnurlAuthServerUrl`: LNURL-auth server URL for authentication

#### `vssShutdownClient() -> Void`
Shutdown the VSS client and clean up resources. Optional but recommended for clean application shutdown.

### Utility Functions

#### `vssDeriveStoreId(prefix: String, mnemonic: String, passphrase: String?) -> String`
Derives a deterministic VSS store ID from a mnemonic and optional passphrase using BIP32 key derivation.

- `prefix`: A prefix to include in the store ID (e.g., "bitkit_v1_regtest")
- `mnemonic`: BIP39 mnemonic phrase (12 or 24 words)  
- `passphrase`: Optional BIP39 passphrase

### Data Operations

#### `vssStore(key: String, value: Data) -> VssItem`
Store a key-value pair. The server automatically manages versioning, incrementing the version number with each update.

#### `vssGet(key: String) -> VssItem?`
Retrieve an item by key. Returns `null` if not found.

#### `vssList(prefix: String?) -> [VssItem]`
List all items, optionally filtered by key prefix. Includes full data.

#### `vssListKeys(prefix: String?) -> [KeyVersion]`
List keys and versions only (more efficient than `vssList`).

#### `vssPutWithKeyPrefix(items: [KeyValue]) -> [VssItem]`
Store multiple items in a single atomic transaction. The server manages versioning for all items.

#### `vssDelete(key: String) -> Bool`
Delete an item. Returns `true` if item existed and was deleted.

### Data Types

#### `VssItem`
- `key: String` - The item key
- `value: Data` - The stored data  
- `version: Int64` - Version number

#### `KeyValue`
- `key: String` - The item key
- `value: Data` - The data to store

#### `KeyVersion`
- `key: String` - The item key
- `version: Int64` - Version number

#### `VssError`
Error enum with detailed error information for different failure scenarios.

## Building from Source

### iOS Framework

```bash
./build_ios.sh
```

Generates:
- `bindings/ios/VssRustClientFfi.xcframework` - iOS framework
- `bindings/ios/vss_rust_client_ffi.swift` - Swift bindings

### Android Library  

```bash
./build_android.sh
```

Generates:
- `bindings/android/vss_rust_client_ffi.kt` - Kotlin bindings
- `bindings/android/jniLibs/` - Native libraries for all Android architectures

### Python Package

```bash
./build_python.sh
```

Generates:
- `bindings/python/` - Python package with bindings and native library

## Development

### Prerequisites

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install uniffi-bindgen
cargo install uniffi_bindgen
```

### Running Tests

```bash
# Run unit tests (recommended)
cargo test tests::tests --lib

# Check compilation
cargo check

# Build library
cargo build --release

# Test bindings generation
cargo run --bin uniffi-bindgen generate \
    --library ./target/release/libvss_rust_client_ffi.dylib \
    --language swift \
    --out-dir ./test_bindings
```

For detailed testing information including integration tests, see [TESTING.md](TESTING.md).

## Architecture

This library provides a thin FFI wrapper around the [vss-client](https://crates.io/crates/vss-client) Rust library, exposing a simplified async API suitable for mobile and cross-platform applications.

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests if applicable  
5. Run `cargo test` and `cargo clippy`
6. Submit a pull request

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Related Projects

- [VSS Server](https://github.com/lightningdevkit/vss-server) - The VSS server implementation
- [vss-client](https://crates.io/crates/vss-client) - The underlying Rust client library
- [UniFFI](https://mozilla.github.io/uniffi-rs/) - The FFI binding generator
