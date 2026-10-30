#!/usr/bin/env bash
# Atomic board build: `copper route-board`, a saved KiCad refill with native
# DRC, and a run.json manifest that binds the outputs by SHA-256.
#
#   route-build.sh SOURCE [--copper PATH] [--project-root DIR] [--output-root DIR]
#                  [--output-dir DIR] [--kicad-cli PATH] [--kicad-footprints DIR]
#                  [--locked] [--offline] [--dry-run] -- [ROUTE-BOARD OPTIONS...]
#
# Exit status: 0 passed, 1 unmet completion gates, 2 setup or runner error,
# 3 unsupported (the compiler's not-implemented exit), 130 interrupted.
# Requires bash, jq, sha256sum (or shasum) and KiCad's kicad-cli.
set -o pipefail

die() { echo "SETUP ERROR: $*" >&2; exit 2; }
sha256() { if command -v sha256sum > /dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
absolute() { if [ -d "$1" ]; then (cd "$1" && pwd -P); else echo "$(cd "$(dirname "$1")" && pwd -P)/$(basename "$1")"; fi; }
quote() { local out="" arg; for arg in "$@"; do out+="${out:+ }$(printf '%q' "$arg")"; done; echo "$out"; }

source_file="" copper="${COPPER_BIN:-copper}" project="$PWD" output_root="" output_dir=""
kicad_cli="${KICAD_CLI:-}" footprints="" locked=() offline=() dry_run=0
while [ $# -gt 0 ]; do
  case "$1" in
    --copper) copper="$2"; shift 2 ;;
    --project-root) project="$2"; shift 2 ;;
    --output-root) output_root="$2"; shift 2 ;;
    --output-dir) output_dir="$2"; shift 2 ;;
    --kicad-cli) kicad_cli="$2"; shift 2 ;;
    --kicad-footprints) footprints="$2"; shift 2 ;;
    --locked) locked=(--locked); shift ;;
    --offline) offline=(--offline); shift ;;
    --dry-run) dry_run=1; shift ;;
    --) shift; break ;;
    -*) die "unknown option: $1" ;;
    *) [ -z "$source_file" ] || die "unexpected argument: $1"; source_file="$1"; shift ;;
  esac
done
options=("$@")
command -v jq > /dev/null || die "jq is required"
[ -n "$source_file" ] || die "a SOURCE board is required"
[ -f "$source_file" ] || die "source missing: $source_file"
for arg in "${options[@]}"; do
  case "${arg%%=*}" in
    -o|--output|--report|--verify-plane-fill|--locked|--offline)
      die "output, report, verification and resolution flags are controlled by the build runner" ;;
    -o*) [[ "$arg" == --* ]] || die "output, report, verification and resolution flags are controlled by the build runner" ;;
  esac
done

compiler="$(command -v "$copper" || true)"
[ -n "$compiler" ] && [ -f "$compiler" ] || die "compiler executable missing: $copper; install the copper CLI"
compiler="$(absolute "$compiler")"
if [ -z "$kicad_cli" ]; then
  kicad_cli="$(command -v kicad-cli || true)"
  [ -n "$kicad_cli" ] || kicad_cli="C:/Program Files/KiCad/10.0/bin/kicad-cli.exe"
fi
kicad_cli="$(command -v "$kicad_cli" || echo "$kicad_cli")"
[ -f "$kicad_cli" ] || die "KiCad CLI or footprint directory missing; supply explicit paths"
kicad_cli="$(absolute "$kicad_cli")"
if [ -z "$footprints" ]; then
  footprints="$(dirname "$(dirname "$kicad_cli")")/share/kicad/footprints"
  [ -d "$footprints" ] || footprints=/usr/share/kicad/footprints
fi
[ -d "$footprints" ] || die "KiCad CLI or footprint directory missing; supply explicit paths"
footprints="$(absolute "$footprints")"

