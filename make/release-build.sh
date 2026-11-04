#!/usr/bin/env bash
# Release bundle: the exported manufacturing files, the route run that
# produced them and the BOM, collected into one checksummed directory and zip
# for manual archiving. Tagging, pushing and uploading stay manual.
#
#   release-build.sh NAME [--project-root DIR] [--source FILE] [--order-dir DIR]
#                    [--fab-dir DIR] [--runs-dir DIR] [--output-root DIR]
#                    [--lock FILE] [--accept-drift]
#
# Writes OUTPUT-ROOT/NAME/ (fab/, route/, bom/, orders/, RELEASE.md,
# SHA256SUMS) and OUTPUT-ROOT/NAME.zip. Refuses an existing bundle, a dirty
# tracked tree, fab files that fail their checksums, and a fab export without a
# passed route run. Routed inputs that differ from HEAD are refused unless
# --accept-drift, and are then listed in RELEASE.md.
# Requires bash, git, jq, zip and sha256sum (or shasum).
set -o pipefail

die() { echo "RELEASE ERROR: $*" >&2; exit 2; }
sha256() { if command -v sha256sum > /dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
sha256sums() { if command -v sha256sum > /dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }
absolute() { if [ -d "$1" ]; then (cd "$1" && pwd -P); else echo "$(cd "$(dirname "$1")" && pwd -P)/$(basename "$1")"; fi; }

name="" project="$PWD" source_file="board.copper" order_dir="" fab_dir="" runs_dir="" output_root="" lock="" accept_drift=0
while [ $# -gt 0 ]; do
  case "$1" in
    --project-root) project="$2"; shift 2 ;;
    --source) source_file="$2"; shift 2 ;;
    --order-dir) order_dir="$2"; shift 2 ;;
    --fab-dir) fab_dir="$2"; shift 2 ;;
    --runs-dir) runs_dir="$2"; shift 2 ;;
    --output-root) output_root="$2"; shift 2 ;;
    --lock) lock="$2"; shift 2 ;;
    --accept-drift) accept_drift=1; shift ;;
    -*) die "unknown option: $1" ;;
    *) [ -z "$name" ] || die "unexpected argument: $1"; name="$1"; shift ;;
  esac
done
for tool in git jq zip; do command -v "$tool" > /dev/null || die "$tool is required"; done
[ -n "$name" ] || die "a release NAME is required (make release RELEASE=<part>-<rev>)"
[[ "$name" =~ ^[A-Za-z0-9._-]+$ ]] || die "release name may contain only letters, digits, '.', '_' and '-': $name"
project="$(absolute "$project")"
cd "$project" || die "project root missing: $project"
board="$(basename "${source_file%.copper}")"
order_dir="$(absolute "${order_dir:-build/order}")"
runs_dir="$(absolute "${runs_dir:-build/$board/runs}")"
output_root="${output_root:-build/release}"
bundle="$output_root/$name"
[ ! -e "$bundle" ] && [ ! -e "$bundle.zip" ] || die "$bundle already exists; a release bundle is never overwritten"

# The bundle records one commit, so every tracked file must be committed.
commit="$(git rev-parse HEAD 2> /dev/null)" || die "not a git repository: $project"
git diff --quiet HEAD -- || die "tracked files have uncommitted changes; commit them before releasing"

# The manufacturing export: the one directory under the order directory with a
# manifest, unless --fab-dir names it. Hidden directories are the previous
# exports that `copper export-manufacturing --replace` keeps.
if [ -z "$fab_dir" ]; then
  mapfile -t found < <(find "$order_dir" -mindepth 2 -maxdepth 2 -name manifest.json ! -path "$order_dir/.*" 2> /dev/null | sort)
  [ "${#found[@]}" -eq 1 ] || die "expected one manufacturing export under $order_dir, found ${#found[@]}; run 'make order' or pass RELEASE_FAB"
  fab_dir="$(dirname "${found[0]}")"
fi
fab_dir="$(absolute "$fab_dir")"
[ -f "$fab_dir/manifest.json" ] && [ -f "$fab_dir/SHA256SUMS" ] || die "no manifest.json and SHA256SUMS in $fab_dir"
# KiCad rewrites the project settings files whenever the board is opened; they
# play no part in fabrication, so only the other files must still match.
bad="$(cd "$fab_dir" && grep -v -E '\.kicad_(pro|prl)$' SHA256SUMS | sha256sums -c --quiet 2>&1)" \
  || die "manufacturing files no longer match their checksums:"$'\n'"$bad"
pcb_sha="$(jq -r '.input_pcb_sha256 // empty' "$fab_dir/manifest.json")"
[ -n "$pcb_sha" ] || die "$fab_dir/manifest.json records no input_pcb_sha256"

