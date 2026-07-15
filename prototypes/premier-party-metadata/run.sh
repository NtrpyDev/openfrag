#!/usr/bin/env bash
set -euo pipefail

readonly pinned_commit="ba39cc44cd5abfd7f34df2b3c0a7dd3630048311"
readonly default_root="/tmp/openfrag-party-demoparser"
readonly parser_root="${DEMOPARSER_ROOT:-$default_root}"
readonly script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly patch_file="$script_dir/parser.patch"

if [[ ! -d "$parser_root/.git" ]]; then
  git clone --filter=blob:none https://github.com/LaihoE/demoparser.git "$parser_root"
fi

git -C "$parser_root" cat-file -e "$pinned_commit^{commit}"
current_commit="$(git -C "$parser_root" rev-parse HEAD)"
if [[ "$current_commit" != "$pinned_commit" ]]; then
  if [[ -n "$(git -C "$parser_root" status --short)" ]]; then
    echo "Refusing to replace changes in $parser_root" >&2
    exit 1
  fi
  git -C "$parser_root" checkout --detach "$pinned_commit"
fi

if git -C "$parser_root" apply --check "$patch_file" 2>/dev/null; then
  git -C "$parser_root" apply "$patch_file"
elif ! git -C "$parser_root" apply --reverse --check "$patch_file" 2>/dev/null; then
  echo "The pinned parser checkout has unrelated changes; use a clean DEMOPARSER_ROOT." >&2
  exit 1
fi

if [[ "$#" -eq 0 ]]; then
  set -- "$parser_root/src/parser/test_demo.dem"
fi

cargo run \
  --quiet \
  --manifest-path "$parser_root/src/parser/Cargo.toml" \
  --bin party_probe \
  -- "$@"
