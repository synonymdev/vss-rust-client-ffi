# VSS Rust Client FFI Python Bindings

Python bindings for the VSS Rust Client FFI.

## Installation

```bash
pip install .
```

## Usage

```python
from vss_rust_client_ffi import *

# Initialize VSS client with LNURL-auth so backups are encrypted with a seed-derived key
vss_new_client_with_lnurl_auth(
    "https://vss.example.com",
    "my-store",
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    None,
    "https://auth.example.com/lnurl",
)

# Store data
item = vss_store("my-key", b"my-data")
print(f"Stored at version: {item.version}")
```
