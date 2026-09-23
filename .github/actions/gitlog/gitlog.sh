#!/bin/bash
set -euo pipefail

# Formats the commits since the last vX.Y.Z release tag into changelog lines,
# annotating each with the GitHub login of its author.
# Adapted from https://github.com/juhaku/utoipa/blob/master/.github/actions/gitlog/gitlog.sh
# (pipa releases the whole repo, not one crate at a time, so there is no
# --crate filter here.)

output_file=""
range=""
token=""

while true; do
    case "${1:-}" in
    "--output-file")
        shift
        output_file="$1"
        shift
        ;;
    "--range")
        shift
        range="$1"
        shift
        ;;
    "--token")
        shift
        token="$1"
        shift
        ;;
    *)
        break
        ;;
    esac
done

if [[ "$output_file" == "" ]]; then
    echo "Missing --output-file <file> option argument, define path to file or - for stdout" && exit 1
fi

commit_range=""
last_release=""
if [ -z "$range" ]; then
    from_commit=HEAD
    last_release=$(git tag --sort=-creatordate | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -1)
    echo "Found last release tag: ${last_release:-<none>}"

    if [[ -n "$last_release" ]]; then
        commit_range="$from_commit...$last_release"
    else
        commit_range="$from_commit"
    fi
else
    commit_range="$range"
fi

ancestry_path=""
if [[ -n "$last_release" ]]; then
    ancestry_path="--ancestry-path"
fi

log_lines=()
while IFS= read -r log_line; do
    log_lines+=("$log_line")
done < <(git log --pretty=format:'(%h) %s' $ancestry_path "$commit_range")

get_username() {
    commit="$1"

    args=()
    if [ -n "$token" ]; then
        args=("${args[@]}" "-H" "Authorization: Bearer $token")
    fi

    curl -sSL \
        -H "Accept: application/vnd.github+json" \
        -H "X-GitHub-Api-Version: 2022-11-28" \
        "${args[@]+"${args[@]}"}" \
        "https://api.github.com/repos/${GITHUB_REPOSITORY}/commits/$commit" | jq -r '.author.login // empty'
}

log=""
for line in "${log_lines[@]+"${log_lines[@]}"}"; do
    commit=$(echo "$line" | awk -F ' ' '{print $1}')
    commit=${commit//[\(\)]/}

    # Merge commits add noise without their own meaningful summary; the
    # underlying commits they bring in are already listed individually.
    if [[ "$line" == *"Merge pull request"* ]]; then
        continue
    fi

    user=$(get_username "$commit")
    if [[ -n "$user" ]]; then
        log="$log* $line @$user\n"
    else
        log="$log* $line\n"
    fi
done

if [[ "$output_file" == "-" ]]; then
    echo -e "$log"
else
    echo -e "$log" >"$output_file"
fi

if [[ -z "$last_release" ]]; then
    last_release=$(git rev-list --reverse HEAD | head -1)
fi

if [ -n "${GITHUB_OUTPUT:-}" ]; then
    echo "last_release=$last_release" >>"$GITHUB_OUTPUT"
fi
