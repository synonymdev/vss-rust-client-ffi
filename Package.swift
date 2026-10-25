// swift-tools-version:5.9
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

let tag = "v0.5.26-rc1"
let checksum = "8bd5a46b6a0a132a76f8da59b2fc6acc7196e432bb138ed8e5666fcff4389b21"
let url = "https://github.com/synonymdev/vss-rust-client-ffi/releases/download/\(tag)/VssRustClientFfi.xcframework.zip"

let package = Package(
    name: "vss-rust-client-ffi",
    platforms: [
        .iOS(.v17),
    ],
    products: [
        .library(
            name: "VssRustClientFfi",
            targets: ["VssRustClientFfi", "VssRustClientFfiFFI"]
        ),
    ],
    targets: [
        .target(
            name: "VssRustClientFfi",
            dependencies: ["VssRustClientFfiFFI"],
            path: "./bindings/ios",
            sources: ["vss_rust_client_ffi.swift"]
        ),
        .binaryTarget(
            name: "VssRustClientFfiFFI",
            url: url,
            checksum: checksum
        ),
    ]
)
