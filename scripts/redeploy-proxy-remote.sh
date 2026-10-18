#!/usr/bin/env bash
# Proxy-only upgrade: preserve Core, SQLite, configuration and decoder versions.
set -Eeuo pipefail
stage=${1:?staging directory required}
stamp=${2:?deployment timestamp required}
proxy_sha=${3:?Proxy SHA-256 required}
case "$stage" in /root/hydra-staging/incoming-*) ;; *) exit 2 ;; esac
case "$stamp" in (*[!0-9TZ]*|'') exit 2 ;; esac
[[ "$proxy_sha" =~ ^[[:xdigit:]]{64}$ ]] || exit 2
backup="/root/hydra-staging/proxy-backup-${stamp}"
socket=$(python3 - <<'PY'
import json
with open('/etc/hydra/proxy.json') as source:
    print(json.load(source)['socket'])
PY
)
core_user=$(systemctl show hydra-core -p User --value)
core_user=${core_user:-root}
status() {
  timeout 3 runuser -u "$core_user" -- /usr/local/bin/hydra-core --socket "$socket" status
}
mkdir -m 0700 "$backup"
printf '%s  %s\n' "$proxy_sha" "$stage/hydra-proxy" | sha256sum -c -
cp -p /usr/local/bin/hydra-proxy "$backup/hydra-proxy"
status > "$backup/status-before.json"
rollback() {
  trap - ERR
  install -m 0755 "$backup/hydra-proxy" /usr/local/bin/hydra-proxy.rollback
  mv -f /usr/local/bin/hydra-proxy.rollback /usr/local/bin/hydra-proxy
  systemctl restart hydra-proxy || true
  echo "Proxy upgrade failed; restored $backup/hydra-proxy" >&2
  exit 1
}
trap rollback ERR
install -m 0755 "$stage/hydra-proxy" /usr/local/bin/hydra-proxy.new
mv -f /usr/local/bin/hydra-proxy.new /usr/local/bin/hydra-proxy
systemctl restart hydra-proxy
healthy=false
for ((attempt=0;attempt<60;attempt++)); do
  if systemctl is-active --quiet hydra-proxy && status > "$backup/status-after.json" 2>/dev/null; then
    healthy=true
    break
  fi
  sleep 0.5
done
if [[ "$healthy" != true ]]; then rollback; fi
trap - ERR
printf 'proxy_sha256=%s\nbackup=%s\n' "$proxy_sha" "$backup"
cat "$backup/status-after.json"
