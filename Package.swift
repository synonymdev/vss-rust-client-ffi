// swift-tools-version:5.9
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

let tag = "v0.5.26-rc3"
let checksum = "f721d8c67445e46cb4a71f9c23bd369e6fc4d089ad4b22dc604f6db9f2e636cc"
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
