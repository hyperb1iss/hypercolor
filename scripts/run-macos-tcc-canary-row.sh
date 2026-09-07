#!/usr/bin/env bash
set -euo pipefail
umask 077

die() {
  printf 'macOS TCC canary failed: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: scripts/run-macos-tcc-canary-row.sh [options]

Required:
  --request PATH                 Validated row request JSON
  --daemon PATH                  Signed daemon built with macos-tcc-canary
  --witness-dir PATH             Manual witness JSON and evidence directory
  --topology NAME                app-sidecar, direct-launchd, homebrew, standalone
  --execute-protected-actions    Allow TCC requests, picker UI, and launcher mutation

Topology-specific:
  --app PATH                     Hypercolor app executable for app-sidecar
  --cli PATH                     hypercolor CLI executable for direct-launchd
  --brew PATH                    brew executable for homebrew

Optional:
  --timeout-seconds N            Receipt deadline with 30s operation headroom
  -h, --help                     Print this help

The driver uses the production launcher for one signed acceptance row. It may
request TCC access, present Apple's picker, restart the selected service, or
relaunch Hypercolor. It never resets TCC. Fresh-database, prompt, System
Settings, and process-replacement observations must be supplied as separately
hashed witness artifacts beside the receipt.
EOF
}

request=""
daemon=""
data_dir="${HOME:?HOME must be set}/Library/Application Support/hypercolor"
witness_dir=""
topology=""
app=""
cli=""
brew=""
timeout_seconds=""
execute=false
armed_request=""
temporary_files=()
installed_row_artifacts=()
row_committed=false

cleanup_canary_artifacts() {
  if [[ -n "${armed_request}" && -f "${armed_request}" ]]; then
    /bin/rm -f -- "${armed_request}"
  fi
  for temporary_file in "${temporary_files[@]-}"; do
    if [[ -n "${temporary_file}" && -f "${temporary_file}" ]]; then
      /bin/rm -f -- "${temporary_file}"
    fi
  done
  if [[ "${row_committed}" != true ]]; then
    for installed_artifact in "${installed_row_artifacts[@]-}"; do
      if [[ -n "${installed_artifact}" && -f "${installed_artifact}" ]]; then
        /bin/rm -f -- "${installed_artifact}"
      fi
    done
  fi
}

trap cleanup_canary_artifacts EXIT

pid_is_alive() {
  kill -0 "$1" 2>/dev/null
}

process_fingerprint() {
  local pid="$1"
  local identity
  identity="$(/bin/ps -p "${pid}" -o lstart= -o command= | awk '{$1=$1; print}')" \
    || return 1
  [[ -n "${identity}" ]] || return 1
  printf '%s' "${identity}" | /usr/bin/shasum -a 256 | awk '{print $1}'
}

wait_for_pid_exit() {
  local pid="$1"
  local expected_fingerprint="$2"
  local timeout="${3:-10}"
  local deadline=$((SECONDS + timeout))
  local current_fingerprint
  while pid_is_alive "${pid}" && ((SECONDS < deadline)); do
    current_fingerprint="$(process_fingerprint "${pid}")" || return 0
    [[ "${current_fingerprint}" == "${expected_fingerprint}" ]] || return 0
    sleep 1
  done
  if pid_is_alive "${pid}"; then
    current_fingerprint="$(process_fingerprint "${pid}")" || return 0
    [[ "${current_fingerprint}" != "${expected_fingerprint}" ]] \
      || die "predecessor process ${pid} did not exit within ${timeout} seconds"
  fi
}

identifier_is_safe() {
  [[ "$1" =~ ^[A-Za-z0-9_.-]{1,128}$ && "$1" != "." && "$1" != ".." ]]
}

install_new_artifact() {
  local source="$1"
  local destination="$2"
  "${daemon}" --macos-tcc-canary-publish \
    "${data_dir}/macos-tcc-canary" "${source}" "${destination}" >/dev/null \
    || die "artifact publication failed: ${destination}"
  installed_row_artifacts+=("${destination}")
}

