# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Cross-platform FFI library wrapping the [VSS (Versioned Storage Service) Rust Client](https://github.com/lightningdevkit/vss-server) via UniFFI. Generates bindings for Swift (iOS), Kotlin (Android), and Python. Used by mobile Lightning wallets to back up both app data and LDK (Lightning Development Kit) channel state to a VSS server.

## Build & Test Commands

```bash
cargo build                  # Debug build
cargo build --release        # Release build
cargo test                   # All tests
cargo test tests::tests --lib    # Unit tests only
cargo test ffi_tests --lib       # FFI interface tests only
cargo clippy                 # Lint

# Platform bindings (requires platform-specific toolchains)
./build.sh ios               # XCFramework + Swift bindings
./build.sh android           # JNI libs + Kotlin bindings
./build.sh python            # Python package
./build.sh all               # All platforms
```

## Architecture

### Dual Client Design

The library maintains two separate global singleton clients, each with its own key derivation:

- **VssClient** (`implementation.rs`) — App backup client. Uses a **truncated 32-byte** seed for key derivation (backward compatible with v0.4.0). Manages general app data storage.
- **LdkVssClient** (`ldk_client.rs`) — Dedicated LDK client. Uses the **full 64-byte** BIP39 seed, matching ldk-node's exact key derivation chain so it can correctly deobfuscate keys stored by ldk-node's VssStore.

Both clients are stored as `OnceCell<Arc<Mutex<Option<T>>>>` statics in `lib.rs`.

### Key Derivation Paths (BIP32)

- `m/877'` — VSS master derivation → 32-byte `vss_seed` → HKDF → (data_encryption_key, obfuscation_key)
- `m/877'/138'` — LNURL-auth signing key (both clients derive from truncated seed for same server identity)
- `m/877'/118'` — Store ID derivation

The critical difference: app client truncates the 64-byte BIP39 seed to 32 bytes before deriving the master key, while the LDK client uses all 64 bytes. This must be preserved for backward compatibility.

### FFI Layer (`lib.rs`)

- `execute_async!` macro bridges sync FFI calls to the single-threaded Tokio runtime
- All exported functions are annotated with `#[uniffi::export]`
- `uniffi::setup_scaffolding!()` generates the FFI glue code
- UniFFI config in `uniffi.toml` (Kotlin package: `com.synonym.vssclient`)

### LDK Namespaces (`types.rs`)

`LdkNamespace` enum maps to ldk-node's storage layout: `Default`, `Monitors`, `MonitorUpdates { monitor_id }`, `ArchivedMonitors`. Each namespace becomes a key prefix in VSS.

### Retry Policy

Exponential backoff (10ms base, 10ms jitter, max 10 attempts, 15s total). Skips retry for NoSuchKey, InvalidRequest, and Conflict errors.

## Related Repositories

When investigating VSS server behavior, protocol details, or LDK key derivation, check the sibling repos:

- **vss-server**: `../vss-server`
- **ldk-node**: `../ldk-node`

## Release Workflow (MANDATORY)

1. **ALWAYS** bump the version in `Cargo.toml` before generating bindings.
2. **ALWAYS** bump the iOS framework tag in `Package.swift` to match the new version.
3. **ALWAYS** regenerate all bindings with `./build.sh all` after any code changes.
4. **ALWAYS** upload `dist/ios/VssRustClientFfi.xcframework.zip` to new GitHub releases.

## Commit Convention

```
feat: ...      # New features
fix: ...       # Bug fixes
refactor: ...  # Code restructuring
chore: ...     # Bindings updates, dependencies
```
