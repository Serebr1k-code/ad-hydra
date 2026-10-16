#!/bin/sh
set -eu
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
protoc -I contracts --descriptor_set_out="$scratch/proxy.pb" contracts/hydra/proxy/v1/proxy.proto
cargo run --locked --quiet -p hydra-core -- openapi > "$scratch/openapi.yaml"
diff -u contracts/openapi.yaml "$scratch/openapi.yaml"
python3 scripts/check-architecture.py
