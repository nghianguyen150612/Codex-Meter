#!/usr/bin/env bash
set -uo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null)" || {
  printf '%s\n' 'arch_reference_tests=UNVERIFIED' 'diagnostic=Run from a Codex-Meter checkout'
  exit 2
}
cd "$repo_root" || exit 2

source /etc/os-release
kernel="$(uname -r)"
machine="$(uname -m)"
printf 'distribution=%s\n' "$PRETTY_NAME"
printf 'kernel=%s\n' "$kernel"
printf 'architecture=%s\n' "$machine"

if [[ "$ID" != arch || "$machine" != x86_64 ]]; then
  printf '%s\n' 'arch_reference_tests=UNVERIFIED' \
    'diagnostic=This procedure requires native Arch Linux x86_64'
  exit 2
fi

if ! command -v systemd-detect-virt >/dev/null 2>&1; then
  printf '%s\n' 'arch_reference_tests=UNVERIFIED' \
    'diagnostic=Cannot exclude a container or virtual machine'
  exit 2
fi
container_kind="$(systemd-detect-virt --container 2>/dev/null || true)"
vm_kind="$(systemd-detect-virt --vm 2>/dev/null || true)"
if [[ -n "$container_kind" && "$container_kind" != none ]] ||
  [[ -n "$vm_kind" && "$vm_kind" != none ]]; then
  printf 'container=%s\n' "$container_kind"
  printf 'virtual_machine=%s\n' "$vm_kind"
  printf '%s\n' 'arch_reference_tests=UNVERIFIED' \
    'diagnostic=Container and virtual-machine results cannot qualify the reference device'
  exit 2
fi
printf '%s\n' 'host_class=native_physical_candidate' 'container=none' 'virtual_machine=none'
if [[ "$#" -ne 1 ]]; then
  printf '%s\n' 'designated_reference_device=not_confirmed' \
    'arch_reference_tests=UNVERIFIED' \
    'diagnostic=Re-run with --confirm-reference-device only on the designated physical Arch device'
  exit 2
fi
if [[ "$1" != --confirm-reference-device ]]; then
  printf '%s\n' 'designated_reference_device=not_confirmed' \
    'arch_reference_tests=UNVERIFIED' \
    'diagnostic=The only supported option is --confirm-reference-device'
  exit 2
fi
printf '%s\n' 'designated_reference_device=operator_confirmed'

if ! command -v findmnt >/dev/null 2>&1; then
  printf '%s\n' 'arch_reference_tests=UNVERIFIED' \
    'diagnostic=findmnt is required to record the tested filesystem'
  exit 2
fi
mount_data="$(findmnt -n -T "$repo_root" -o FSTYPE,OPTIONS 2>/dev/null)" || {
  printf '%s\n' 'arch_reference_tests=UNVERIFIED' \
    'diagnostic=The checkout filesystem could not be identified'
  exit 2
}
read -r fs_type raw_mount_options <<<"$mount_data"
if [[ -z "$fs_type" || -z "$raw_mount_options" ]]; then
  printf '%s\n' 'arch_reference_tests=UNVERIFIED' \
    'diagnostic=Filesystem type or mount options were unavailable'
  exit 2
fi
mount_options=()
IFS=',' read -r -a option_list <<<"$raw_mount_options"
for option in "${option_list[@]}"; do
  case "$option" in
    rw|ro|sync|dirsync|relatime|noatime|strictatime|lazytime|barrier|nobarrier)
      mount_options+=("$option")
      ;;
    commit=*|subvolid=*|space_cache=*)
      if [[ "$option" =~ ^(commit|subvolid)=[0-9]+$ ]] ||
        [[ "$option" =~ ^space_cache=v[0-9]+$ ]]; then
        mount_options+=("$option")
      fi
      ;;
    discard=async)
      mount_options+=("$option")
      ;;
    compress=*)
      if [[ "$option" =~ ^compress=[A-Za-z0-9._:-]+$ ]]; then
        mount_options+=("$option")
      fi
      ;;
  esac
done
sanitized_options="$(IFS=,; printf '%s' "${mount_options[*]}")"
printf 'filesystem=%s\n' "$fs_type"
printf 'mount_options=%s\n' "$sanitized_options"

if [[ ! -t 0 ]]; then
  printf '%s\n' 'attached_terminal=none'
else
  printf '%s\n' 'attached_terminal=interactive'
