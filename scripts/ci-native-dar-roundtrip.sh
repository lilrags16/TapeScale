#!/usr/bin/env bash
set -euo pipefail

# Exercise the exact separately packaged DAR codec without claiming physical
# tape qualification. This is the same stage-first shape used by the native
# worker: create large numbered slices, create an isolated catalogue, test the
# completed archive, then extract it with the original source unavailable.

readonly compression="${NATIVE_CI_COMPRESSION:-zstd}"
case "$compression" in
  zstd) readonly compression_argument="zstd:1:1M" ;;
  lz4) readonly compression_argument="lz4:1:1M" ;;
  gzip) readonly compression_argument="gzip:6:1M" ;;
  *)
    echo "NATIVE_CI_COMPRESSION must be zstd, lz4, or gzip" >&2
    exit 2
    ;;
esac

readonly work_root="$(mktemp -d /tmp/tapescale-native-roundtrip.XXXXXX)"
readonly source_root="$work_root/source"
readonly stage_root="$work_root/stage"
readonly restore_root="$work_root/restore"
readonly held_source="$work_root/source-unavailable"

cleanup() {
  local status=$?
  if (( status != 0 )); then
    find "$work_root" -maxdepth 3 -printf '%y %m %s %p -> %l\n' >&2 || true
  fi
  rm -rf -- "$work_root"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

command -v dar >/dev/null
test "$(dar -V 2>&1 | sed -n 's/^[[:space:]]*dar version \([^,[:space:]]*\).*/\1/p' | head -1)" = "2.8.6"

install -d -m 0750 "$source_root/tree/nested" "$source_root/small" "$stage_root" "$restore_root"
printf '%s\n' 'TapeScale native DAR fixture.' >"$source_root/tree/nested/content.txt"
printf '%s\n' 'hard-link payload' >"$source_root/tree/hard-link-origin.bin"
ln "$source_root/tree/hard-link-origin.bin" "$source_root/tree/hard-link-peer.bin"
ln -s 'nested/content.txt' "$source_root/tree/content-link"
chmod 0640 "$source_root/tree/nested/content.txt"
chmod 0750 "$source_root/tree/nested"
touch -t 202611190101.02 "$source_root/tree/nested/content.txt"

# Small-file packaging is the reason TapeScale stages DAR archives. A
# deterministic set catches accidental per-file shortcuts without adding much
# CI runtime.
for ordinal in $(seq 1 512); do
  printf 'native-small-file-%04d\n' "$ordinal" \
    >"$source_root/small/file-$(printf '%04d' "$ordinal").txt"
done

# DAR should retain this as a sparse regular file rather than materializing a
# large zero-filled payload.
truncate -s 67108864 "$source_root/tree/sparse.bin"
printf 'tail' | dd of="$source_root/tree/sparse.bin" bs=1 seek=67108860 conv=notrunc status=none

dar -N -Q -w \
  -c "$stage_root/archive" \
  -R "$source_root" \
  -s 67108864 \
  --min-digits 6 \
  --on-fly-isolate "$stage_root/catalog" \
  --compression="$compression_argument" \
  --multi-thread 2

test -s "$stage_root/archive.000001.dar"
test -s "$stage_root/catalog.1.dar"
dar -N -Q -t "$stage_root/archive" \
  --sequential-read \
  --min-digits 6 \
  --multi-thread 2

# Prove extraction is source-independent: the source tree is renamed before
# DAR is asked to reconstruct the archive.
mv "$source_root" "$held_source"
dar -N -Q -w \
  -x "$stage_root/archive" \
  -R "$restore_root" \
  --min-digits 6 \
  --multi-thread 2

test "$(find "$restore_root/small" -type f | wc -l)" = "512"
test "$(sha256sum "$held_source/tree/nested/content.txt" | cut -d' ' -f1)" = \
     "$(sha256sum "$restore_root/tree/nested/content.txt" | cut -d' ' -f1)"
test "$(stat -c '%a' "$restore_root/tree/nested/content.txt")" = "640"
test "$(stat -c '%a' "$restore_root/tree/nested")" = "750"
test "$(stat -c '%Y' "$restore_root/tree/nested/content.txt")" = \
     "$(stat -c '%Y' "$held_source/tree/nested/content.txt")"
test "$(readlink "$restore_root/tree/content-link")" = "nested/content.txt"
test "$(stat -c '%i' "$restore_root/tree/hard-link-origin.bin")" = \
     "$(stat -c '%i' "$restore_root/tree/hard-link-peer.bin")"
test "$(stat -c '%s' "$restore_root/tree/sparse.bin")" = "67108864"
test "$(tail -c 4 "$restore_root/tree/sparse.bin")" = "tail"

# Compare all regular-file content after excluding inode numbers and directory
# ordering from the assertion.
(
  cd "$held_source"
  find . -type f -print0 | sort -z | xargs -0 sha256sum
) >"$work_root/source.sha256"
(
  cd "$restore_root"
  find . -type f -print0 | sort -z | xargs -0 sha256sum
) >"$work_root/restore.sha256"
cmp "$work_root/source.sha256" "$work_root/restore.sha256"

printf 'TapeScale native DAR %s roundtrip passed.\n' "$compression"
