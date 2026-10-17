#!/usr/bin/env bash
set -Eeuo pipefail

stage=${1:?staging directory required}
stamp=${2:?deployment timestamp required}
core_sha=${3:?Core SHA-256 required}
proxy_sha=${4:?Proxy SHA-256 required}
operations_source=${5:-}
case "$stage" in /root/hydra-staging/incoming-*) ;; *) echo "invalid staging directory" >&2; exit 2 ;; esac
case "$stamp" in (*[!0-9TZ]*|'') echo "invalid timestamp" >&2; exit 2 ;; esac
if [[ -n "$operations_source" && "$operations_source" != "$stage/operations.json" ]]; then
  echo "operations config must be staged with binaries" >&2; exit 2
fi
backup="/root/hydra-staging/backup-${stamp}"
database=/var/lib/hydra-core/hydra.sqlite3
# Follow the effective service unit, including competition drop-ins. This keeps
# backups and rollback on the exact config path consumed by Core.
operations=$(python3 - <<'PYCONFIG'
import re, subprocess
unit = subprocess.check_output(['systemctl', 'show', 'hydra-core', '--property=ExecStart', '--value'], text=True)
match = re.search(r'--operations[ =]+(/[^\s;"\x27]+)', unit)
if not match:
    raise SystemExit('Cannot determine effective Core --operations path')
print(match.group(1))
PYCONFIG
)
mkdir -m 0700 "$backup"
printf '%s  %s\n' "$core_sha" "$stage/hydra-core" "$proxy_sha" "$stage/hydra-proxy" | sha256sum -c -
cp -p /usr/local/bin/hydra-core "$backup/hydra-core"
cp -p /usr/local/bin/hydra-proxy "$backup/hydra-proxy"
# Stop the sole SQLite writer before backing up or replacing state. Proxy keeps
# forwarding while Core is down. No database/secret contents leave this VM.
systemctl stop hydra-core
trap 'systemctl start hydra-core || true' ERR
cp -p "$operations" "$backup/operations.json"
python3 - "$database" "$backup/hydra.sqlite3" <<'PY'
import os, sqlite3, sys
source = sqlite3.connect(sys.argv[1])
target = sqlite3.connect(sys.argv[2])
source.backup(target)
target.close()
source.close()
os.chmod(sys.argv[2], 0o600)
PY
install -m 0755 "$stage/hydra-core" /usr/local/bin/hydra-core.new
install -m 0755 "$stage/hydra-proxy" /usr/local/bin/hydra-proxy.new
mv -f /usr/local/bin/hydra-core.new /usr/local/bin/hydra-core
mv -f /usr/local/bin/hydra-proxy.new /usr/local/bin/hydra-proxy
if [[ -n "$operations_source" ]]; then
  python3 - "$operations_source" <<'PY'
import json, sys
with open(sys.argv[1]) as source:
    assert isinstance(json.load(source), dict)
PY
  cp "$operations_source" "$operations.new"
  chmod 0600 "$operations.new"
  chown --reference="$operations" "$operations.new"
  mv -f "$operations.new" "$operations"
fi
healthy() {
  for ((attempt=0;attempt<50;attempt++)); do
    if systemctl is-active --quiet hydra-core && systemctl is-active --quiet hydra-proxy &&
       curl --silent --fail --insecure https://127.0.0.1:8080/ >/dev/null; then return 0; fi
    sleep 0.2
  done
  return 1
}
if ! systemctl restart hydra-core || ! systemctl restart hydra-proxy || ! healthy; then
  systemctl stop hydra-core || true
  install -m 0755 "$backup/hydra-core" /usr/local/bin/hydra-core
  install -m 0755 "$backup/hydra-proxy" /usr/local/bin/hydra-proxy
  cp -p "$backup/operations.json" "$operations"
  cp "$backup/hydra.sqlite3" "$database.restore"
  chmod 0600 "$database.restore"
  chown --reference="$database" "$database.restore"
  rm -f "$database-wal" "$database-shm"
  mv -f "$database.restore" "$database"
  systemctl restart hydra-core || true
  systemctl restart hydra-proxy || true
  echo "Hydra deployment failed; prior binaries, config and database restored from $backup" >&2
  exit 1
fi
trap - ERR
printf 'core_sha256=%s\nproxy_sha256=%s\nbackup=%s\n' "$core_sha" "$proxy_sha" "$backup"
systemctl --no-pager --full status hydra-core hydra-proxy | sed -n '1,34p'
