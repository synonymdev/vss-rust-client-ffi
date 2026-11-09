#!/usr/bin/env python3

import argparse
import re
import subprocess
import sys
from pathlib import Path
from typing import Optional


def normalize_tag(tag: str) -> str:
    tag = tag.strip()
    if not tag:
        raise ValueError("Release tag must not be empty")
    return tag if tag.startswith("v") else f"v{tag}"


def read_manifest(package_path: Path) -> tuple[str, str]:
    package = package_path.read_text()

    tag_match = re.search(r'^let\s+tag\s*=\s*"([^"]+)"', package, re.MULTILINE)
    checksum_match = re.search(r'^let\s+checksum\s*=\s*"([^"]+)"', package, re.MULTILINE)

    if tag_match is None or checksum_match is None:
        raise ValueError(f"Failed to read tag/checksum from {package_path}")

    return tag_match.group(1), checksum_match.group(1)


def write_output(path: Optional[str], name: str, value: str) -> None:
    if path is None:
        print(f"{name}={value}")
        return

    with open(path, "a") as output:
        output.write(f"{name}={value}\n")


def compute_checksum(artifact_path: Path) -> str:
    result = subprocess.run(
        ["swift", "package", "compute-checksum", str(artifact_path)],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def capture_manifest(args: argparse.Namespace) -> None:
    release_tag = normalize_tag(args.release_tag)
    manifest_tag, manifest_checksum = read_manifest(args.package)

    if manifest_tag != release_tag:
        raise ValueError(f"Package.swift tag ({manifest_tag}) does not match release tag ({release_tag})")

    write_output(args.github_output, "tag", manifest_tag)
    write_output(args.github_output, "checksum", manifest_checksum)


def verify_artifact(args: argparse.Namespace) -> None:
    release_tag = normalize_tag(args.release_tag)
    manifest_tag, _ = read_manifest(args.package)

    if manifest_tag != release_tag:
        raise ValueError(f"Package.swift tag ({manifest_tag}) does not match release tag ({release_tag})")

    if not args.artifact.is_file():
        raise ValueError(f"Missing artifact: {args.artifact}")

    checksum = compute_checksum(args.artifact)
    if checksum != args.expected_checksum:
        raise ValueError(
            "Generated artifact checksum does not match Package.swift\n"
            f"Package.swift checksum: {args.expected_checksum}\n"
            f"Generated checksum: {checksum}"
        )

    write_output(args.github_output, "checksum", checksum)
    print(f"SwiftPM checksum: {checksum}")

    if args.github_summary is not None:
        with open(args.github_summary, "a") as summary:
            summary.write("## iOS release artifact\n\n")
            summary.write(f"- Tag: {release_tag}\n")
            summary.write(f"- SwiftPM checksum: `{checksum}`\n")


def main() -> None:
    parser = argparse.ArgumentParser(description="Validate iOS SwiftPM release artifacts")
    parser.add_argument("--package", type=Path, default=Path("Package.swift"), help="Path to Package.swift")
    subparsers = parser.add_subparsers(dest="command", required=True)

    manifest = subparsers.add_parser("manifest", help="Read and validate the committed SwiftPM release manifest")
    manifest.add_argument("--release-tag", required=True)
    manifest.add_argument("--github-output", default=None)
    manifest.set_defaults(func=capture_manifest)

    verify = subparsers.add_parser("verify", help="Verify the generated SwiftPM binary artifact")
    verify.add_argument("--release-tag", required=True)
    verify.add_argument("--expected-checksum", required=True)
    verify.add_argument("--artifact", type=Path, default=Path("bindings/ios/VssRustClientFfi.xcframework.zip"))
    verify.add_argument("--github-output", default=None)
    verify.add_argument("--github-summary", default=None)
    verify.set_defaults(func=verify_artifact)

    args = parser.parse_args()

    try:
        args.func(args)
    except Exception as error:
        print(error, file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
