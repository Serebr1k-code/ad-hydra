#!/usr/bin/env bash
set -Eeuo pipefail

stage=${1:?staging directory required}
stamp=${2:?deployment timestamp required}
core_sha=${3:?Core SHA-256 required}
proxy_sha=${4:?Proxy SHA-256 required}
case "$stage" in /root/hydra-staging/incoming-*) ;; *) echo "invalid staging directory" >&2; exit 2 ;; esac
case "$stamp" in (*[!0-9TZ]*|'') echo "invalid timestamp" >&2; exit 2 ;; esac
backup="/root/hydra-staging/backup-${stamp}"
mkdir -m 0700 "$backup"
printf '%s  %s\n' "$core_sha" "$stage/hydra-core" "$proxy_sha" "$stage/hydra-proxy" | sha256sum -c -
cp -p /usr/local/bin/hydra-core "$backup/hydra-core"
cp -p /usr/local/bin/hydra-proxy "$backup/hydra-proxy"
install -m 0755 "$stage/hydra-core" /usr/local/bin/hydra-core.new
install -m 0755 "$stage/hydra-proxy" /usr/local/bin/hydra-proxy.new
mv -f /usr/local/bin/hydra-core.new /usr/local/bin/hydra-core
mv -f /usr/local/bin/hydra-proxy.new /usr/local/bin/hydra-proxy
if ! systemctl restart hydra-core || ! systemctl restart hydra-proxy || \
   ! systemctl is-active --quiet hydra-core || ! systemctl is-active --quiet hydra-proxy; then
  install -m 0755 "$backup/hydra-core" /usr/local/bin/hydra-core
  install -m 0755 "$backup/hydra-proxy" /usr/local/bin/hydra-proxy
  systemctl restart hydra-core || true
  systemctl restart hydra-proxy || true
  echo "Hydra deployment failed; prior binaries restored from $backup" >&2
  exit 1
fi
printf 'core_sha256=%s\nproxy_sha256=%s\nbackup=%s\n' "$core_sha" "$proxy_sha" "$backup"
systemctl --no-pager --full status hydra-core hydra-proxy | sed -n '1,34p'
