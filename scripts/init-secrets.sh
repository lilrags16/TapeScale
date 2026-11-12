#!/bin/sh
set -eu

secret_dir=${1:-secrets}
secret_file="$secret_dir/postgres_password.txt"

umask 077
mkdir -p "$secret_dir"
if [ -e "$secret_file" ]; then
    echo "Refusing to replace existing secret: $secret_file" >&2
    exit 1
fi

if command -v openssl >/dev/null 2>&1; then
    openssl rand -base64 36 > "$secret_file"
elif command -v od >/dev/null 2>&1; then
    od -An -N36 -tx1 /dev/urandom | tr -d ' \n' > "$secret_file"
    printf '\n' >> "$secret_file"
else
    echo "OpenSSL or od is required to generate a secret." >&2
    exit 1
fi
chmod 600 "$secret_file"
echo "Created $secret_file with mode 0600."
