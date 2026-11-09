// swift-tools-version:5.9
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

let tag = "v0.5.26-rc2"
let checksum = "4ef61f9158339a155c9988a4ceec9f7032895b83851478abc49ead0094d2c435"
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
