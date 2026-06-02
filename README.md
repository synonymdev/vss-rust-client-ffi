# VSS Rust Client FFI

Cross-platform FFI bindings for the [VSS (Versioned Storage Service) Rust Client](https://github.com/lightningdevkit/vss-server), providing a simple interface for mobile applications to interact with VSS servers.

## Installation

### Prerequisites

- Rust 1.75+
- `uniffi-bindgen` for generating bindings

### Building

```bash
# Basic build
cargo build --release

# Generate iOS and Android bindings
./build.sh all

# Generate Swift bindings for iOS
./build_ios.sh

# Generate Kotlin bindings for Android  
./build_android.sh
```

## Usage Examples

### Swift (iOS)

```swift
import VssRustClientFfi

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

#### Dedicated LDK Client (Swift)

For fully isolated LDK operations with independent key derivation (full 64-byte BIP39 seed):

```swift
// Initialize the dedicated LDK client (separate from the app client)
try await vssNewLdkClientWithLnurlAuth(
    baseUrl: "https://vss.example.com",
    storeId: storeId,
    mnemonic: mnemonic,
    passphrase: passphrase,
    lnurlAuthServerUrl: "https://auth.example.com/lnurl"
)

// Read an ldk-node key
if let item = try await vssLdkGet(key: "network_graph", namespace: .default) {
    print("Graph size: \(item.value.count) bytes")
}

// Store a key
let item = try await vssLdkStore(
    key: "network_graph",
    value: graphData,
    namespace: .default
)

// Delete a key
let wasDeleted = try await vssLdkDelete(key: "network_graph", namespace: .default)

// List keys in a namespace
let keys = try await vssLdkListKeys(namespace: .monitors)

// List all keys across all namespaces
let allKeys = try await vssLdkListAllKeys()

// Clean shutdown
vssShutdownLdkClient()
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

#### Dedicated LDK Client (Kotlin)

```kotlin
// Initialize the dedicated LDK client (separate from the app client)
vssNewLdkClientWithLnurlAuth(
    baseUrl = "https://vss.example.com",
    storeId = storeId,
    mnemonic = mnemonic,
    passphrase = passphrase,
    lnurlAuthServerUrl = "https://auth.example.com/lnurl"
)

// Read an ldk-node key
val item = vssLdkGet(key = "network_graph", namespace = LdkNamespace.Default)
item?.let { println("Graph size: ${it.value.size} bytes") }

// Store a key
val stored = vssLdkStore(
    key = "network_graph",
    value = graphData,
    namespace = LdkNamespace.Default
)

// Delete a key
val wasDeleted = vssLdkDelete(key = "network_graph", namespace = LdkNamespace.Default)

// List keys in a namespace
val keys = vssLdkListKeys(namespace = LdkNamespace.Monitors)

// List all keys across all namespaces
val allKeys = vssLdkListAllKeys()

// Clean shutdown
vssShutdownLdkClient()
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

### Dedicated LDK Client

A fully separate client (`LdkVssClient`) with its own key derivation using the full 64-byte BIP39 seed, independent from the app backup client. Use this when LDK operations must be completely isolated from app backup operations.

#### `vssNewLdkClientWithLnurlAuth(baseUrl: String, storeId: String, mnemonic: String, passphrase: String?, lnurlAuthServerUrl: String) -> Void`
Initialize the dedicated LDK client with LNURL-auth authentication. This client is fully separate from the app client initialized with `vssNewClientWithLnurlAuth`.

#### `vssShutdownLdkClient() -> Void`
Shutdown the dedicated LDK client and clean up resources.

#### `vssLdkStore(key: String, value: Data, namespace: LdkNamespace) -> VssItem`
Store a key-value pair using the dedicated LDK client.

#### `vssLdkGet(key: String, namespace: LdkNamespace) -> VssItem?`
Retrieve an item by key using the dedicated LDK client. Returns `null` if not found.

#### `vssLdkDelete(key: String, namespace: LdkNamespace) -> Bool`
Delete an item using the dedicated LDK client. Returns `true` if item existed and was deleted.

#### `vssLdkListKeys(namespace: LdkNamespace) -> [KeyVersion]`
List keys in a namespace using the dedicated LDK client.

#### `vssLdkListAllKeys() -> [KeyVersion]`
List all keys across all singleton LDK namespaces using the dedicated LDK client.

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

#### `LdkNamespace`
- `Default` - Default namespace (channel_manager, scorer, etc.)
- `Monitors` - Channel monitors
- `MonitorUpdates(monitorId)` - Updates for a specific monitor
- `ArchivedMonitors` - Archived channel monitors

#### `VssError`
Error enum with detailed error information for different failure scenarios.

## Building from Source

### iOS Framework

```bash
./build_ios.sh
```

Generates:
- `dist/ios/VssRustClientFfi.xcframework` - iOS framework
- `dist/ios/VssRustClientFfi.xcframework.zip` - zipped SwiftPM binary artifact for release upload
- `bindings/ios/vss_rust_client_ffi.swift` - Swift bindings source used by `Package.swift`
- `bindings/ios/vss_rust_client_ffiFFI.h` and `bindings/ios/module.modulemap` - C interface files used by the Swift package

### Android Library  

```bash
./build_android.sh
```

Generates:
- `bindings/android/vss_rust_client_ffi.kt` - Kotlin bindings
- `bindings/android/jniLibs/` - Native libraries for all Android architectures

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

## Architecture

This library provides a thin FFI wrapper around the [vss-client-ng](https://crates.io/crates/vss-client-ng) Rust library, exposing a simplified async API suitable for mobile and cross-platform applications.

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
- [vss-client-ng](https://crates.io/crates/vss-client-ng) - The underlying Rust client library
- [UniFFI](https://mozilla.github.io/uniffi-rs/) - The FFI binding generator
