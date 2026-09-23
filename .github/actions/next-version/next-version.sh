#!/bin/bash
set -euo pipefail

# Computes the next vX.Y.Z tag from Conventional Commits since the last
# release: any "!:" or "BREAKING CHANGE" bumps major, any "feat:" bumps
# minor, any "fix:" bumps patch. No matching commits means no release.

last_release=""

while true; do
    case "${1:-}" in
    "--last-release")
        shift
        last_release="$1"
        shift
        ;;
    *)
        break
        ;;
    esac
done

emit() {
    echo "version=$1" >>"${GITHUB_OUTPUT:-/dev/null}"
    echo "bump=$2" >>"${GITHUB_OUTPUT:-/dev/null}"
}

if [[ -z "$last_release" ]] || [[ ! "$last_release" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "No previous vX.Y.Z tag found (last_release='$last_release'); skipping version bump" >&2
    emit "" "none"
    exit 0
fi

version="${last_release#v}"
IFS='.' read -r major minor patch <<<"$version"

commit_range="HEAD...$last_release"

subjects=()
while IFS= read -r subject; do
    subjects+=("$subject")
done < <(git log --pretty=format:'%s' --ancestry-path "$commit_range")
full_log=$(git log --pretty=format:'%B' --ancestry-path "$commit_range")

bump="none"

breaking_re='^[a-zA-Z]+(\([^)]*\))?!:'
feat_re='^feat(\([^)]*\))?:'
fix_re='^fix(\([^)]*\))?:'

for subject in "${subjects[@]+"${subjects[@]}"}"; do
    if [[ "$subject" =~ $breaking_re ]]; then
        bump="major"
        break
    fi
done

if [[ "$bump" != "major" ]] && grep -q "BREAKING CHANGE" <<<"$full_log"; then
    bump="major"
fi

if [[ "$bump" == "none" ]]; then
    for subject in "${subjects[@]+"${subjects[@]}"}"; do
        if [[ "$subject" =~ $feat_re ]]; then
            bump="minor"
            break
        fi
    done
fi

if [[ "$bump" == "none" ]]; then
    for subject in "${subjects[@]+"${subjects[@]}"}"; do
        if [[ "$subject" =~ $fix_re ]]; then
            bump="patch"
            break
        fi
    done
fi

next_version=""
case "$bump" in
major)
    next_version="v$((major + 1)).0.0"
    ;;
minor)
    next_version="v${major}.$((minor + 1)).0"
    ;;
patch)
    next_version="v${major}.${minor}.$((patch + 1))"
    ;;
esac

echo "Determined bump: $bump -> ${next_version:-<none>}" >&2
emit "$next_version" "$bump"
