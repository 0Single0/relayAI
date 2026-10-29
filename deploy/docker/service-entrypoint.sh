#!/bin/sh
set -eu

secrets_dir=${RELAYAI_SECRETS_DIR:-/run/relayai-secrets}
for name in postgres_password master_key internal_token; do
  if [ ! -s "$secrets_dir/$name" ]; then
    echo "Missing secret: $name. Run docker compose up -d to initialize the deployment." >&2
    exit 1
  fi
done

postgres_password=$(cat "$secrets_dir/postgres_password")
export DATABASE_URL="postgresql://relayai:${postgres_password}@postgres:5432/relayai?sslmode=disable"
RELAYAI_MASTER_KEY=$(cat "$secrets_dir/master_key")
GATEWAY_INTERNAL_TOKEN=$(cat "$secrets_dir/internal_token")
export RELAYAI_MASTER_KEY GATEWAY_INTERNAL_TOKEN

exec "$@"
