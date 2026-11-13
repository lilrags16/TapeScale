#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
cd "$repo_root"

fail() {
  echo "release metadata check failed: $*" >&2
  exit 1
}

require_text() {
  local file=$1
  local expected=$2
  if ! grep -Fq -- "$expected" "$file"; then
    fail "$file does not contain: $expected"
  fi
}

package_version="$(awk '
  /^\[package\]$/ { in_package = 1; next }
  /^\[/ && in_package { exit }
  in_package && /^version = "/ {
    value = $0
    sub(/^version = "/, "", value)
    sub(/"$/, "", value)
    print value
    exit
  }
' Cargo.toml)"
[[ -n "$package_version" ]] || fail "Cargo.toml package version is missing"

lock_version="$(awk '
  /^\[\[package\]\]$/ { tapescale = 0; next }
  $0 == "name = \"tapescale\"" { tapescale = 1; next }
  tapescale && /^version = "/ {
    value = $0
    sub(/^version = "/, "", value)
    sub(/"$/, "", value)
    print value
    exit
  }
' Cargo.lock)"
[[ "$lock_version" == "$package_version" ]] \
  || fail "Cargo.lock reports ${lock_version:-<missing>}, expected $package_version"

release_tag="v${package_version}"

require_text Dockerfile "ARG TAPESCALE_VERSION=${package_version}"
require_text Dockerfile 'org.opencontainers.image.version="${TAPESCALE_VERSION}"'
require_text Dockerfile 'org.opencontainers.image.revision="${TAPESCALE_REVISION}"'
require_text .env.example "TAPESCALE_VERSION=${package_version}"
require_text compose.yaml 'TAPESCALE_VERSION: ${TAPESCALE_VERSION:-'"${package_version}"'}'
require_text compose.yaml 'image: tapescale:${TAPESCALE_VERSION:-'"${package_version}"'}'
require_text compose.truenas.yaml "context: https://github.com/lilrags16/TapeScale.git#${release_tag}"
require_text compose.truenas.yaml "TAPESCALE_VERSION: ${package_version}"
require_text compose.truenas.yaml "TAPESCALE_REVISION: ${release_tag}"
require_text compose.truenas.yaml "image: tapescale:${package_version}"
require_text .github/workflows/ci.yml 'tags: ["v*"]'
require_text .github/workflows/ci.yml 'run: scripts/check-release-consistency.sh'

recovery_version="$(awk '
  /"component": "tapescale-recovery"/ { recovery = 1; next }
  recovery && /"version":/ {
    value = $0
    sub(/^[^:]*:[[:space:]]*"/, "", value)
    sub(/".*/, "", value)
    print value
    exit
  }
' fixtures/recovery-bundle/manifest.json)"
[[ "$recovery_version" == "$package_version" ]] \
  || fail "recovery fixture reports ${recovery_version:-<missing>}, expected $package_version"

if [[ "${GITHUB_REF_TYPE:-}" == "tag" ]]; then
  [[ -n "${GITHUB_REF_NAME:-}" ]] || fail "GITHUB_REF_NAME is missing for a tag build"
  [[ "$GITHUB_REF_NAME" == "$release_tag" ]] \
    || fail "tag build is $GITHUB_REF_NAME, expected $release_tag"
fi

echo "Release metadata is consistent for ${package_version} (${release_tag})."
