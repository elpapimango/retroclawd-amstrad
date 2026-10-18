#!/usr/bin/env bash
# Builds the media the scenario manifests load, with retroclawd-tools, into software/scenarios/ (not committed).
#   scenarios/build.sh [path to the retroclawd-tools target dir]   default ../../../../retroclawd-tools/target/debug
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
tools="${1:-$here/../../../../retroclawd-tools/target/debug}"
out="$here/../software/scenarios"
mkdir -p "$out"
"$tools/rcld" build "$here/cpc/hello.toml" -o "$out/cpc-hello.dsk" --force
