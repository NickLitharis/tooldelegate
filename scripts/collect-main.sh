#!/usr/bin/env bash
# Collect main-experiment records from every executed root, using only run
# directories referenced by an index. Partial attempts left by an interruption are
# excluded until they have an index entry.
set -euo pipefail

out=${1:-main/records.jsonl}
prefix=${2:-main}
if [[ -e "$out" ]]; then
  echo "refusing to overwrite $out" >&2
  exit 1
fi

dirs=()
for index in "$prefix"/index.jsonl "$prefix"-[0-9]*/index.jsonl; do
  [[ -f "$index" ]] || continue
  mapfile -t part < <(jq -r '.run_dir' "$index")
  dirs+=("${part[@]}")
done

if [[ ${#dirs[@]} -eq 0 ]]; then
  echo "no indexed runs found" >&2
  exit 1
fi
if [[ -n "$(printf '%s\n' "${dirs[@]}" | sort | uniq -d)" ]]; then
  echo "duplicate run directory in indexes" >&2
  exit 1
fi

cargo run --locked --quiet -- collect-records "$out" "${dirs[@]}"
printf 'collected %s records into %s\n' "$(wc -l < "$out")" "$out"
