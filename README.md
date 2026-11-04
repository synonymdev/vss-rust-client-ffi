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

### Conditional backup transactions

`vss_put_conditional` (Swift `vssPutConditional`, Kotlin `vssPutConditional`)
accepts `ConditionalKeyValue` records with a key, plaintext value and expected
server version. Zero creates a key that must not exist; a positive version from
`vss_get` updates an existing key. Every key must match or the server rejects the
whole transaction. The acknowledged result contains each key's resulting version
(expected version plus one). Values use the existing seed-derived encryption and
key-bound authentication.

Save the complete candidate values and expected versions durably before sending.
This operation makes one mutation attempt, with transport retries and redirects
disabled. `Conflict` means a version did not match, while `OutcomeUnknown` means
the transaction may have committed. Cancellation also leaves an uncertain outcome.
After uncertainty, read and compare the retained candidate under the original
wallet identity before deciding whether to retry. Never replace the candidate
with newly captured data or fall back to an unconditional write. A client replacement
does not transfer an in-flight transaction to the replacement client's store.

Use this API only for keys whose writers consistently use conditional versions
and do not delete/recreate those keys. Legacy unconditional writes reset key
versions to one, so conditional writes alone cannot safely migrate a legacy wallet
or exclude old clients. Migration policy, immutable component references, causal
snapshot consistency and restore gates are application/server responsibilities.
This API does not enable a new Bitkit backup format.

### Exact encrypted-object read observations (Rust source API)

`vss_get_with_receipt(key, max_response_bytes)` returns the decrypted app object,
logical and opaque storage keys, configured store ID, actual outer server version,
and a lowercase SHA-256 digest of the exact encrypted `KeyValue.value` bytes. The
hash is computed before protobuf decoding, including any unknown envelope fields.
Legacy unconditional envelopes with embedded version `-1` remain readable; other
embedded versions must match the positive outer version. Key derivation and
key-bound authentication are unchanged.

The storage request is made once without redirects. Only an explicit typed HTTP
404 missing-key response returns `None`; an empty success, malformed response,
authentication failure, wrong key/version, or exceeded limit is an error. The
caller chooses a complete protobuf response limit between 1 byte and 64 MiB. The
ten-second deadline includes credential acquisition and response reading; the
byte limit applies to the storage response, not the existing credential provider's
separate requests. Authentic encrypted empty plaintext remains a valid object.

This is a local observation, not a signed server receipt or proof of freshness,
principal identity, migration authority, or multi-object coherence. Keep the
original wallet and endpoint context, reject stale-owner completions, and never
log the receipt or its plaintext. Replacing or shutting down the global client
does not retarget or necessarily cancel an in-flight read. No backup format,
reader/writer fence, or app activation is enabled by this API.

This operation and its record are not exported through UniFFI. The checked-in
Swift package and regenerated interfaces continue to match the released API.
Swift/Kotlin availability requires a coordinated SDK release that adds the exports,
regenerates the interfaces, and publishes their matching native binaries. The
crate currently produces static and dynamic libraries, not a Rust dependency
library; this is a Rust source API, not a new distributed SDK entry point.

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
This constructor is rejected and always returns `AuthError`. Unauthenticated VSS does not
have a seed-derived encryption key. Use `vssNewClientWithLnurlAuth` instead.

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
- `bindings/android/src/main/kotlin/com/synonym/vssclient/vss_rust_client_ffi.kt` - Kotlin bindings
- `bindings/android/src/main/jniLibs/` - native libraries for all Android architectures
- `bindings/android/native-debug-symbols.zip` - native debug symbols for release metadata

Generated iOS and Android artifacts are ignored by git and should be produced by the build scripts or CI instead of committed.

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

### Retained backup read session (Rust source preparation)

`BackupReadSession` constructs the app and dedicated LDK clients from one full
wallet seed and one endpoint/store/auth configuration. It preserves the truncated
app derivation and full-seed LDK derivation. It never combines separately replaced
global clients. `read(BackupReadTarget, byte_limit)` produces an opaque observation;
`receipt(&observation)` checks its original session and revocation state. Even a
second session with identical wallet arguments cannot adopt that observation.
Typed absence retains the original logical target and owner as well.

`revoke()` permanently closes local read admission and wakes pending reads. A
cancelled/dropped read schedules no retry. Revocation cannot retract an already
admitted network request, previously borrowed data or copied plaintext. A receipt
check is valid only at that instant; it grants no later mutation authority. The
storage response is capped, while the inherited credential provider's separate
responses are not. The ten-second deadline covers both credential acquisition and
storage. No lock is held across a network await.

LDK receipts use the dedicated client's existing V1 namespace/AAD and full-seed
encryption. A V1 missing-key result says nothing about older V0 objects. This client
does not control a separately running ldk-node. Equal store IDs do not establish
physical principal identity, and sequential reads do not prove a coherent archive.
App lifecycle drains, authoritative principal mapping, old-reader/writer exclusion
and conditional migration remain separate prerequisites.

These types and methods have no UniFFI export. Like the earlier receipt primitive,
they are in-crate Rust source preparation: this package still emits only static
and dynamic libraries, not a downstream Cargo `rlib`. Released Swift/Kotlin
interfaces, binary tags and checksums are unchanged. No migration or writer is
activated by creating or reading a session. Do not log observations or plaintext.


### Original-wallet migration status (Rust source preparation)

