#!/bin/sh
# Run from the relayAI Compose deployment directory. Preview is the default.
set -eu
umask 077
mode=${1:---preview}
case "$mode" in --preview|--apply) ;; *) echo 'Usage: sh run-import.sh [--preview|--apply]' >&2; exit 1 ;; esac
bundle_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
container=$(docker compose ps -q backend)
if [ -z "$container" ]; then echo 'Run this from the running relayAI Compose deployment directory.' >&2; exit 1; fi
case "$(docker exec "$container" uname -m)" in
  x86_64) arch=amd64 ;; aarch64|arm64) arch=arm64 ;;
  *) echo 'Unsupported server architecture.' >&2; exit 1 ;;
esac
binary="$bundle_dir/import-legacy-linux-$arch"
if [ ! -f "$binary" ]; then echo 'Import binary is missing.' >&2; exit 1; fi
container_dir=$(docker exec "$container" mktemp -d /tmp/relayai-legacy.XXXXXXXX)
cleanup() { docker exec "$container" rm -rf "$container_dir" >/dev/null 2>&1 || true; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
docker cp "$binary" "$container:$container_dir/import-legacy" >/dev/null
docker cp "$bundle_dir/accounts.json" "$container:$container_dir/accounts.json" >/dev/null
docker cp "$bundle_dir/gateway_master-key" "$container:$container_dir/gateway_master-key" >/dev/null
docker exec "$container" chmod 700 "$container_dir/import-legacy"
docker exec "$container" chmod 600 "$container_dir/accounts.json" "$container_dir/gateway_master-key"
run_import() {
  docker exec "$container" /bin/sh /app/docker/service-entrypoint.sh \
    "$container_dir/import-legacy" -bundle "$container_dir/accounts.json" \
    -source-key-file "$container_dir/gateway_master-key" "$@"
}
run_import
if [ "$mode" = '--preview' ]; then
  echo 'Preview complete. Database unchanged. Run again with --apply to import.'
  exit 0
fi
backup_dir="$PWD/data/legacy-import-backups/$(date -u +%Y%m%dT%H%M%SZ)-$$"
mkdir -p "$backup_dir"
if ! docker compose exec -T postgres pg_dump -U relayai -d relayai -Fc > "$backup_dir/relayai-before-import.dump"; then
  echo 'Database backup failed. Import was not started.' >&2
  exit 1
fi
if [ ! -s "$backup_dir/relayai-before-import.dump" ]; then echo 'Empty backup. Import was not started.' >&2; exit 1; fi
echo "Database backup: $backup_dir/relayai-before-import.dump"
run_import -apply
if ! docker exec "$container" /bin/sh /app/docker/service-entrypoint.sh /bin/sh -c \
  'curl --fail --silent --show-error --max-time 60 -o /dev/null -X POST -H "X-Internal-Token: $GATEWAY_INTERNAL_TOKEN" "${GATEWAY_BASE_URL:-http://gateway:18788}/internal/state/reload"'; then
  echo 'Accounts committed, but gateway reload failed. Retry by running: docker compose restart gateway' >&2
  exit 1
fi
echo 'Accounts committed and gateway reloaded. Existing accounts were preserved.'
