#!/usr/bin/env bash
set -euo pipefail

# Exercise the exact Bacula 15.0.3 catalog contract without tape hardware. The
# Storage Daemon still uses TapeScale's LTO media type and resource names, but
# its Device Type is File so CI can prove backup, Data verify, initial restore,
# then a second restore after daemon restart with the source unavailable. The
# spanning mode adds GZIP6 and forces that same readback path across exactly two
# 1 MB file-backed volumes. LZO mode proves the other public compression choice
# with an actual write, Data verify, and two source-independent restores.

: "${DATABASE_URL:?DATABASE_URL is required}"
: "${BACULA_CATALOG_HOST:?BACULA_CATALOG_HOST is required}"
: "${BACULA_CATALOG_PORT:?BACULA_CATALOG_PORT is required}"
: "${BACULA_CATALOG_DATABASE:?BACULA_CATALOG_DATABASE is required}"
: "${BACULA_CATALOG_USER:?BACULA_CATALOG_USER is required}"
: "${BACULA_CATALOG_PASSWORD:?BACULA_CATALOG_PASSWORD is required}"

for config_value in \
  "$BACULA_CATALOG_HOST" \
  "$BACULA_CATALOG_PORT" \
  "$BACULA_CATALOG_DATABASE" \
  "$BACULA_CATALOG_USER" \
  "$BACULA_CATALOG_PASSWORD"; do
  if [[ ! "$config_value" =~ ^[A-Za-z0-9._-]+$ ]]; then
    echo "Bacula CI catalog values must use only ASCII letters, digits, dot, underscore, or hyphen" >&2
    exit 2
  fi
done

readonly ci_mode="${BACULA_CI_MODE:-single}"
case "$ci_mode" in
  single)
    run_id="01a1d15f-c6ae-768a-85ff-27b5121d5c05"
    run_token="01a1d15fc6ae768a85ff27b5121d5c05"
    volume_name="AA0344"
    continuation_volume_name=""
    maximum_volumes=1
    maximum_volume_bytes_directive=""
    compression_directive=""
    fixture_name="fixture.txt"
    ;;
  spanning)
    run_id="01a1d15f-c6ae-768a-85ff-27b5121d5c06"
    run_token="01a1d15fc6ae768a85ff27b5121d5c06"
    volume_name="BB0344"
    continuation_volume_name="BB0345"
    maximum_volumes=0
    maximum_volume_bytes_directive="Maximum Volume Bytes = 1M"
    compression_directive="Compression = GZIP6"
    fixture_name="spanning-fixture.bin"
    ;;
  lzo)
    run_id="01a1d15f-c6ae-768a-85ff-27b5121d5c07"
    run_token="01a1d15fc6ae768a85ff27b5121d5c07"
    volume_name="CC0344"
    continuation_volume_name=""
    maximum_volumes=1
    maximum_volume_bytes_directive=""
    compression_directive="Compression = LZO"
    fixture_name="lzo-fixture.bin"
    ;;
  *)
    echo "BACULA_CI_MODE must be single, spanning, or lzo" >&2
    exit 2
    ;;
esac
readonly ci_mode run_id run_token volume_name continuation_volume_name
readonly maximum_volumes maximum_volume_bytes_directive compression_directive fixture_name
readonly pool_name="TS-LTO-${run_token}"
readonly config_root="/etc/tapescale"
readonly work_root="/var/lib/bacula"
readonly runtime_root="/run/bacula"
readonly source_root="/source"
readonly restore_root="/restore"
readonly restore_destination="${restore_root}/${run_token}"
readonly rehearsal_destination="${restore_root}/${run_token}-rehearsal"
readonly log_root="$(mktemp -d /tmp/tapescale-bacula-roundtrip.XXXXXX)"

sd_pid=""
fd_pid=""
director_pid=""

