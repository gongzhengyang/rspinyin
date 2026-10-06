#!/usr/bin/env bash
# Builds the commit-probe client that FEAT-TEST-P0.02.02 specifies.
#
# The client is a small C program on purpose: it must never enter the workspace's
# dependency closure (AGENTS.md 3.5 -- GTK is the platform's own, not the product's),
# and a one-line gcc call keeps the test scaffold auditable. The output path is the
# one `xtask testd-client --program` documents.
set -euo pipefail
out="${1:-target/rspinyin-test-client}"
mkdir -p "$(dirname "$out")"
pkg-config --exists gtk+-3.0 || {
    echo "build.sh: gtk+-3.0 development files are missing (libgtk-3-dev / gtk3-devel)" >&2
    exit 1
}
gcc -O2 -Wall -o "$out" "$(dirname "$0")/rspinyin_probe_client.c" \
    $(pkg-config --cflags --libs gtk+-3.0)
echo "built: $out"