[ -d "$project" ] || die "project root missing: $project"
project="$(absolute "$project")"
source_file="$(absolute "$source_file")"
build="$project/build"
case "$source_file" in "$project"/*) ;; *) die "source must be inside --project-root" ;; esac
stem="$(basename "${source_file%.*}")"
output_root="${output_root:-$build/$stem}"
started="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
output="${output_dir:-$output_root/$(date -u +%Y%m%dT%H%M%SZ)-$$}"
mkdir -p "$(dirname "$output")" || die "cannot create $(dirname "$output")"
output="$(absolute "$output")"
case "$output" in "$build"/*) ;; *) die "output must stay below project build/" ;; esac
if [ -e "$output" ] && { [ ! -d "$output" ] || [ -n "$(ls -A "$output")" ]; }; then die "output is not empty: $output"; fi

route=("$compiler" route-board "$source_file" "${locked[@]}" --offline --footprint-root "$footprints" "${options[@]}"
  --verify-plane-fill "$kicad_cli" --report "$output/route-report.json" -o "$output/board.kicad_pcb")
fill=("$kicad_cli" pcb drc --refill-zones --save-board --severity-all --format json
  --output "$output/kicad-drc.json" "$output/board.kicad_pcb")

# Resolve this board's imports and physical macro assets through its nearest
# manifest (fetching only pinned dependencies unless offline); routing itself
# then runs offline. The check lists every macro input for the provenance.
check_offline=("${offline[@]}")
[ "$dry_run" = 0 ] || check_offline=(--offline)
macro_inputs="$(mktemp)" || die "cannot create a temporary file"
trap 'rm -f "$macro_inputs"' EXIT
"$compiler" check "$source_file" "${locked[@]}" "${check_offline[@]}" --macro-inputs "$macro_inputs" > /dev/null ||
  die "copper check failed for $source_file"

echo "Output: $output"
quote "${route[@]}"
quote "${fill[@]}"
[ "$dry_run" = 0 ] || exit 0

# Project-owned sources and manifests, excluding generated and cache trees,
# plus the macro inputs the source binds and those the options name;
# external package inventories are authenticated by the committed lock.
inputs="$(
  {
    echo "$source_file"
    cat "$macro_inputs"
    find "$project" \( -name build -o -name dist -o -name .git -o -name .copper-cache -o -name target \) -prune -o \
      -type f \( -name '*.copper' -o -name '*.mk' -o -name copper.mod -o -name copper.lock -o -name Makefile \) -print
    for ((i = 0; i < ${#options[@]}; i++)); do
      case "${options[$i]}" in
        --hard-macro|--placement-templates) [ -n "${options[$((i + 1))]:-}" ] && absolute "${options[$((i + 1))]}" ;;
        --hard-macro=*|--placement-templates=*) absolute "${options[$i]#*=}" ;;
      esac
    done
  } | LC_ALL=C sort -u | while IFS= read -r path; do
    [ -f "$path" ] && printf '%s\t%s\n' "${path#"$project"/}" "$(sha256 "$path")"
  done | jq -R -s 'split("\n") | map(select(length > 0) | split("\t") | {(.[0]): .[1]}) | add // {}'
)"
git_commit="$(git -C "$project" rev-parse HEAD 2> /dev/null || true)"
provenance="$(jq -n --argjson inputs "$inputs" --arg commit "$git_commit" \
  --arg changes "$(git -C "$project" status --porcelain --untracked-files=no 2> /dev/null || true)" \
  --arg diff "$(git -C "$project" diff HEAD --no-ext-diff 2> /dev/null | sha256 /dev/stdin)" \
  --arg platform "$(uname -srm)" --arg runner "$(sha256 "${BASH_SOURCE[0]}")" \
  --arg compiler "$compiler" --arg compiler_sha "$(sha256 "$compiler")" \
  --arg version "$("$compiler" --version 2> /dev/null || true)" \
  '{platform: $platform, build_runner_sha256: $runner,
    git_commit: (if $commit == "" then null else $commit end), tracked_changes: $changes, tracked_diff_sha256: $diff,
    compiler: {executable: $compiler, sha256: $compiler_sha, version: $version}, input_sha256: $inputs}')"

mkdir -p "$output" || die "cannot create $output"
manifest="$output/run.json"
# Claim the run exclusively: never overwrite another caller's evidence.
( set -o noclobber; : > "$manifest" ) 2> /dev/null || die "output is not empty: $output"
run="$(jq -n --arg started "$started" --arg route "$(quote "${route[@]}")" --arg fill "$(quote "${fill[@]}")" \
  --argjson provenance "$provenance" \
  '{started_utc: $started, status: "running", routing_command: $route, fill_command: $fill,
    fabrication_ready: false, provenance: $provenance}')"
echo "$run" > "$manifest"

completed() {
  jq -e -n --slurpfile route "$output/route-report.json" --slurpfile native "$output/kicad-drc.json" '
    $route[0] as $r | $native[0] as $n
    | $r.status == "pass" and $r.erc_pass == true and $r.routing_complete == true
      and $r.connectivity.native_fill_verified == true and $r.connectivity.unrouted_ordinary_nets == []
      and $n.violations == [] and $n.unconnected_items == []' > /dev/null 2>&1 && [ -f "$output/board.kicad_pcb" ]
}

tick=$SECONDS code=2
trap 'code=130' INT
"${route[@]}" 2>&1 | tee "$output/routing.log"
code=${PIPESTATUS[0]}
if [ -f "$output/board.editor-intent.json" ] && [ -f "$output/board.kicad_pcb" ]; then
  run="$(jq --arg intent "$(sha256 "$output/board.editor-intent.json")" --arg draft "$(sha256 "$output/board.kicad_pcb")" \
    '. + {editor_intent_sha256: $intent, draft_board_sha256: $draft}' <<< "$run")"
fi
if [ "$code" = 0 ]; then
  "${fill[@]}" 2>&1 | tee "$output/fill.log"
  code=${PIPESTATUS[0]}
  if [ "$code" = 0 ] && ! completed; then code=1; fi
  if [ "$code" = 0 ]; then
    run="$(jq --arg board "$(sha256 "$output/board.kicad_pcb")" --arg drc "$(sha256 "$output/kicad-drc.json")" \
      '. + {filled_board_sha256: $board, native_drc_sha256: $drc}' <<< "$run")"
  fi
fi
case "$code" in 0) status=passed ;; 1) status=unmet_gates ;; 3) status=unsupported ;; 130) status=interrupted ;; *) status=error ;; esac
jq --arg status "$status" --argjson code "$code" --argjson elapsed "$((SECONDS - tick))" \
  --arg finished "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '. + {status: $status, exit_code: $code, elapsed_seconds: $elapsed, finished_utc: $finished}' <<< "$run" > "$manifest" \
  || { echo "MANIFEST ERROR: cannot write $manifest" >&2; code=2; }
echo "Build exit code: $code; outputs: $output"
exit "$code"