cleanup() {
  local status=$?
  trap - EXIT
  for daemon_pid in "$director_pid" "$fd_pid" "$sd_pid"; do
    if [[ -n "$daemon_pid" ]] && kill -0 "$daemon_pid" 2>/dev/null; then
      kill -TERM "$daemon_pid" 2>/dev/null || true
    fi
  done
  for daemon_pid in "$director_pid" "$fd_pid" "$sd_pid"; do
    if [[ -n "$daemon_pid" ]]; then
      wait "$daemon_pid" 2>/dev/null || true
    fi
  done
  if (( status != 0 )); then
    for daemon_log in "$log_root"/*.log; do
      if [[ -f "$daemon_log" ]]; then
        echo "===== ${daemon_log} =====" >&2
        tail -n 200 "$daemon_log" >&2 || true
      fi
    done
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

install -d -m 0750 \
  "$config_root" \
  "$work_root" \
  "$work_root/bootstrap" \
  "$work_root/file-volumes" \
  "$runtime_root" \
  "$source_root" \
  "$restore_root"

if [[ "$ci_mode" == "spanning" ]]; then
  # This mix proves effective GZIP6 while leaving more than 1 MiB of encoded
  # payload, so the 1 MiB volume boundary still forces exactly two volumes.
  # The non-zero repeated prefix cannot be elided by Bacula's sparse detector.
  dd if=/dev/zero bs=1024 count=1024 status=none \
    | tr '\000' 'A' >"$source_root/$fixture_name"
  dd if=/dev/urandom bs=1024 count=1200 status=none \
    >>"$source_root/$fixture_name"
elif [[ "$ci_mode" == "lzo" ]]; then
  # Repeated non-zero bytes are compressible without looking like sparse-file
  # zero runs. The byte-for-byte restores prove that the packaged FD can decode
  # the LZO payload rather than merely reconstructing skipped sparse extents.
  dd if=/dev/zero bs=1024 count=512 status=none \
    | tr '\000' 'A' >"$source_root/$fixture_name"
else
  printf '%s\n' 'TapeScale exact Bacula catalog roundtrip fixture.' >"$source_root/$fixture_name"
fi

cat >"$config_root/bacula-dir.conf" <<EOF
Director {
  Name = tapescale-dir
  DIRport = 9101
  DirAddress = 127.0.0.1
  QueryFile = "/etc/bacula/scripts/query.sql"
  Working Directory = "$work_root"
  Pid Directory = "$runtime_root"
  Maximum Concurrent Jobs = 1
  Password = "ci-director-password"
  Messages = Daemon
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}

Catalog {
  Name = TS-Catalog
  dbname = "$BACULA_CATALOG_DATABASE"
  dbuser = "$BACULA_CATALOG_USER"
  dbpassword = "$BACULA_CATALOG_PASSWORD"
  dbaddress = "$BACULA_CATALOG_HOST"
  dbport = $BACULA_CATALOG_PORT
}

Client {
  Name = tapescale-fd
  Address = "127.0.0.1"
  FDPort = 9102
  Catalog = TS-Catalog
  Password = "ci-file-daemon-password"
  Auto Prune = no
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}

Storage {
  Name = TS-LTO
  Address = "127.0.0.1"
  SDPort = 9103
  Password = "ci-storage-password"
  Device = TS-LTO-Drive
  Media Type = LTO
  Maximum Concurrent Jobs = 1
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}

Pool {
  Name = $pool_name
  Pool Type = Backup
  Recycle = no
  Auto Prune = no
  Maximum Volumes = $maximum_volumes
  $maximum_volume_bytes_directive
}

FileSet {
  Name = TS-Source
  Include {
    Options {
      Signature = SHA256
      ACL Support = yes
      XATTR Support = yes
      One FS = yes
      Sparse = yes
      $compression_directive
    }
    File = "$source_root"
  }
}

Job {
  Name = TS-Full
  Type = Backup
  Level = Full
  Accurate = yes
  Client = tapescale-fd
  FileSet = TS-Source
  Storage = TS-LTO
  Pool = $pool_name
  Messages = Standard
  Spool Attributes = yes
  Write Bootstrap = "$work_root/bootstrap/%i.bsr"
}

Job {
  Name = TS-VerifyData
  Type = Verify
  Level = Data
  Accurate = yes
  Verify Job = TS-Full
  Client = tapescale-fd
  FileSet = TS-Source
  Storage = TS-LTO
  Pool = $pool_name
  Messages = Standard
}

Job {
  Name = TS-Restore
  Type = Restore
  Client = tapescale-fd
  FileSet = TS-Source
  Storage = TS-LTO
  Pool = $pool_name
  Messages = Standard
  Where = $restore_root
  Replace = Always
}

Console {
  Name = tapescale-adapter
  Password = "ci-console-password"
  Command ACL = run, cancel, status, .status, .api, .jlist, restore, label, mount, unmount, release
  Job ACL = TS-Full, TS-VerifyData, TS-Restore
  Client ACL = tapescale-fd
  Backup Client ACL = tapescale-fd
  Restore Client ACL = tapescale-fd
  Storage ACL = TS-LTO
  Pool ACL = $pool_name
  FileSet ACL = TS-Source
  Catalog ACL = TS-Catalog
  Directory ACL = *all*
  User Id ACL = *all*
  Where ACL = $restore_root, $restore_root/*
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}

Messages {
  Name = Standard
  catalog = all
  console = all, !skipped, !saved
}

Messages {
  Name = Daemon
  console = all, !skipped, !saved
}
EOF

cat >"$config_root/bacula-sd.conf" <<EOF
Storage {
  Name = tapescale-sd
  SDPort = 9103
  SDAddress = 127.0.0.1
  Working Directory = "$work_root"
  Pid Directory = "$runtime_root"
  Plugin Directory = "/usr/lib/bacula"
  Maximum Concurrent Jobs = 1
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}

Director {
  Name = tapescale-dir
  Password = "ci-storage-password"
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}

Device {
  Name = TS-LTO-Drive
  Device Type = File
  Media Type = LTO
  Archive Device = "$work_root/file-volumes"
  Automatic Mount = yes
  Always Open = no
  Removable Media = no
  Random Access = yes
  Label Media = yes
  Maximum Concurrent Jobs = 1
}

Messages {
  Name = Standard
  director = tapescale-dir = all
}
EOF

cat >"$config_root/bacula-fd.conf" <<EOF
Director {
  Name = tapescale-dir
  Password = "ci-file-daemon-password"
  Allowed Backup Directories = $source_root
  Allowed Restore Directories = $restore_root
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}

FileDaemon {
  Name = tapescale-fd
  FDPort = 9102
  FDAddress = 127.0.0.1
  Working Directory = "$work_root"
  Pid Directory = "$runtime_root"
  Plugin Directory = "/usr/lib/bacula"
  Maximum Concurrent Jobs = 1
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}

Messages {
  Name = Standard
  director = tapescale-dir = all, !skipped, !restored, !verified, !saved
}
EOF

cat >"$config_root/bconsole.conf" <<EOF
Director {
  Name = tapescale-dir
  DIRport = 9101
  Address = "127.0.0.1"
  Password = "ci-console-password"
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}

Console {
  Name = tapescale-adapter
  Director = tapescale-dir
  Password = "ci-console-password"
  TLS Enable = no
  TLS Require = yes
  TLS PSK Enable = yes
}
EOF

export PGPASSWORD="$BACULA_CATALOG_PASSWORD"

psql_catalog() {
  psql \
    --host "$BACULA_CATALOG_HOST" \
    --port "$BACULA_CATALOG_PORT" \
    --username "$BACULA_CATALOG_USER" \
    --dbname "$BACULA_CATALOG_DATABASE" \
    --no-psqlrc \
    --tuples-only \
    --no-align \
    "$@"
}

run_console() {
  local command=$1
  printf '%s\nquit\n' "$command" | bconsole -c "$config_root/bconsole.conf"
}

extract_job_id() {
  local output=$1
  local job_id
  job_id="$(sed -n 's/.*Job queued\. JobId=\([0-9][0-9]*\).*/\1/p' <<<"$output" | tail -n 1)"
  if [[ ! "$job_id" =~ ^[1-9][0-9]*$ ]]; then
    echo "Bacula did not return one valid queued JobId" >&2
    echo "$output" >&2
    return 1
  fi
  printf '%s\n' "$job_id"
}

