# CI releases

`.github/workflows/ci.yml` runs on main pushes affecting source/build/test/workflow paths, matching pull requests, and manual dispatch. The verification job builds Vue into Core, validates contracts, runs Rust/frontend/integration tests and produces static x86_64 Linux binaries. The existing Actions ZIP/tar artifact remains downloadable for 30 days.

The final **Publish latest Linux binaries to S3** job depends on successful verification. It runs only for main pushes or manual main dispatches. It downloads the artifact from that exact run, checks the current main SHA, and skips superseded builds. Publication is serialized; an in-progress upload is not canceled by a newer run.

## S3 destination

| Setting | Value |
| --- | --- |
| API endpoint | `https://s3.example.com` |
| Region | `example-region` |
| Bucket | `example-hydra-bucket` |
| Core object | `hydra-core` |
| Proxy object | `hydra-proxy` |

Only these two fixed keys are written. Each successful build overwrites them; no commit prefixes, archives, manifests or timestamped objects are uploaded to S3. Other existing bucket contents are untouched. Both objects carry `commit`, `sha256` and `target` metadata and a revalidation cache policy.

GitHub repository secrets:

- `HYDRA_S3_ACCESS_KEY_ID`
- `HYDRA_S3_SECRET_ACCESS_KEY`

These are configured for `nrf24l01/ad-hydra`. To replace either value, use repository **Settings → Secrets and variables → Actions**, or `gh secret set NAME` and enter its value through the hidden prompt. Never place credentials in this repository, workflow literals, URLs or command arguments.

The runner needs Ubuntu 24.04's AWS CLI, GitHub CLI, Python, tar, readelf and sha256sum. The publish script forces path-style Signature V4 against the custom HTTPS endpoint. The release key needs object upload permission for the two keys; bucket listing and object read permission are not required. Before either upload, it checks the archive's binary hashes, BUILD.txt commit/target and static ELF headers. Each upload sends Content-MD5 and validates Garage's single-part ETag response against that checksum. SHA-256 is retained as object metadata.

Two separate S3 objects cannot be replaced atomically: if one upload fails, the job fails and rerunning publication replaces both. There is no extra staging object/manifest because the release bucket is intended to contain only the latest pair. Object metadata identifies the commit for each binary.

## Reproduce publication

With credentials already exported securely, download the verified Actions artifact and run:

```bash
export AWS_DEFAULT_REGION=example-region
export HYDRA_S3_ENDPOINT=https://s3.example.com
export HYDRA_S3_BUCKET=example-hydra-bucket
export GITHUB_SHA=FULL_VERIFIED_COMMIT_SHA
scripts/publish-s3.sh /path/to/hydra-linux-x86_64-musl.tar.gz
```

For routine publication, push to main or manually dispatch **Rust verification** on main. Tests must pass before the upload job starts.
