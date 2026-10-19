#!/usr/bin/env bash
# Publish only the two current static binaries; never create commit/version prefixes.
set -euo pipefail
umask 077

[[ $# == 1 ]] || { echo 'Usage: publish-s3.sh VERIFIED_BUILD.tar.gz' >&2; exit 2; }
: "${AWS_ACCESS_KEY_ID:?Set the S3 access key through the environment}"
: "${AWS_SECRET_ACCESS_KEY:?Set the S3 secret key through the environment}"
: "${AWS_DEFAULT_REGION:=example-region}"
: "${HYDRA_S3_ENDPOINT:=https://s3.example.com}"
: "${HYDRA_S3_BUCKET:=example-hydra-bucket}"
: "${GITHUB_SHA:?Set the verified build commit}"
[[ "$HYDRA_S3_ENDPOINT" == https://* && "$HYDRA_S3_ENDPOINT" != *'@'* && "$HYDRA_S3_ENDPOINT" != *'?'* ]] || { echo 'Use an HTTPS S3 API endpoint without embedded credentials.' >&2; exit 2; }
[[ "$GITHUB_SHA" =~ ^[0-9a-f]{40}$ ]] || { echo 'Expected a full commit SHA.' >&2; exit 2; }
for tool in aws python3 tar sha256sum readelf; do command -v "$tool" >/dev/null; done

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
export AWS_DEFAULT_REGION AWS_EC2_METADATA_DISABLED=true AWS_PAGER=''
# Garage supports ordinary signed payloads; do not require optional SDK trailers.
export AWS_REQUEST_CHECKSUM_CALCULATION=when_required AWS_RESPONSE_CHECKSUM_VALIDATION=when_required
export AWS_CONFIG_FILE="$scratch/aws-config"
cat > "$AWS_CONFIG_FILE" <<'CONFIG'
[default]
s3 =
    addressing_style = path
    signature_version = s3v4
CONFIG

tar -xzf "$1" -C "$scratch" --no-same-owner
(cd "$scratch" && sha256sum -c SHA256SUMS)
grep -Fxq "commit=$GITHUB_SHA" "$scratch/BUILD.txt"
grep -Fxq 'target=x86_64-unknown-linux-musl' "$scratch/BUILD.txt"
# Validate both before replacing either object.
for name in core proxy; do
  binary="$scratch/hydra-$name-linux-x86_64-musl"
  [[ -f "$binary" && ! -L "$binary" ]]
  readelf -l "$binary" > "$scratch/elf-$name.txt"
  if grep -q INTERP "$scratch/elf-$name.txt"; then
    echo "Expected a static binary: hydra-$name" >&2; exit 1
  fi
done
for name in core proxy; do
  binary="$scratch/hydra-$name-linux-x86_64-musl"
  sha=$(sha256sum "$binary" | cut -d ' ' -f 1)
  md5=$(python3 - "$binary" <<'PY'
import base64,hashlib,pathlib,sys
print(base64.b64encode(hashlib.md5(pathlib.Path(sys.argv[1]).read_bytes(),usedforsecurity=False).digest()).decode())
PY
)
  aws --endpoint-url "$HYDRA_S3_ENDPOINT" s3api put-object \
    --bucket "$HYDRA_S3_BUCKET" --key "hydra-$name" --body "$binary" \
    --content-type application/octet-stream --content-md5 "$md5" \
    --cache-control 'no-cache, max-age=0, must-revalidate' \
    --metadata "sha256=$sha,commit=$GITHUB_SHA,target=x86_64-unknown-linux-musl" \
    > "$scratch/put-$name.json"
  # Single-part Garage uploads return the payload MD5 as ETag. This works with
  # a write-only release key: no bucket listing, HEAD or GET permission needed.
  python3 - "$scratch/put-$name.json" "$md5" <<'PY'
import base64,json,pathlib,sys
result=json.loads(pathlib.Path(sys.argv[1]).read_text())
expected=base64.b64decode(sys.argv[2]).hex()
if result.get('ETag','').strip('"').lower()!=expected:
    raise SystemExit('Published object checksum response did not match the binary')
PY
  echo "Published s3://$HYDRA_S3_BUCKET/hydra-$name ($GITHUB_SHA, SHA-256 $sha)"
done