validate_roundtrip_catalog() {
  local backup_job_id=$1
  local verify_job_id=$2
  local restore_job_id=$3
  local -a command=(
    /usr/local/bin/tapescale-worker
    --validate-bacula-roundtrip-catalog
    --catalog-backup-job-id "$backup_job_id"
    --catalog-verify-job-id "$verify_job_id"
    --catalog-restore-job-id "$restore_job_id"
    --catalog-run-id "$run_id"
    --catalog-volume "$volume_name"
  )
  if [[ -n "$continuation_volume_name" ]]; then
    command+=(--catalog-volume "$continuation_volume_name")
  fi
  "${command[@]}"
}

wait_for_job() {
  local job_id=$1
  local description=$2
  local row=""
  for _attempt in $(seq 1 90); do
    row="$(psql_catalog --command "SELECT jobstatus || '|' || joberrors FROM job WHERE jobid = ${job_id}")"
    if [[ "$row" == "T|0" ]]; then
      return 0
    fi
    case "$row" in
      A\|*|D\|*|E\|*|e\|*|f\|*|I\|*|W\|*)
        echo "$description JobId $job_id ended unsuccessfully: $row" >&2
        return 1
        ;;
    esac
    sleep 1
  done
  echo "$description JobId $job_id did not complete in 90 seconds; last state: ${row:-missing}" >&2
  return 1
}

