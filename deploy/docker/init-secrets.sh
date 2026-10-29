#!/bin/sh
set -eu
umask 077

data_dir=${RELAYAI_DATA_DIR:-/data}
secrets_dir="$data_dir/secrets"
mkdir -p "$secrets_dir"

# An existing database must keep its original passwords and encryption key.
# Refuse to silently replace missing secrets and make saved credentials unusable.
if [ -f "$data_dir/postgres/PG_VERSION" ]; then
  for name in postgres_password master_key internal_token; do
    if [ ! -s "$secrets_dir/$name" ]; then
      echo "Missing $name for an existing database. Restore data/secrets from backup." >&2
      exit 1
    fi
  done
fi

generate_secret() {
  name=$1
  format=$2
  secret_file="$secrets_dir/$name"
  if [ -e "$secret_file" ]; then
    if [ ! -s "$secret_file" ]; then
      echo "Secret file is empty: $name. Restore it or remove it before first initialization." >&2
      exit 1
    fi
    return
  fi
  openssl rand "-$format" 32 > "$secret_file.tmp"
  mv "$secret_file.tmp" "$secret_file"
}

generate_secret postgres_password hex
generate_secret master_key base64
generate_secret internal_token hex
echo "relayAI secrets are ready (existing files preserved)."
