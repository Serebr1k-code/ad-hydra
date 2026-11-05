#!/usr/bin/env bash
# Replace Core while Proxy continues forwarding. Run as root on the target VM.
set -Eeuo pipefail
stage=${1:?staging directory required}
stamp=${2:?deployment timestamp required}
core_sha=${3:?Core SHA-256 required}
case "$stage" in /root/hydra-staging/incoming-*) ;; *) exit 2 ;; esac
case "$stamp" in (*[!0-9TZ]*|'') exit 2 ;; esac
[[ "$core_sha" =~ ^[0-9a-f]{64}$ ]] || exit 2
backup="/root/hydra-staging/core-backup-${stamp}"
database=$(python3 - <<'PY'
import re, subprocess
unit = subprocess.check_output(['systemctl','show','hydra-core','--property=ExecStart','--value'],text=True)
match = re.search(r'--database[ =]+(/[^\s;"\x27]+)',unit)
if not match:
    raise SystemExit('Cannot determine effective Core database path')
print(match.group(1))
PY
)
printf '%s  %s\n' "$core_sha" "$stage/hydra-core" | sha256sum -c -
mkdir -m 0700 "$backup"
cp -p /usr/local/bin/hydra-core "$backup/hydra-core"
database_backed_up=0
rollback() {
  trap - ERR
  set +e
  if ! systemctl stop hydra-core; then
    echo "Rollback could not stop Core; backup remains at $backup" >&2
    exit 1
  fi
  if ! install -m 0755 "$backup/hydra-core" /usr/local/bin/hydra-core.rollback ||
     ! mv -f /usr/local/bin/hydra-core.rollback /usr/local/bin/hydra-core; then
    echo "Rollback could not restore Core; backup remains at $backup" >&2
    exit 1
  fi
  if [[ "$database_backed_up" == 1 ]]; then
    if ! cp "$backup/hydra.sqlite3" "$database.restore" ||
       ! chmod 0600 "$database.restore" ||
       ! chown --reference="$database" "$database.restore" ||
       ! cmp -s "$backup/hydra.sqlite3" "$database.restore"; then
      echo "Database restore staging failed; original database/WAL preserved and Core stopped. Backup: $backup" >&2
      exit 1
    fi
    if ! sync -f "$database.restore" ||
       ! rm -f "$database-wal" "$database-shm" ||
       ! mv -f "$database.restore" "$database"; then
      echo "Database restore failed; Core remains stopped. Backup: $backup" >&2
      exit 1
    fi
  fi
  if ! systemctl start hydra-core; then
    echo "Backup restored but Core did not start: $backup" >&2
    exit 1
  fi
  echo "Core deployment failed; restored backup $backup" >&2
  exit 1
}
trap rollback ERR
systemctl stop hydra-core
python3 - "$database" "$backup/hydra.sqlite3" <<'PY'
import os,sqlite3,sys
with sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True) as source:
    with sqlite3.connect(sys.argv[2]) as destination:
        source.backup(destination)
os.chmod(sys.argv[2],0o600)
PY
database_backed_up=1
install -m 0755 "$stage/hydra-core" /usr/local/bin/hydra-core.new
mv -f /usr/local/bin/hydra-core.new /usr/local/bin/hydra-core
systemctl start hydra-core
healthy=0
# Allow the bounded decoded-traffic compression migration to finish.
health_deadline=$((SECONDS + 45))
while ((SECONDS < health_deadline)); do
  if systemctl is-active --quiet hydra-core && curl --silent --fail --insecure --connect-timeout 1 --max-time 2 https://127.0.0.1:8080/ >/dev/null; then
    healthy=1
    break
  fi
  sleep 0.2
done
[[ "$healthy" == 1 ]]
trap - ERR
printf 'core_sha256=%s\nbackup=%s\n' "$core_sha" "$backup"
systemctl is-active hydra-core hydra-proxy