stop_runtime() {
  local daemon_pid
  local running
  for daemon_pid in "$director_pid" "$fd_pid" "$sd_pid"; do
    if [[ -n "$daemon_pid" ]] && kill -0 "$daemon_pid" 2>/dev/null; then
      kill -TERM "$daemon_pid" 2>/dev/null || true
    fi
  done
  for _attempt in $(seq 1 150); do
    running=false
    for daemon_pid in "$director_pid" "$fd_pid" "$sd_pid"; do
      if [[ -n "$daemon_pid" ]] && kill -0 "$daemon_pid" 2>/dev/null; then
        running=true
      fi
    done
    if [[ "$running" == false ]]; then
      break
    fi
    sleep 0.1
  done
  for daemon_pid in "$director_pid" "$fd_pid" "$sd_pid"; do
    if [[ -n "$daemon_pid" ]]; then
      if kill -0 "$daemon_pid" 2>/dev/null; then
        kill -KILL "$daemon_pid" 2>/dev/null || true
      fi
      wait "$daemon_pid" || true
    fi
  done
  director_pid=""
  fd_pid=""
  sd_pid=""
}

start_runtime() {
  local suffix=$1
  bacula-sd -f -P -c "$config_root/bacula-sd.conf" >"$log_root/storage-${suffix}.log" 2>&1 &
  sd_pid=$!
  bacula-fd -f -P -c "$config_root/bacula-fd.conf" >"$log_root/file-${suffix}.log" 2>&1 &
  fd_pid=$!
  bacula-dir -f -P -c "$config_root/bacula-dir.conf" >"$log_root/director-${suffix}.log" 2>&1 &
  director_pid=$!

  local director_ready=false
  for _attempt in $(seq 1 30); do
    if director_status="$(run_console 'status director' 2>&1)" \
      && grep -Fq '1000 OK:' <<<"$director_status"; then
      director_ready=true
      break
    fi
    sleep 1
  done
  if [[ "$director_ready" != true ]]; then
    echo "Bacula Director did not accept the restricted console within 30 seconds" >&2
    return 1
  fi
}

/usr/local/bin/tapescale-worker --prepare-bacula-catalog

bacula-sd -t -c "$config_root/bacula-sd.conf"
bacula-fd -t -c "$config_root/bacula-fd.conf"
bacula-dir -t -c "$config_root/bacula-dir.conf"

start_runtime initial

label_volume() {
  local requested_volume=$1
  local label_output
  label_output="$(run_console "label storage=TS-LTO pool=${pool_name} volume=${requested_volume} yes")"
  if ! grep -Fq 'successfully created' <<<"$label_output"; then
    echo "Bacula did not confirm the ${requested_volume} test-volume catalog record" >&2
    echo "$label_output" >&2
    return 1
  fi
}

label_volume "$volume_name"

wait_for_spanning_backup() {
  local job_id=$1
  local row=""
  local continuation_labeled=false
  for _attempt in $(seq 1 120); do
    row="$(psql_catalog --command "SELECT jobstatus || '|' || joberrors FROM job WHERE jobid = ${job_id}")"
    if [[ "$row" == "T|0" ]]; then
      if [[ "$continuation_labeled" != true ]]; then
        echo "Spanning fixture completed without requesting its second volume" >&2
        return 1
      fi
      return 0
    fi
    case "$row" in
      M\|*)
        run_console 'mount storage=TS-LTO' >/dev/null
        ;;
      m\|*)
        if [[ "$continuation_labeled" != true ]]; then
          label_volume "$continuation_volume_name"
          run_console 'mount storage=TS-LTO' >/dev/null
          continuation_labeled=true
        fi
        ;;
      A\|*|D\|*|E\|*|e\|*|f\|*|I\|*|W\|*)
        echo "Spanning Full backup JobId $job_id ended unsuccessfully: $row" >&2
        return 1
        ;;
    esac
    sleep 1
  done
  echo "Spanning Full backup JobId $job_id did not complete in 120 seconds; last state: ${row:-missing}" >&2
  return 1
}

backup_output="$(run_console 'run job=TS-Full level=Full yes')"
backup_job_id="$(extract_job_id "$backup_output")"
if [[ "$ci_mode" == "spanning" ]]; then
  wait_for_spanning_backup "$backup_job_id"
else
  wait_for_job "$backup_job_id" "Full backup"
fi

