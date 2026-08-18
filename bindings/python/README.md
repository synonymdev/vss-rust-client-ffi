# VSS Rust Client FFI Python Bindings

Python bindings for the VSS Rust Client FFI.

## Installation

```bash
pip install .
```

## Usage

```python
import asyncio

from vss_rust_client_ffi import *

async def main():
    # Initialize VSS with LNURL-auth so backups use a seed-derived encryption key
    await vss_new_client_with_lnurl_auth(
        "https://vss.example.com",
        "my-store",
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        None,
        "https://auth.example.com/lnurl",
    )

    # Store data
    item = await vss_store("my-key", b"my-data")
    print(f"Stored at version: {item.version}")

asyncio.run(main())
```
