// swift-tools-version:5.9
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

let tag = "v0.5.26-rc3"
let checksum = "00a411f8b5fbf0f89b619ae6b9164353fd3fa17e6ad6fedd147a3d723e3bdf62"
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