require_real_path_ancestors() {
  local path="$1"
  [[ "${path}" == /* ]] || die "path must be absolute: ${path}"
  local relative="${path#/}"
  local current=""
  local component
  IFS='/' read -r -a components <<<"${relative}"
  for component in "${components[@]-}"; do
    [[ -z "${component}" ]] && continue
    [[ "${component}" != "." && "${component}" != ".." ]] \
      || die "path contains traversal: ${path}"
    current="${current}/${component}"
    [[ ! -L "${current}" ]] || die "path has a symlink ancestor: ${current}"
    [[ -e "${current}" ]] || die "path component does not exist: ${current}"
  done
}

ensure_real_directory() {
  local directory="$1"
  if [[ -e "${directory}" || -L "${directory}" ]]; then
    [[ -d "${directory}" && ! -L "${directory}" ]] \
      || die "directory must be real and not a symlink: ${directory}"
  else
    /bin/mkdir "${directory}" || die "failed to create directory: ${directory}"
  fi
}

ensure_descendant_directory() {
  local root="$1"
  local directory="$2"
  ensure_real_directory "${root}"
  [[ "${directory}" == "${root}" || "${directory}" == "${root}/"* ]] \
    || die "directory escapes canary root: ${directory}"
  local relative="${directory#"${root}"}"
  relative="${relative#/}"
  local current="${root}"
  local component
  IFS='/' read -r -a components <<<"${relative}"
  for component in "${components[@]-}"; do
    [[ -z "${component}" ]] && continue
    identifier_is_safe "${component}" \
      || die "unsafe canary directory component: ${component}"
    current="${current}/${component}"
    ensure_real_directory "${current}"
  done
}

while (($# > 0)); do
  case "$1" in
    --request)
      (($# >= 2)) || die '--request requires a path'
      request="$2"
      shift 2
      ;;
    --daemon)
      (($# >= 2)) || die '--daemon requires a path'
      daemon="$2"
      shift 2
      ;;
    --witness-dir)
      (($# >= 2)) || die '--witness-dir requires a path'
      witness_dir="$2"
      shift 2
      ;;
    --topology)
      (($# >= 2)) || die '--topology requires a value'
      topology="$2"
      shift 2
      ;;
    --app)
      (($# >= 2)) || die '--app requires a path'
      app="$2"
      shift 2
      ;;
    --cli)
      (($# >= 2)) || die '--cli requires a path'
      cli="$2"
      shift 2
      ;;
    --brew)
      (($# >= 2)) || die '--brew requires a path'
      brew="$2"
      shift 2
      ;;
    --timeout-seconds)
      (($# >= 2)) || die '--timeout-seconds requires a value'
      timeout_seconds="$2"
      shift 2
      ;;
    --execute-protected-actions)
      execute=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

require_real_path_ancestors "${request}"
require_real_path_ancestors "${witness_dir}"
require_real_path_ancestors "${witness_dir}/evidence"
require_real_path_ancestors "${data_dir}"
[[ -f "${request}" && ! -L "${request}" ]] \
  || die '--request must name a regular non-symlink file'
request_bytes="$(/usr/bin/stat -f '%z' "${request}")"
[[ "${request_bytes}" =~ ^[0-9]+$ ]] || die 'request size is invalid'
((request_bytes <= 65536)) || die 'request exceeds 65536 bytes'
request_snapshot="$(mktemp -t hypercolor-tcc-request.XXXXXX)"
temporary_files+=("${request_snapshot}")
/bin/cp "${request}" "${request_snapshot}"
chmod 600 "${request_snapshot}"
[[ "$(/usr/bin/stat -f '%z' "${request_snapshot}")" == "${request_bytes}" ]] \
  || die 'request changed during snapshot'
[[ -x "${daemon}" ]] || die '--daemon must name an executable file'
[[ -d "${witness_dir}" && ! -L "${witness_dir}" ]] \
  || die '--witness-dir must name a real non-symlink directory'
[[ -d "${witness_dir}/evidence" && ! -L "${witness_dir}/evidence" ]] \
  || die '--witness-dir/evidence must be a real non-symlink directory'
command -v jq >/dev/null 2>&1 || die 'jq is required'

case "${topology}" in
  app-sidecar)
    [[ -x "${app}" ]] || die 'app-sidecar requires --app executable'
    expected_topology='app_sidecar'
    ;;
  direct-launchd)
    [[ -x "${cli}" ]] || die 'direct-launchd requires --cli executable'
    expected_topology='direct_launchd'
    ;;
  homebrew)
    [[ -x "${brew}" ]] || die 'homebrew requires --brew executable'
    expected_topology='homebrew'
    ;;
  standalone)
    expected_topology='standalone'
    ;;
  *)
    die '--topology must be app-sidecar, direct-launchd, homebrew, or standalone'
    ;;
esac

request_topology="$(jq -er '.expected_topology' "${request_snapshot}")" \
  || die 'request is missing expected_topology'
[[ "${request_topology}" == "${expected_topology}" ]] \
  || die "request topology ${request_topology} does not match ${expected_topology}"
run_id="$(jq -er '.run_id' "${request_snapshot}")" || die 'request is missing run_id'
row_id="$(jq -er '.row_id' "${request_snapshot}")" || die 'request is missing row_id'
lifecycle_phase="$(jq -er '.lifecycle_phase' "${request_snapshot}")" \
  || die 'request is missing lifecycle_phase'
predecessor_row_id="$(jq -r '.predecessor_row_id // ""' "${request_snapshot}")"
replacement_witness_id="$(jq -r '.process_replacement_witness_id // ""' "${request_snapshot}")"
lifecycle_witness_id="$(jq -r '.lifecycle_action_witness_id // ""' "${request_snapshot}")"
login_witness_id="$(jq -r '.login_arbitration_witness_id // ""' "${request_snapshot}")"
settings_witness_id="$(jq -er '.system_settings_identity_witness_id' "${request_snapshot}")" \
  || die 'request is missing system_settings_identity_witness_id'
expected_prompt_text="$(jq -er '.expected_prompt_text' "${request_snapshot}")" \
  || die 'request is missing expected_prompt_text'
expected_system_settings_entry="$(jq -er '.expected_system_settings_entry' "${request_snapshot}")" \
  || die 'request is missing expected_system_settings_entry'
fresh_witness_id="$(jq -r '.fresh_tcc_reset_witness_id // ""' "${request_snapshot}")"
operation_timeout_ms="$(jq -er '.operation_timeout_ms' "${request_snapshot}")" \
  || die 'request is missing operation_timeout_ms'
identifier_is_safe "${run_id}" || die 'request run_id is invalid'
identifier_is_safe "${row_id}" || die 'request row_id is invalid'
identifier_is_safe "${settings_witness_id}" \
  || die 'request system_settings_identity_witness_id is invalid'
if [[ -n "${fresh_witness_id}" ]]; then
  identifier_is_safe "${fresh_witness_id}" \
    || die 'request fresh_tcc_reset_witness_id is invalid'
fi
if [[ -n "${predecessor_row_id}" ]]; then
  identifier_is_safe "${predecessor_row_id}" \
    || die 'request predecessor_row_id is invalid'
  if [[ -n "${replacement_witness_id}" ]]; then
    identifier_is_safe "${replacement_witness_id}" \
      || die 'request process_replacement_witness_id is invalid'
  fi
elif [[ -n "${replacement_witness_id}" ]]; then
  die 'process_replacement_witness_id requires predecessor_row_id'
fi
if [[ -n "${login_witness_id}" ]]; then
  identifier_is_safe "${login_witness_id}" \
    || die 'request login_arbitration_witness_id is invalid'
fi
if [[ -n "${lifecycle_witness_id}" ]]; then
  identifier_is_safe "${lifecycle_witness_id}" \
    || die 'request lifecycle_action_witness_id is invalid'
fi
[[ "${operation_timeout_ms}" =~ ^[0-9]+$ ]] \
  || die 'request operation_timeout_ms is invalid'
minimum_timeout_seconds=$(( (operation_timeout_ms + 999) / 1000 + 30 ))
if [[ -z "${timeout_seconds}" ]]; then
  timeout_seconds="${minimum_timeout_seconds}"
fi
[[ "${timeout_seconds}" =~ ^[0-9]+$ ]] || die '--timeout-seconds must be an integer'
((timeout_seconds >= minimum_timeout_seconds && timeout_seconds <= 660)) \
  || die "--timeout-seconds must be ${minimum_timeout_seconds} through 660 for this row"

request_canonical="$(jq -cS . "${request_snapshot}")" || die 'request is not valid JSON'
request_sha256="$(/usr/bin/shasum -a 256 "${request_snapshot}" | awk '{print $1}')"
"${daemon}" --macos-tcc-canary-check-request "${request_snapshot}" >/dev/null
[[ "$(/usr/bin/shasum -a 256 "${request_snapshot}" | awk '{print $1}')" == "${request_sha256}" ]] \
  || die 'request snapshot changed during validation'

receipt_dir="${data_dir}/macos-tcc-canary/receipts/${run_id}"
receipt="${receipt_dir}/${row_id}.receipt.json"
pending_receipt="${receipt_dir}/${row_id}.receipt.pending"
[[ ! -e "${receipt}" ]] || die "receipt already exists: ${receipt}"
[[ ! -e "${pending_receipt}" ]] || die "pending receipt already exists: ${pending_receipt}"

if [[ "${execute}" != true ]]; then
  die 'refusing protected actions without --execute-protected-actions'
fi

arm_output="$("${daemon}" --macos-tcc-canary-arm "${request_snapshot}")"
armed_request="${arm_output#macos_tcc_canary_armed=}"
[[ "${armed_request}" == "${data_dir}/macos-tcc-canary/request.json" ]] \
  || die 'daemon armed the request outside its canonical data directory'
[[ -f "${armed_request}" && ! -L "${armed_request}" ]] \
  || die 'daemon did not create a regular armed request'
armed_canonical="$(jq -cS . "${armed_request}")" \
  || die 'armed request is not valid JSON'
[[ "${armed_canonical}" == "${request_canonical}" ]] \
  || die 'armed request does not exactly match the validated row'

install_witness() {
  local witness_id="$1"
  local expected_kind="$2"
  local source_witness="${witness_dir}/${witness_id}.witness.json"
  [[ -f "${source_witness}" && ! -L "${source_witness}" ]] \
    || die "missing regular non-symlink witness: ${source_witness}"
  local source_witness_bytes
  source_witness_bytes="$(/usr/bin/stat -f '%z' "${source_witness}")"
  [[ "${source_witness_bytes}" =~ ^[0-9]+$ ]] \
    || die "witness size is invalid: ${source_witness}"
  ((source_witness_bytes <= 65536)) \
    || die "witness exceeds 65536 bytes: ${source_witness}"
  ensure_descendant_directory "${data_dir}/macos-tcc-canary" "${receipt_dir}/evidence"
  local witness_temp
  witness_temp="$(mktemp "${receipt_dir}/.witness.XXXXXX")"
  temporary_files+=("${witness_temp}")
  /bin/cp "${source_witness}" "${witness_temp}"
  chmod 600 "${witness_temp}"
  local evidence_sha256
  evidence_sha256="$(jq -er \
    --arg run_id "${run_id}" \
    --arg row_id "${row_id}" \
    --arg witness_id "${witness_id}" \
    --arg kind "${expected_kind}" \
    'select(
      .schema_version == 2
      and .run_id == $run_id
      and .row_id == $row_id
      and .witness_id == $witness_id
      and .kind == $kind
    ) | .evidence_sha256' \
    "${witness_temp}")" || die "invalid witness: ${source_witness}"
  if [[ "${expected_kind}" == system_settings_identity ]]; then
    jq -e \
      --arg topology "${expected_topology}" \
      --arg prompt_text "${expected_prompt_text}" \
      --arg system_settings_entry "${expected_system_settings_entry}" \
      'select(
        .prompt_text == $prompt_text
        and .system_settings_entry == $system_settings_entry
        and .observed_audit_token_identity == .observed_signing_audit_token_identity
        and (.observed_designated_requirement_sha256 | test("^[0-9a-f]{64}$"))
        and (if $topology == "app_sidecar" then
          .parent_audit_token_identity == .parent_signing_audit_token_identity
          and (.parent_designated_requirement_sha256 | test("^[0-9a-f]{64}$"))
        else
          .parent_pid == null
          and .parent_audit_token_identity == null
          and .parent_signing_audit_token_identity == null
          and .parent_designated_requirement_sha256 == null
        end)
      )' "${witness_temp}" >/dev/null \
      || die "system settings witness lacks audit-token-bound signing evidence: ${source_witness}"
  fi
  [[ "${evidence_sha256}" =~ ^[0-9a-f]{64}$ ]] \
    || die "witness evidence hash is invalid: ${source_witness}"
  local source_evidence="${witness_dir}/evidence/${evidence_sha256}.bin"
  [[ -f "${source_evidence}" && ! -L "${source_evidence}" ]] \
    || die "missing regular witness evidence: ${source_evidence}"
  local source_evidence_bytes
  source_evidence_bytes="$(/usr/bin/stat -f '%z' "${source_evidence}")"
  [[ "${source_evidence_bytes}" =~ ^[0-9]+$ ]] \
    || die "witness evidence size is invalid: ${source_evidence}"
  ((source_evidence_bytes <= 16777216)) \
    || die "witness evidence exceeds 16777216 bytes: ${source_evidence}"
  local destination_witness="${receipt_dir}/${witness_id}.witness.json"
  local destination_evidence="${receipt_dir}/evidence/${evidence_sha256}.bin"
  [[ ! -e "${destination_witness}" ]] \
    || die "witness already exists: ${destination_witness}"
  if [[ ! -e "${destination_evidence}" ]]; then
    local evidence_temp
    evidence_temp="$(mktemp "${receipt_dir}/evidence/.witness-evidence.XXXXXX")"
    temporary_files+=("${evidence_temp}")
    /bin/cp "${source_evidence}" "${evidence_temp}"
    chmod 600 "${evidence_temp}"
    [[ "$(/usr/bin/shasum -a 256 "${evidence_temp}" | awk '{print $1}')" == "${evidence_sha256}" ]] \
      || die "witness evidence hash mismatch: ${source_evidence}"
    install_new_artifact "${evidence_temp}" "${destination_evidence}"
  else
    [[ "$(/usr/bin/shasum -a 256 "${destination_evidence}" | awk '{print $1}')" == "${evidence_sha256}" ]] \
      || die "installed witness evidence hash mismatch: ${destination_evidence}"
  fi
  install_new_artifact "${witness_temp}" "${destination_witness}"
}

ensure_descendant_directory "${data_dir}/macos-tcc-canary" "${receipt_dir}/evidence"
if [[ -n "${fresh_witness_id}" ]]; then
  install_witness "${fresh_witness_id}" fresh_tcc_reset
fi
if [[ -n "${login_witness_id}" ]]; then
  install_witness "${login_witness_id}" login_arbitration
fi

predecessor_pid=""
predecessor_fingerprint=""
predecessor_audit_token_identity=""
predecessor_finished_unix_ms=""
predecessor_was_live=false
predecessor_parent_pid=""
predecessor_parent_audit_token_identity=""
predecessor_parent_fingerprint=""
predecessor_parent_was_live=false
launcher_action=""
replacement_required=false
case "${lifecycle_phase}" in
  later_grant|grant_after_revocation|owner_restart|app_relaunch|service_restart|signed_update)
    replacement_required=true
    ;;
esac
if [[ -n "${predecessor_row_id}" ]]; then
  if [[ "${replacement_required}" == true ]]; then
    [[ -n "${replacement_witness_id}" ]] \
      || die 'replacement phase requires process_replacement_witness_id'
  elif [[ -n "${replacement_witness_id}" ]]; then
    die 'non-replacement phase cannot name process_replacement_witness_id'
  fi
  predecessor_receipt="${receipt_dir}/${predecessor_row_id}.receipt.json"
  [[ -f "${predecessor_receipt}" && ! -L "${predecessor_receipt}" ]] \
    || die "predecessor receipt must be a regular non-symlink file: ${predecessor_receipt}"
  predecessor_bytes="$(/usr/bin/stat -f '%z' "${predecessor_receipt}")"
  [[ "${predecessor_bytes}" =~ ^[0-9]+$ ]] \
    || die 'predecessor receipt size is invalid'
  ((predecessor_bytes <= 131072)) || die 'predecessor receipt exceeds 131072 bytes'
  predecessor_snapshot="$(mktemp "${receipt_dir}/.predecessor.XXXXXX")"
  temporary_files+=("${predecessor_snapshot}")
  /bin/cp "${predecessor_receipt}" "${predecessor_snapshot}"
  chmod 600 "${predecessor_snapshot}"
  [[ "$(/usr/bin/stat -f '%z' "${predecessor_snapshot}")" == "${predecessor_bytes}" ]] \
    || die 'predecessor receipt changed during snapshot'
  predecessor_identity="$(jq -er \
    --arg run_id "${run_id}" \
    --arg topology "${expected_topology}" \
    'select(.schema_version == 2 and .run_id == $run_id and .topology == $topology) \
      | [.pid, .process_fingerprint, .audit_token_identity, .operation_finished_unix_ms] \
      | @tsv' \
    "${predecessor_snapshot}")" \
    || die 'predecessor receipt does not match this run and topology'
  IFS=$'\t' read -r predecessor_pid predecessor_fingerprint \
    predecessor_audit_token_identity predecessor_finished_unix_ms <<<"${predecessor_identity}"
  [[ "${predecessor_pid}" =~ ^[0-9]+$ ]] || die 'predecessor pid is invalid'
  [[ "${predecessor_fingerprint}" =~ ^[0-9a-f]{64}$ ]] \
    || die 'predecessor process fingerprint is invalid'
  [[ "${predecessor_audit_token_identity}" =~ ^([0-9a-fA-F]{8}:){7}[0-9a-fA-F]{8}$ ]] \
    || die 'predecessor audit token identity is invalid'
  [[ "${predecessor_finished_unix_ms}" =~ ^[0-9]+$ ]] \
    || die 'predecessor completion time is invalid'
  if pid_is_alive "${predecessor_pid}"; then
    current_predecessor_fingerprint="$(process_fingerprint "${predecessor_pid}")" \
      || die 'live predecessor process identity is unavailable'
    [[ "${current_predecessor_fingerprint}" == "${predecessor_fingerprint}" ]] \
      || die 'predecessor pid was reused by a different process'
    predecessor_was_live=true
  fi
  if [[ "${topology}:${lifecycle_phase}" == app-sidecar:app_relaunch ]]; then
    predecessor_parent_identity="$(jq -er \
      '[.launcher.parent_pid, .launcher.parent_signing.process_bound_fingerprint, \
        .system_settings_identity_witness_id] | @tsv' \
      "${predecessor_snapshot}")" \
      || die 'app predecessor parent identity is missing'
    IFS=$'\t' read -r predecessor_parent_pid predecessor_parent_fingerprint \
      predecessor_settings_witness_id <<<"${predecessor_parent_identity}"
    [[ "${predecessor_parent_pid}" =~ ^[0-9]+$ ]] \
      || die 'app predecessor parent pid is invalid'
    [[ "${predecessor_parent_fingerprint}" =~ ^[0-9a-f]{64}$ ]] \
      || die 'app predecessor parent fingerprint is invalid'
    identifier_is_safe "${predecessor_settings_witness_id}" \
      || die 'app predecessor settings witness id is invalid'
    predecessor_settings_witness="${receipt_dir}/${predecessor_settings_witness_id}.witness.json"
    [[ -f "${predecessor_settings_witness}" && ! -L "${predecessor_settings_witness}" ]] \
      || die 'app predecessor settings witness is missing'
    predecessor_parent_audit_token_identity="$(jq -er \
      --arg run_id "${run_id}" \
      --arg row_id "${predecessor_row_id}" \
      'select(.schema_version == 2 and .run_id == $run_id and .row_id == $row_id \
        and .kind == "system_settings_identity") | .parent_audit_token_identity' \
      "${predecessor_settings_witness}")" \
      || die 'app predecessor parent audit token is missing'
    [[ "${predecessor_parent_audit_token_identity}" =~ ^([0-9a-fA-F]{8}:){7}[0-9a-fA-F]{8}$ ]] \
      || die 'app predecessor parent audit token is invalid'
    pid_is_alive "${predecessor_parent_pid}" \
      || die 'app predecessor parent is no longer running'
    current_parent_fingerprint="$(process_fingerprint "${predecessor_parent_pid}")" \
      || die 'live app predecessor parent identity is unavailable'
    [[ "${current_parent_fingerprint}" == "${predecessor_parent_fingerprint}" ]] \
      || die 'app predecessor parent pid was reused by a different process'
    predecessor_parent_was_live=true
  fi
fi

case "${topology}" in
  app-sidecar)
    case "${lifecycle_phase}" in
      owner_restart)
        die 'owner_restart requires launcher action app_supervisor_daemon_restart'
        ;;
      later_grant|grant_after_revocation)
        die "${lifecycle_phase} requires launcher action app_supervisor_daemon_restart_after_authorization"
        ;;
      signed_update)
        die 'signed_update requires launcher action signed_app_update_then_app_relaunch'
        ;;
      app_relaunch)
        launcher_action='app_quit_then_minimized_launch'
        ;;
      app_launch)
        launcher_action='app_minimized_launch'
        ;;
    esac
    ;;
  direct-launchd)
    case "${lifecycle_phase}" in
      service_install) launcher_action='hypercolor_service_enable' ;;
      login_start) die 'login_start requires launcher action launchd_login_start' ;;
      service_restart) launcher_action='hypercolor_service_restart' ;;
      later_grant|grant_after_revocation)
        launcher_action='hypercolor_service_restart_after_authorization'
        ;;
      signed_update)
        die 'signed_update requires launcher action signed_daemon_update_then_hypercolor_service_restart'
        ;;
      *) launcher_action='hypercolor_service_restart' ;;
    esac
    ;;
  homebrew)
    case "${lifecycle_phase}" in
      service_install) launcher_action='brew_services_start' ;;
      login_start) die 'login_start requires launcher action brew_services_login_start' ;;
      service_restart) launcher_action='brew_services_restart' ;;
      later_grant|grant_after_revocation)
        launcher_action='brew_services_restart_after_authorization'
        ;;
      signed_update)
        die 'signed_update requires launcher action signed_daemon_update_then_brew_services_restart'
        ;;
      *) launcher_action='brew_services_restart' ;;
    esac
    ;;
  standalone)
    case "${lifecycle_phase}" in
      later_grant|grant_after_revocation)
        launcher_action='terminal_successor_launch_after_authorization'
        ;;
      signed_update)
        die 'signed_update requires launcher action signed_daemon_update_then_terminal_launch'
        ;;
      *) launcher_action='terminal_launch' ;;
    esac
    ;;
esac

if [[ "${replacement_required}" == true ]]; then
  case "${topology}" in
    app-sidecar)
      "${app}" --quit >/dev/null 2>&1
      ;;
    direct-launchd)
      "${cli}" service stop
      ;;
    homebrew)
      "${brew}" services stop hypercolor
      ;;
    standalone)
      :
      ;;
  esac
  if [[ "${predecessor_was_live}" == true ]]; then
    wait_for_pid_exit "${predecessor_pid}" "${predecessor_fingerprint}" 60
  fi
  if [[ "${predecessor_parent_was_live}" == true ]]; then
    wait_for_pid_exit "${predecessor_parent_pid}" "${predecessor_parent_fingerprint}" 60
  fi
fi

action_observed_unix_ms=$(( $(date +%s) * 1000 ))
while [[ -n "${predecessor_finished_unix_ms}" ]] \
  && ((action_observed_unix_ms < predecessor_finished_unix_ms)); do
  sleep 1
  action_observed_unix_ms=$(( $(date +%s) * 1000 ))
done

if [[ "${replacement_required}" == true ]]; then
  evidence_temp="$(mktemp "${receipt_dir}/evidence/.replacement.XXXXXX")"
  printf 'run=%s\nrow=%s\npredecessor=%s\npid=%s\naudit_token=%s\nfingerprint=%s\nparent_pid=%s\nparent_audit_token=%s\nparent_fingerprint=%s\ntopology=%s\naction=%s\nexit_observed=true\n' \
    "${run_id}" "${row_id}" "${predecessor_row_id}" "${predecessor_pid}" \
    "${predecessor_audit_token_identity}" "${predecessor_fingerprint}" \
    "${predecessor_parent_pid}" "${predecessor_parent_audit_token_identity}" \
    "${predecessor_parent_fingerprint}" \
    "${expected_topology}" "${launcher_action}" >"${evidence_temp}"
  evidence_sha256="$(/usr/bin/shasum -a 256 "${evidence_temp}" | awk '{print $1}')"
  evidence_path="${receipt_dir}/evidence/${evidence_sha256}.bin"
  [[ ! -e "${evidence_path}" ]] || die "evidence already exists: ${evidence_path}"
  install_new_artifact "${evidence_temp}" "${evidence_path}"
  witness_path="${receipt_dir}/${replacement_witness_id}.witness.json"
  [[ ! -e "${witness_path}" ]] || die "witness already exists: ${witness_path}"
  witness_temp="$(mktemp "${receipt_dir}/.replacement-witness.XXXXXX")"
  temporary_files+=("${witness_temp}")
  jq -n \
    --arg run_id "${run_id}" \
    --arg row_id "${row_id}" \
    --arg witness_id "${replacement_witness_id}" \
    --arg evidence_sha256 "${evidence_sha256}" \
    --arg launcher_action "${launcher_action}" \
    --arg predecessor_audit_token_identity "${predecessor_audit_token_identity}" \
    --arg predecessor_process_fingerprint "${predecessor_fingerprint}" \
    --arg predecessor_parent_audit_token_identity "${predecessor_parent_audit_token_identity}" \
    --arg predecessor_parent_process_fingerprint "${predecessor_parent_fingerprint}" \
    --argjson observed_unix_ms "${action_observed_unix_ms}" \
    --argjson predecessor_pid "${predecessor_pid}" \
    --arg predecessor_parent_pid "${predecessor_parent_pid}" \
    '{
      schema_version: 2,
      run_id: $run_id,
      row_id: $row_id,
      witness_id: $witness_id,
      kind: "process_replacement",
      observer: "run-macos-tcc-canary-row.sh",
      observed_unix_ms: $observed_unix_ms,
      evidence_sha256: $evidence_sha256,
      prompt_text: null,
      system_settings_entry: null,
      fresh_tcc_database_observed: null,
      predecessor_pid: $predecessor_pid,
      predecessor_audit_token_identity: $predecessor_audit_token_identity,
      predecessor_process_fingerprint: $predecessor_process_fingerprint,
      predecessor_exit_observed: true,
      predecessor_parent_pid: (if $predecessor_parent_pid == "" then null else ($predecessor_parent_pid | tonumber) end),
      predecessor_parent_audit_token_identity: (if $predecessor_parent_audit_token_identity == "" then null else $predecessor_parent_audit_token_identity end),
      predecessor_parent_process_fingerprint: (if $predecessor_parent_process_fingerprint == "" then null else $predecessor_parent_process_fingerprint end),
      predecessor_parent_exit_observed: (if $predecessor_parent_pid == "" then null else true end),
      launcher_action: $launcher_action
    }' >"${witness_temp}"
  chmod 600 "${witness_temp}"
  install_new_artifact "${witness_temp}" "${witness_path}"
fi

if [[ -n "${lifecycle_witness_id}" ]]; then
  [[ "${replacement_required}" == false ]] \
    || die 'replacement rows cannot use lifecycle_action_witness_id'
  action_evidence_temp="$(mktemp "${receipt_dir}/evidence/.lifecycle.XXXXXX")"
  printf 'run=%s\nrow=%s\ntopology=%s\naction=%s\n' \
    "${run_id}" "${row_id}" "${expected_topology}" "${launcher_action}" \
    >"${action_evidence_temp}"
  action_evidence_sha256="$(/usr/bin/shasum -a 256 "${action_evidence_temp}" | awk '{print $1}')"
  action_evidence_path="${receipt_dir}/evidence/${action_evidence_sha256}.bin"
  [[ ! -e "${action_evidence_path}" ]] \
    || die "evidence already exists: ${action_evidence_path}"
  install_new_artifact "${action_evidence_temp}" "${action_evidence_path}"
  lifecycle_witness_path="${receipt_dir}/${lifecycle_witness_id}.witness.json"
  lifecycle_witness_temp="$(mktemp "${receipt_dir}/.lifecycle-witness.XXXXXX")"
  temporary_files+=("${lifecycle_witness_temp}")
  jq -n \
    --arg run_id "${run_id}" \
    --arg row_id "${row_id}" \
    --arg witness_id "${lifecycle_witness_id}" \
    --arg evidence_sha256 "${action_evidence_sha256}" \
    --arg launcher_action "${launcher_action}" \
    --argjson observed_unix_ms "${action_observed_unix_ms}" \
    '{
      schema_version: 2,
      run_id: $run_id,
      row_id: $row_id,
      witness_id: $witness_id,
      kind: "lifecycle_action",
      observer: "run-macos-tcc-canary-row.sh",
      observed_unix_ms: $observed_unix_ms,
      evidence_sha256: $evidence_sha256,
      launcher_action: $launcher_action
    }' >"${lifecycle_witness_temp}"
  chmod 600 "${lifecycle_witness_temp}"
  install_new_artifact "${lifecycle_witness_temp}" "${lifecycle_witness_path}"
fi

case "${topology}:${lifecycle_phase}" in
  app-sidecar:*)
    "${app}" --minimized >/dev/null 2>&1 &
    ;;
  direct-launchd:service_install)
    "${cli}" service enable
    ;;
  direct-launchd:service_restart|direct-launchd:later_grant|direct-launchd:grant_after_revocation)
    "${cli}" service start
    ;;
  direct-launchd:*)
    "${cli}" service restart
    ;;
  homebrew:service_install)
    "${brew}" services start hypercolor
    ;;
  homebrew:service_restart|homebrew:later_grant|homebrew:grant_after_revocation)
    "${brew}" services start hypercolor
    ;;
  homebrew:*)
    "${brew}" services restart hypercolor
    ;;
  standalone:*)
    "${daemon}" --macos-owner standalone >/dev/null 2>&1 &
    ;;
esac

deadline=$((SECONDS + timeout_seconds))
while [[ ! -f "${pending_receipt}" && ${SECONDS} -lt ${deadline} ]]; do
  sleep 1
done
[[ -f "${pending_receipt}" && ! -L "${pending_receipt}" ]] \
  || die "a regular atomic pending receipt did not arrive within ${timeout_seconds} seconds"

install_witness "${settings_witness_id}" system_settings_identity

while [[ ! -f "${receipt}" && ${SECONDS} -lt ${deadline} ]]; do
  sleep 1
done
[[ -f "${receipt}" && ! -L "${receipt}" ]] \
  || die "a regular atomic receipt did not arrive within ${timeout_seconds} seconds"

jq -e \
  --arg run_id "${run_id}" \
  --arg row_id "${row_id}" \
  --arg topology "${expected_topology}" \
  '.schema_version == 2
    and .run_id == $run_id
    and .row_id == $row_id
    and .topology == $topology
    and .acceptance_claim == "evidence_only"
    and .signing.process_bound_valid == true
    and .signing.audit_token_bound_valid == true
    and .signing.process_bound_pid == .pid
    and (if .topology == "app_sidecar"
      then .launcher.parent_signing.audit_token_bound_valid == true
      else true
    end)
    and .launcher.verified == true' \
  "${receipt}" >/dev/null \
  || die 'daemon receipt does not match the requested production launcher row'

row_committed=true
printf 'macOS TCC canary receipt: %s\n' "${receipt}"
