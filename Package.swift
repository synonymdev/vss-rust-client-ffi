// swift-tools-version:5.5
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

let tag = "v0.5.6"
let checksum = "11df637177c2f7f3b222ece10dee1d9a6c3c23905d12884d6085de967f498fc5"
let url = "https://github.com/synonymdev/vss-rust-client-ffi/releases/download/\(tag)/VssRustClientFfi.xcframework.zip"

let package = Package(
    name: "vss-rust-client-ffi",
    platforms: [
        .iOS(.v13),
        .macOS(.v11),
    ],
    products: [
        .library(
            name: "VssRustClientFfi",
            targets: ["VssRustClientFfi", "VssRustClientFfiFFI"]),
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
        )
    ]
)
