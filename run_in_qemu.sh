#!/usr/bin/env bash
# Interactively launch KazuOS in QEMU (wrapper around scripts/launch.rs).
set -e
cd "$(dirname "$0")"
exec cargo +nightly -Zscript scripts/launch.rs "$@"
