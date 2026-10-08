#!/bin/bash
set -euo pipefail

# Validates that a PR which changes a crate also releases it in that crate's own
# CHANGELOG.md. For every crates/<name>/ with changes (other than *.md files):
#   1. crates/<name>/CHANGELOG.md must be changed too,
#   2. the `version` in crates/<name>/Cargo.toml must differ from the base branch's,
#   3. the changelog must have a `## [<version>]` heading for that new version.
# A crate that is new in the PR only has to satisfy 1 and 3.
#
# Usage: check-changelog.sh <base-ref>   (e.g. origin/main; needs the full history)

base="${1:?usage: check-changelog.sh <base-ref>}"

changed="$(git diff --name-only "$base"...HEAD)"
failed=0

fail() {
    echo "::error file=$1::$2"
    failed=1
}

for manifest in crates/*/Cargo.toml; do
    dir="$(dirname "$manifest")"
    name="$(basename "$dir")"
    changelog="$dir/CHANGELOG.md"

    # Anything under the crate except Markdown (README, CHANGELOG, ...) counts as a code change.
    if ! grep -E "^$dir/" <<<"$changed" | grep -qvE '\.md$'; then
        continue
    fi

    version="$(grep -m1 '^version' "$manifest" | sed -E 's/^version *= *"([^"]+)".*/\1/')"

    if ! grep -qx "$changelog" <<<"$changed"; then
        fail "$changelog" "crates/$name changed but its CHANGELOG.md did not; add an entry for version $version (or label the PR skip-changelog)."
    elif ! grep -qE "^## \[$version\]" "$changelog"; then
        fail "$changelog" "CHANGELOG.md has no '## [$version]' heading matching the version in $manifest."
    fi

    if base_manifest="$(git show "$base:$manifest" 2>/dev/null)"; then
        base_version="$(grep -m1 '^version' <<<"$base_manifest" | sed -E 's/^version *= *"([^"]+)".*/\1/')"
        if [ "$version" = "$base_version" ]; then
            fail "$manifest" "crates/$name changed but its version is still $base_version; bump it (or label the PR skip-changelog)."
        fi
    fi
done

if [ "$failed" -eq 0 ]; then
    echo "Changelog check passed."
fi
exit "$failed"
