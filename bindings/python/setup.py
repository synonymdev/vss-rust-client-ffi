from setuptools import setup, find_packages
import os

# Try to read README.md if it exists, otherwise use a default description
try:
    with open("README.md", "r") as f:
        long_description = f.read()
except FileNotFoundError:
    long_description = "Python bindings for the VSS Rust Client FFI"

setup(
    name="vss-rust-client-ffi",
    version="0.1.0",
    packages=find_packages(),
    package_data={
        "vss_rust_client_ffi": ["*.so", "*.dylib", "*.dll"],
    },
    install_requires=[],
    author="VSS",
    author_email="",
    description="Python bindings for the VSS Rust Client FFI",
    long_description=long_description,
    long_description_content_type="text/markdown",
    url="",
    classifiers=[
        "Programming Language :: Python :: 3",
        "License :: OSI Approved :: MIT License",
        "Operating System :: OS Independent",
    ],
    python_requires=">=3.6",
)