`BackupReadSession::read_migration_state` observes the candidate VSS status contract
through the original app or LDK credential provider. Each result retains its session
and domain, including the server-reported physical principal/store. Different
principals may share the same textual store ID. A renewed proof can return a changed
mapping; old observations remain unchanged and never authorize a later mutation.

An explicit legacy tuple is distinct from committed `bitkit-migration-v1` metadata.
Unsupported routes, authorization failures, incomplete or conflicting fields,
unknown protocols/epochs and over-limit responses remain errors. The storage POST
has no automatic retry or redirect, a 64 KiB body limit, and a ten-second deadline
covering credentials and body. The inherited credential provider keeps its own
challenge/callback redirect and body policies. Dropping a read does not replay it;
session revocation rejects pending and later results.

This is unpublished-contract, in-crate source preparation: no new UniFFI export,
released native API, server deployment or app activation. A single response cannot
prove all-tuples coherence, deployed principal mapping or old reader/writer
exclusion. Before any mutation an enforcing server must atomically recheck fresh
authorization against the observed original principal/store. Do not serialize
observations, log identities/proofs or treat status as permission to migrate.

### Immutable migration proposal (Rust source preparation)

`BackupReadSession::prepare_migration` consumes references to the original session's
app and LDK status observations, original logical `WALLET` observation and present
app component observations. Components must already be safe to make immutable;
ordinary mutable app keys, including same-key USDT retries that re-encrypt, cannot
be substituted for immutable snapshots. This semantic remains the app's duty.
LDK operational objects are rejected as components, while LDK policy still
participates in the proposal. It preserves the legacy archive's encrypted key/AAD and
exact raw envelope digest; explicit absence is a separate condition. It encrypts
the supplied initial head once with the original app key, without uploading or
replacing any object. The app remains responsible for the head schema and complete,
coherent capture. An opaque wallet value alone cannot prove that completeness.

Every domain must have a positive legacy status. Equal store IDs do not combine
identities: only an exact match of both reported principal and store coalesces app
and LDK into one physical tuple, retaining both domain labels. Duplicate/conflicting
component keys, destructive head/archive collisions, noncanonical UUIDs and inputs
outside the candidate server's activation limits are rejected. The digest uses the
server's UTF-8 tuple/key ordering and exact field encoding, including the encrypted
head and all archive/component conditions. It contains no renewable proof.

Access the frozen result through `prepared_migration`. Another session cannot adopt
it, even with the same seed. Revocation or starting either domain's next status read
invalidates access and preparation from earlier status observations. Failed,
pending and abandoned status reads also invalidate earlier preparation; historical
status remains readable. Repeated access retains the same ciphertext/digest. Calling
preparation again requires a fresh UUID and creates different encrypted content,
not a retry of the old plan. This preparation API has no durable UUID registry;
future dispatch must preserve one exact attempt across uncertain outcomes.

This content is neither persisted authority nor an execution API. Separate status
and object reads cannot prove a coherent physical snapshot or stable authorization
mapping between requests. A future enforcing server must atomically compare fresh
proofs against every original physical tuple and exact envelope condition before
mutation. Lifecycle drains, RN and running-node capabilities, old reader/writer
exclusion, durable uncertain-outcome reconciliation and app restoration remain
unfinished requirements. No activation request, new UniFFI API, release or deployed
server support is introduced here. Never log plaintext, proofs or private identities.

Coalescing does not verify that both credential providers still map to the shared
tuple. The preserved server candidate accepts one proof per unique target; that
alone cannot recheck a second provider which has since remapped elsewhere. Retained
domain labels are necessary, but enforcing that condition remains unresolved before
any future activation.

### Retained migration outcome observations (Rust source only)

`BackupReadSession::reconcile_migration` compares an opaque retained proposal with
fresh status reads through both original app and LDK clients, including when their
original physical tuple was shared. It checks each original principal/store and the
exact UUID, all-tuples digest, archive presence/hash, initial encrypted-head hash and
complete component set. Component wire order is irrelevant. It compares the immutable
initial receipt, never a later mutable head or a substitute wallet export.

Only two matching committed receipts return `ObservedCommitted`; two expected legacy
receipts return `ObservedUncommitted`. Neither is an atomic snapshot or write/replay
permit. Read failures, malformed/unsupported responses, remapped providers, mismatched
receipts, mixed outcomes, superseding reads and owner revocation remain errors.
Each read obtains a new token with its captured original signer, even when the ordinary
provider holds a valid cached token. Fresh authentication failure cannot fall back to
that token. The ten-second deadline includes authentication; the VSS storage POST has
no automatic retry or redirect. The inherited LNURL GETs retain their own redirect
policy and uncapped response parsing within that deadline. Cancellation
abandons pending reads; revocation discards their results. Another session, even with
the same seed, cannot adopt the handle. Starting reconciliation invalidates preparation
access and does not restore it. The retained immutable content is used only for comparison.

This is in-memory native source, without new UniFFI exports or durable restart adoption.
Sequential provider reads do not repair the coalesced-provider atomic enforcement gap
above. Server deployment, app lifecycle admission, legacy-reader exclusion and complete
coherent capture/restore remain separate prerequisites; no migration is dispatched.

The candidate server may initialize an empty legacy policy row during a status read.
This client invokes only status and authentication routes; it does not assert that
server implementation performs zero database writes.
