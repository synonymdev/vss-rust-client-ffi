# VSS Rust Client FFI Python Bindings

Python bindings for the VSS Rust Client FFI.

## Installation

```bash
pip install .
```

## Usage

```python
from vss_rust_client_ffi import *

# Initialize VSS client
vss_new_client(
    "https://vss.example.com",
    "my-store",
    None
)

# Store data
item = vss_store("my-key", b"my-data")
print(f"Stored at version: {item.version}")
```
