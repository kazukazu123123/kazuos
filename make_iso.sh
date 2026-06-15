#!/usr/bin/env bash
# Build the KazuOS bootable ISO (wrapper around scripts/make_iso.rs).
set -e
cd "$(dirname "$0")"
exec cargo +nightly -Zscript scripts/make_iso.rs "$@"
