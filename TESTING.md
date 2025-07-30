# Testing Guide

This document explains how to run tests for the VSS Rust Client FFI library.

## Test Types

### Unit Tests ✅
Tests basic functionality without requiring external services:
- Client creation
- Error type creation and display  
- Data type instantiation
- Input validation

**Run unit tests:**
```bash
cargo test tests::tests --lib
```

### Integration Tests (Commented Out)
These tests require a live VSS server and are currently commented out because:
1. The provided backup server (`https://blocktank.synonym.to/backups-ldk`) appears to be LDK-specific, not a standard VSS server
2. It doesn't respond to the standard VSS API endpoints (`/getObject`, `/putObjects`, etc.)

**To enable integration tests:**
1. Set up or get access to a VSS server that implements the standard VSS API
2. Update the constants in `src/tests.rs`:
   ```rust
   const INTEGRATION_BASE_URL: &str = "https://your-vss-server.com";
   const INTEGRATION_STORE_ID: &str = "your-store-id";
   ```
3. Uncomment the integration tests
4. Run with: `cargo test --ignored`

## Test Coverage

Current tests cover:
- ✅ Client initialization
- ✅ Error handling for uninitialized clients
- ✅ Data structure creation and validation
- ✅ Error type functionality
- ❌ Actual VSS operations (store, get, list, delete) - requires live server
- ❌ Version tracking - requires live server
- ❌ Batch operations - requires live server

## Running All Available Tests

```bash
# Run all tests (unit and FFI)
cargo test --lib

# Run only unit tests
cargo test tests::tests --lib

# Run only FFI tests
cargo test ffi_tests::ffi_tests --lib

# Run with output for debugging
cargo test --lib -- --nocapture
```

## Server Requirements for Integration Tests

To run full integration tests, you need a VSS server that supports:
- `POST /getObject` - Retrieve objects by key
- `POST /putObjects` - Store objects  
- `POST /deleteObject` - Delete objects
- `POST /listKeyVersions` - List keys with versions

The server should accept protobuf-encoded requests with `Content-Type: application/octet-stream`.

## Setting Up a Local VSS Server for Testing

If you want to run integration tests, you'll need access to a VSS server. Here are some options:

### Option 1: Use the Official VSS Server
Check the [VSS Server repository](https://github.com/lightningdevkit/vss-server) for setup instructions.

### Option 2: Mock Server for Testing
You could create a simple mock server that implements the VSS API endpoints for testing purposes:

```bash
# Example using a simple HTTP server that accepts the right endpoints
# This would need to be implemented to handle protobuf requests
curl -X POST "http://localhost:8080/putObjects" \
  -H "Content-Type: application/octet-stream" \
  --data-binary @request.pb
```

### Option 3: Docker Setup
If available, use a Docker container running a VSS server:

```bash
# This is an example - check the VSS server docs for actual Docker setup
docker run -p 8080:8080 vss-server:latest
```

Then update your test constants:
```rust
const INTEGRATION_BASE_URL: &str = "http://localhost:8080";
const INTEGRATION_STORE_ID: &str = "test-store";
```

## Contributing Test Improvements

If you have access to a VSS server or want to set up integration tests:
1. Update the server constants in the test files
2. Uncomment the integration tests
3. Run the tests to ensure they pass
4. Submit a PR with your improvements

For questions about testing, please check the main README or open an issue.