if [[ "$ci_mode" == "spanning" ]]; then
  spanning_shape="$(psql_catalog --command "
    WITH used AS (
      SELECT m.volumename, min(jm.volindex) AS first_volume_index
        FROM jobmedia jm
        JOIN media m ON m.mediaid = jm.mediaid
       WHERE jm.jobid = ${backup_job_id}
       GROUP BY m.volumename
    )
    SELECT count(*) || '|' ||
           string_agg(volumename, ',' ORDER BY first_volume_index)
      FROM used")"
  test "$spanning_shape" = "2|${volume_name},${continuation_volume_name}"
  volume_shape="$(psql_catalog --command "
    SELECT p.maxvols || '|' || count(*) || '|' ||
           bool_and(m.maxvolbytes > 0)::text
      FROM pool p
      JOIN media m ON m.poolid = p.poolid
     WHERE p.name = '${pool_name}'
     GROUP BY p.maxvols")"
  test "$volume_shape" = "0|2|true"
fi

if [[ -n "$compression_directive" ]]; then
  compression_proven="$(psql_catalog --command "
    SELECT (
      j.jobstatus = 'T'
      AND j.joberrors = 0
      AND j.readbytes > 0
      AND j.compressratio >= 0.5
      AND EXISTS (
        SELECT 1 FROM log l
         WHERE l.jobid = j.jobid
           AND l.logtext ~ 'Software Compression:[[:space:]]+[0-9]+[.][0-9]+%[[:space:]]+[0-9]+[.][0-9]+:1'
      )
    )::text
      FROM job j
     WHERE j.jobid = ${backup_job_id}")"
  test "$compression_proven" = "true"
fi

verify_output="$(run_console "run job=TS-VerifyData level=Data accurate=yes jobid=${backup_job_id} yes")"
verify_job_id="$(extract_job_id "$verify_output")"
wait_for_job "$verify_job_id" "Data verify"

restore_output="$(run_console "restore jobid=${backup_job_id} restorejob=TS-Restore restoreclient=tapescale-fd where=\"${restore_destination}\" all done yes")"
restore_job_id="$(extract_job_id "$restore_output")"
wait_for_job "$restore_job_id" "Restore"

cmp "$source_root/$fixture_name" "$restore_destination/source/$fixture_name"

verify_shape="$(psql_catalog --command "
  SELECT j.name || '|' || j.type || '|' || j.level || '|' || j.jobstatus || '|' ||
         j.priorjobid || '|' || j.poolid || '|' || COALESCE(p.name, '<missing>') || '|' ||
         COALESCE(rs.name, '<missing>') || '|' || j.joberrors
  FROM job j
  LEFT JOIN pool p ON p.poolid = j.poolid
  LEFT JOIN storage rs ON rs.storageid = j.lastreadstorageid
  WHERE j.jobid = ${verify_job_id}")"
test "$verify_shape" = "TS-VerifyData|V|A|T|${backup_job_id}|0|<missing>|TS-LTO|0"

restore_shape="$(psql_catalog --command "
  SELECT j.name || '|' || j.type || '|' || j.level || '|' || j.jobstatus || '|' ||
         j.priorjobid || '|' || j.poolid || '|' || COALESCE(p.name, '<missing>') || '|' ||
         COALESCE(rs.name, '<missing>') || '|' || j.joberrors
  FROM job j
  LEFT JOIN pool p ON p.poolid = j.poolid
  LEFT JOIN storage rs ON rs.storageid = j.lastreadstorageid
  WHERE j.jobid = ${restore_job_id}")"
test "$restore_shape" = "TS-Restore|R|F|T|0|0|<missing>|TS-LTO|0"

validate_roundtrip_catalog "$backup_job_id" "$verify_job_id" "$restore_job_id"

# A verified point must remain restorable after the engine processes restart and
# without consulting the original source. This is the deterministic software
# equivalent of the attended Recovery Vault rehearsal; physical SAS/LTO proof
# remains a separate hardware acceptance result.
stop_runtime
mv "$source_root" "${source_root}.unavailable"
install -d -m 0750 "$source_root"
test ! -e "$source_root/$fixture_name"
start_runtime rehearsal

rehearsal_output="$(run_console "restore jobid=${backup_job_id} restorejob=TS-Restore restoreclient=tapescale-fd where=\"${rehearsal_destination}\" all done yes")"
rehearsal_job_id="$(extract_job_id "$rehearsal_output")"
wait_for_job "$rehearsal_job_id" "Restarted-runtime restore rehearsal"
cmp "${source_root}.unavailable/$fixture_name" "$rehearsal_destination/source/$fixture_name"

validate_roundtrip_catalog "$backup_job_id" "$verify_job_id" "$rehearsal_job_id"

echo "Bacula ${ci_mode} file-backed recovery path passed: backup=${backup_job_id} verify=${verify_job_id} initial_restore=${restore_job_id} restarted_restore=${rehearsal_job_id}"
