#!/usr/bin/env bash
# Checks every action the workflows use, and the actions those use in turn (a composite
# action's steps), against allowed-actions.txt, and that each is pinned to a commit SHA.
# Needs gh, signed in (GH_TOKEN), to read the actions' action.yml.
set -euo pipefail
cd "$(dirname "$0")"

mapfile -t allowed < <(grep -vE '^\s*(#|$)' allowed-actions.txt)

# The `uses:` references in a workflow or action.yml, without local (./) or docker:// ones.
refs() {
  sed -nE 's/^\s*(-\s+)?uses:\s*["'\'']?([^"'\''[:space:]#]+).*/\2/p' | grep -vE '^(\./|docker://)' || true
}

is_allowed() {
  local ref=${1,,} pattern
  [[ $ref == actions/* || $ref == github/* ]] && return 0
  for pattern in "${allowed[@]}"; do
    # shellcheck disable=SC2053 # the pattern is a glob on purpose
    [[ $ref == ${pattern,,} ]] && return 0
  done
  return 1
}

declare -A seen
queue=()
while read -r ref; do queue+=("$ref|workflows"); done < <(cat workflows/*.yml | refs | sort -u)

failed=0
while ((${#queue[@]})); do
  item=${queue[0]}; queue=("${queue[@]:1}")
  ref=${item%%|*} from=${item#*|}
  [[ -n ${seen[$ref]:-} ]] && continue
  seen[$ref]=1

  if [[ $ref == ./.github/workflows/* ]]; then continue; fi
  name=${ref%@*} sha=${ref##*@}
  if ! [[ $sha =~ ^[0-9a-f]{40}$ ]]; then
    echo "::error::$ref (used by $from) isn't pinned to a full commit SHA"; failed=1
  fi
  if ! is_allowed "$ref"; then
    echo "::error::$ref (used by $from) isn't in .github/allowed-actions.txt or the repo's allowed actions"; failed=1
  fi

  # owner/repo[/path] → the action's action.yml (or .yaml) at that commit.
  repo=$(cut -d/ -f1-2 <<< "$name") path=$(cut -s -d/ -f3- <<< "$name")
  yml=""
  for file in action.yml action.yaml; do
    if yml=$(gh api -H 'Accept: application/vnd.github.raw' \
        "repos/$repo/contents/${path:+$path/}$file?ref=$sha" 2>/dev/null); then break; fi
    yml=""
  done
  if [[ -z $yml ]]; then
    echo "::error::couldn't read the action.yml of $ref"; failed=1; continue
  fi
  echo "ok  $ref"
  while read -r inner; do queue+=("$inner|$name"); done < <(refs <<< "$yml")
done
exit "$failed"