fi
if ! python3 -c 'import pty, os; master, slave = pty.openpty(); print("pty_openpty=available"); os.close(master); os.close(slave)'; then
  printf '%s\n' 'pty_openpty=unavailable'
  printf '%s\n' 'arch_reference_tests=UNVERIFIED' \
    'diagnostic=Python could not create a PTY on the reference host'
  exit 2
fi

commit="$(git rev-parse HEAD)"
if [[ -n "$(git status --porcelain)" ]]; then
  printf 'commit=%s\n' "$commit"
  printf '%s\n' 'working_tree=dirty' 'arch_reference_tests=UNVERIFIED' \
    'diagnostic=Use a clean checkout so evidence identifies the tested source'
  exit 2
fi
printf 'commit=%s\n' "$commit"
printf '%s\n' 'working_tree=clean'

if ! command -v rustc >/dev/null 2>&1 || ! command -v cargo >/dev/null 2>&1 ||
  ! command -v rustup >/dev/null 2>&1; then
  printf '%s\n' 'rust_toolchain=unavailable' 'arch_reference_tests=UNVERIFIED'
  exit 2
fi
printf 'rustc=%s\n' "$(rustc --version)"
printf 'rust_host=%s\n' "$(rustc -Vv | awk -F': ' '$1 == "host" { print $2 }')"
printf 'rust_toolchain=%s\n' "$(rustup show active-toolchain | awk '{ print $1 }')"
installed_targets="$(rustup target list --installed | paste -sd, -)"
printf 'rust_targets=%s\n' "$installed_targets"

if command -v codex >/dev/null 2>&1; then
  printf 'codex_cli_version=%s\n' "$(codex --version 2>/dev/null | head -n 1)"
  python3 scripts/p038a_probe_app_server.py --program "$(command -v codex)"
else
  printf '%s\n' 'codex_cli=unavailable' 'app_server_probe=UNVERIFIED'
fi

overall_status=0
run_command() {
  local command_label="$1"
  shift
  local capture
  local exit_code
  capture="$(mktemp)"
  printf 'command=%s\n' "$command_label"
  if "$@" >"$capture" 2>&1; then
    exit_code=0
  else
    exit_code=$?
    overall_status=1
  fi
  printf 'exit_code=%s\n' "$exit_code"
  if grep -E '^test result: ' "$capture" >/dev/null; then
    grep -E '^test result: ' "$capture" | sed -E 's/; [0-9]+ measured; [0-9]+ filtered out//'
  fi
  if [[ "$exit_code" -ne 0 ]]; then
    sed -nE 's/^test (.+) \.\.\. FAILED$/failed_test=\1/p' "$capture"
    printf '%s\n' 'failure_details=raw test output withheld from sanitized evidence'
  fi
  rm -f -- "$capture"
}

if [[ "$machine" != x86_64 || "$(rustc -Vv | awk -F': ' '$1 == "host" { print $2 }')" != x86_64-unknown-linux-gnu ]]; then
  printf '%s\n' 'arch_reference_tests=UNVERIFIED' \
    'diagnostic=The installed Rust host target must be x86_64-unknown-linux-gnu'
  exit 2
fi

run_command 'cargo fmt --manifest-path rust/Cargo.toml --all -- --check' \
  cargo fmt --manifest-path rust/Cargo.toml --all -- --check
run_command 'cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets --all-features -- -D warnings' \
  cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets --all-features -- -D warnings
run_command 'cargo test --manifest-path rust/Cargo.toml -p codex-meter --lib codex_app_server::execution_recovery' \
  cargo test --manifest-path rust/Cargo.toml -p codex-meter --lib codex_app_server::execution_recovery
run_command 'cargo test --manifest-path rust/Cargo.toml -p codex-meter --test pty_execution_recovery -- --test-threads=1' \
  cargo test --manifest-path rust/Cargo.toml -p codex-meter --test pty_execution_recovery -- --test-threads=1
run_command 'cargo test --manifest-path rust/Cargo.toml --workspace --all-targets' \
  cargo test --manifest-path rust/Cargo.toml --workspace --all-targets
run_command 'cargo test --manifest-path rust/Cargo.toml --workspace --all-targets --all-features' \
  cargo test --manifest-path rust/Cargo.toml --workspace --all-targets --all-features

if [[ "$overall_status" -eq 0 ]]; then
  printf '%s\n' 'arch_reference_tests=PASS'
  exit 0
fi
printf '%s\n' 'arch_reference_tests=FAIL'
exit 1
