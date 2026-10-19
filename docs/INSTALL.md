# Install Hydra from a build

Use an x86_64 Linux machine running systemd (Debian/Ubuntu or a dnf-based distribution). Hydra runs as two services; Rust, Node and Docker are not needed on the machine.

1. Copy [install-hydra.sh](../scripts/install-hydra.sh) onto the machine.
2. Edit `DOWNLOAD_URL` near the top to your HTTPS build URL.
3. Run `sudo bash install-hydra.sh`.

The URL accepts the CI tarball, its outer Actions ZIP, or the GitHub Actions artifacts index. The default index selects the newest unexpired **main** build; a new commit becomes available only when CI publishes its artifact. GitHub Actions downloads require a token with Actions read access. Enter it without putting it in shell history:

```bash
read -rs -p 'GitHub token: ' HYDRA_GITHUB_TOKEN; echo
export HYDRA_GITHUB_TOKEN
sudo --preserve-env=HYDRA_GITHUB_TOKEN bash install-hydra.sh
unset HYDRA_GITHUB_TOKEN
```

A direct HTTPS download from your own server needs no GitHub token. Set `ARCHIVE_SHA256` at the top for an additional trusted archive checksum. Every build must contain both binaries and the CI-generated `SHA256SUMS`; the installer verifies them before installing. The optional `HYDRA_DOWNLOAD_URL` and `HYDRA_ARCHIVE_SHA256` environment variables override those values.

For an already downloaded artifact, use:

```bash
sudo bash install-hydra.sh --archive /path/to/hydra-linux-x86_64-musl.tar.gz
```

You can also supply the Actions ZIP. To check/download a build without installing anything, run `bash install-hydra.sh --download-only /tmp/hydra-verified` (the output directory must not exist).

## Open the console

The installer creates private state, service users, a localhost TLS certificate, systemd units, and a random initial account through Hydra's CLI. Read its credentials as root:

```bash
sudo cat /root/hydra-install-credentials.json
```

From your workstation, forward the console port and open **https://localhost:8080**:

```bash
ssh -L 8080:127.0.0.1:8080 root@YOUR_MACHINE
```

Accept the generated certificate for your own machine. Configure Services with each listener/upstream pair, choose its decoder and enable it. Configure authorized Farm teams explicitly. Competition service applications are installed separately; this script installs Hydra itself. Swagger is at `/docs/`.

Python execution uses pinned `uv` (installed if absent). Host execution remains an explicit Settings choice; the initial mode requires sandbox configuration before exploit execution. No submission destination or flag regex is invented.

## Upgrade and recovery

Running the same command again upgrades binaries while preserving existing users, database, certificates, settings, service units and their drop-ins. Existing accounts are never reset; the initial credential file is only created when the database has no users. Use your existing credentials after an upgrade.

The installer stops Core before taking a consistent SQLite backup, then verifies both services, gRPC and the HTTPS console. Failed installation restores the prior binaries, configuration and database and restarts services that were previously running. Backups are root-private under `/var/backups/hydra/`; retain or remove old backups according to your recovery needs. Newly installed OS packages, service users and directories remain after rollback.

Standard paths are `/etc/hydra/`, `/var/lib/hydra-core/`, `/var/lib/hydra-proxy/` and `/usr/local/bin/hydra-{core,proxy}`. Existing deployments with custom Core database, key or operations paths must use their normal deployment procedure. This script does not overwrite custom unit files. Console access stays on loopback; Proxy listeners bind the addresses configured in Services, including low ports through its restricted bind capability.