# The route run whose filled board was exported.
run=""
for candidate in "$runs_dir"/*/run.json; do
  [ -f "$candidate" ] || continue
  [ -L "$(dirname "$candidate")" ] && continue
  if [ "$(jq -r '.filled_board_sha256 // empty' "$candidate")" = "$pcb_sha" ]; then run="$(dirname "$candidate")"; break; fi
done
[ -n "$run" ] || die "no route run under $runs_dir produced the exported board (sha256 $pcb_sha)"
[ "$(jq -r .status "$run/run.json")" = passed ] || die "route run $(basename "$run") did not pass"

# Routed inputs that are tracked project files, compared with HEAD.
drift=()
while IFS=$'\t' read -r path routed; do
  case "$path" in .copper-cache/*|/*) continue ;; esac
  git cat-file -e "HEAD:$path" 2> /dev/null || continue
  current="$(git show "HEAD:$path" | sha256 /dev/stdin)"
  [ "$current" = "$routed" ] || drift+=("$path")
done < <(jq -r '.provenance.input_sha256 // {} | to_entries[] | "\(.key)\t\(.value)"' "$run/run.json")
if [ "${#drift[@]}" -gt 0 ] && [ "$accept_drift" -ne 1 ]; then
  die "the board was routed from inputs that differ from HEAD: ${drift[*]}."$'\n'"Route and order again, or confirm the changes leave the fabricated board unchanged and rerun with RELEASE_ACCEPT_DRIFT=1"
fi

mkdir -p "$bundle/route" "$bundle/bom" "$bundle/orders" || die "cannot create $bundle"
cp -a "$fab_dir" "$bundle/fab"
for file in run.json route-report.summary.json kicad-drc.json routing.log fill.log; do
  [ -f "$run/$file" ] && cp "$run/$file" "$bundle/route/"
done
cp "$run"/*.via-process.json "$bundle/route/" 2> /dev/null
for file in "$order_dir"/*.csv "$order_dir"/assembly-report.json; do
  [ -f "$file" ] && cp "$file" "$bundle/bom/"
done
[ -n "$lock" ] && [ -f "$lock" ] && cp "$lock" "$bundle/bom/"

remote="$(git remote get-url origin 2> /dev/null || echo none)"
routed_commit="$(jq -r '.provenance.git_commit // "unknown"' "$run/run.json")"
routed_changes="$(jq -r '.provenance.tracked_changes // ""' "$run/run.json")"
pins="$(awk '$1 == "require" { printf "| `%s` | `%s` |\n", $2, $3 }' copper.mod 2> /dev/null)"
bom_lines=""
[ -f "$bundle/bom/ordering.csv" ] && bom_lines="$(($(wc -l < "$bundle/bom/ordering.csv") - 1))"
{
  echo "# $name"
  echo
  echo "Release bundle for \`$source_file\`, made $(date -u +%Y-%m-%d)."
  echo
  echo "**Not a qualified release** unless stated: \`fab/manifest.json\` records"
  echo "\`qualified_release=$(jq -r .qualified_release "$fab_dir/manifest.json")\`,"
  echo "\`independent_cam=$(jq -r .independent_cam "$fab_dir/manifest.json")\` and"
  echo "\`native_drc=$(jq -r .native_drc "$fab_dir/manifest.json")\`."
  echo
  echo "## Notes"
  echo
  echo "_Add what was ordered, from whom and when, deviations, and what to check on the built boards._"
  echo
  echo "## Source"
  echo
  echo "| | |"
  echo "| --- | --- |"
  echo "| Commit | \`$commit\` |"
  echo "| Remote | \`$remote\` |"
  echo "| Tag (to create) | \`$name\` |"
  echo "| KiCad (export) | $(jq -r '.kicad_version // "unknown"' "$fab_dir/manifest.json") |"
  echo "| Route run | \`$(basename "$run")\`, status passed, finished $(jq -r '.finished_utc // "unknown"' "$run/run.json") |"
  echo "| Routed from | \`$routed_commit\`$([ -n "$routed_changes" ] && echo " with uncommitted changes") |"
  echo "| Exported board SHA-256 | \`$pcb_sha\` |"
  echo
  if [ -n "$pins" ]; then
    echo "Dependencies (\`copper.mod\`, content locked by \`copper.lock\`):"
    echo
    echo "| Module | Version |"
    echo "| --- | --- |"
    echo "$pins"
    echo
  fi
  if [ "${#drift[@]}" -gt 0 ]; then
    echo "**Routed inputs that differ from the tagged commit** (accepted with"
    echo "RELEASE_ACCEPT_DRIFT=1; explain under Notes why the board is unchanged):"
    echo
    for path in "${drift[@]}"; do echo "- \`$path\`"; done
    echo
  fi
  echo "## Contents"
  echo
  echo "| Path | What |"
  echo "| --- | --- |"
  echo "| \`fab/\` | Manufacturing export as made by \`make order\`, with its \`manifest.json\` and \`SHA256SUMS\` |"
  echo "| \`route/\` | Record of the route run that produced the exported board (\`run.json\` binds inputs and outputs by SHA-256) |"
  echo "| \`bom/\` | Ordering and hand-assembly lists$([ -n "$bom_lines" ] && echo " ($bom_lines ordering lines)"), assembly report and lock |"
  echo "| \`orders/\` | Order confirmations (add them) |"
  echo "| \`SHA256SUMS\` | Checksums of every file in this bundle |"
} > "$bundle/RELEASE.md"

(cd "$bundle" && find . -type f ! -name SHA256SUMS | LC_ALL=C sort | while IFS= read -r file; do sha256sums "$file"; done > SHA256SUMS) || die "checksums failed"
(cd "$output_root" && zip -qr "$name.zip" "$name") || die "zip failed"

cat << EOF
Release bundle -> $bundle ($bundle.zip)
Route run $(basename "$run"), commit ${commit:0:12}${drift:+; drift accepted: ${drift[*]}}
Next, by hand:
  1. Fill in the Notes in $bundle/RELEASE.md and add order confirmations to $bundle/orders/.
  2. Refresh the checksums and zip:
       (cd $bundle && find . -type f ! -name SHA256SUMS | LC_ALL=C sort | xargs sha256sum > SHA256SUMS)
       (cd $output_root && rm $name.zip && zip -qr $name.zip $name)
  3. Tag and push: git tag -a $name ${commit:0:12} && git push origin $name
  4. Archive $bundle.zip where releases are kept.
EOF
