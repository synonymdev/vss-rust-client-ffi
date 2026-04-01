# Changelog

All notable changes to this project will be documented in this file.

## [v0.5.13] - 2026-04-01

### Fixed

- Flattened xcframework headers layout for Xcode 26 SPM compatibility — removed nested `Headers/VssRustClientFfiFFI/` subdirectory
- Added `use "Darwin"`, `use "_Builtin_stdbool"`, `use "_Builtin_stdint"` directives to modulemap (injected via `sed` for UniFFI 0.28.3 compatibility)
- Restored `url` + `checksum` in Package.swift binaryTarget

## [v0.5.12] - 2026-02-11

### Added

- Dedicated `LdkVssClient` with its own global singleton, fully separate from the app backup `VssClient`
- `LdkNamespace` enum for type-safe namespace addressing of ldk-node's obfuscated key format
- Dedicated LDK client APIs: `vss_new_ldk_client_with_lnurl_auth`, `vss_shutdown_ldk_client`, `vss_ldk_get`, `vss_ldk_store`, `vss_ldk_delete`, `vss_ldk_list_keys`, `vss_ldk_list_all_keys`

### Changed

- Dual key derivation: app backups use truncated 32-byte seed (backward-compatible with v0.4.0); LDK backups use full 64-byte BIP39 seed (matching ldk-node's key derivation)

## [v0.4.0] - 2025-12-09

### Changed

- Updated `vss-client` dependency from `0.3` to `vss-client-ng` `0.4`
- Adapted to new `StorableBuilder` API (`AAD` parameter, key by reference)
- Store `data_encryption_key` separately in `VssClient` struct
- Updated MSRV to `1.75`+

### Breaking

- Data encrypted with `vss-client` `0.3` is not compatible with this version due to `AAD` changes in the encryption scheme

## [v0.3.2] - 2025-11-03

Initial release.

### Added

- Cross-platform FFI bindings for the VSS Rust Client via UniFFI
- Swift (iOS), Kotlin (Android), and Python binding generation
- LNURL-auth JWT authentication support
- Store ID derivation from BIP39 mnemonic
- GitHub Packages distribution for Android

[v0.5.13]: https://github.com/synonymdev/vss-rust-client-ffi/releases/tag/v0.5.13
[v0.5.12]: https://github.com/synonymdev/vss-rust-client-ffi/releases/tag/v0.5.12
[v0.4.0]: https://github.com/synonymdev/vss-rust-client-ffi/releases/tag/v0.4.0
[v0.3.2]: https://github.com/synonymdev/vss-rust-client-ffi/releases/tag/v0.3.2
