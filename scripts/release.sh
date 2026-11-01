#!/usr/bin/env bash
# Publish a built image as a release of this fork, which is what the recovery's
# update page reads: the newest release that carries the phone's image is the
# one it offers.
#
#   scripts/release.sh /path/to/skyward-bronco.img --tag bronco-20261101
#   scripts/release.sh ... --dry-run          # print what would be published
#
# Needs a token with `contents: write` in GITHUB_TOKEN (a release is created
# over the API, and its files uploaded to the upload URL it returns). Without
# one, the work is printed instead, and the release can be made by hand on
# GitHub: create a tag, attach the image and a SHA256SUMS file.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
image=${1:?usage: release.sh IMAGE --tag TAG [--dry-run]}
shift || true
tag=${TAG:-}
dry=0
repo=${REPO:-iljasrb/skyward}

while [ $# -gt 0 ]; do
    case $1 in
        --tag) tag=$2; shift 2 ;;
        --repo) repo=$2; shift 2 ;;
        --dry-run) dry=1; shift ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

[ -f "$image" ] || { echo "no image at $image" >&2; exit 1; }
name=$(basename "$image")
[ -n "$tag" ] || tag="bronco-$(date -u +%Y%m%d-%H%M)"
sum=$(sha256sum "$image" | cut -d' ' -f1)
size=$(stat -c %s "$image")

work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
printf '%s  %s\n' "$sum" "$name" > "$work/SHA256SUMS"
notes=$(printf 'Skyward for the ThinkPhone (bronco)\n\n%s\n\n%s  %s\n' "$tag" "$sum" "$name")

if [ "$dry" = 1 ] || [ -z "${GITHUB_TOKEN:-}" ]; then
    [ "$dry" = 1 ] || echo "GITHUB_TOKEN is not set: make this release on GitHub instead" >&2
    printf 'would publish %s (%s bytes, sha256 %s) of %s as tag %s:\n' "$name" "$size" "$sum" "$repo" "$tag"
    printf '  POST https://api.github.com/repos/%s/releases\n' "$repo"
    printf '  upload %s and SHA256SUMS\n' "$name"
    exit 0
fi

api=https://api.github.com/repos/$repo/releases
body=$(printf '{"tag_name":"%s","name":"%s","body":"%s","draft":false,"prerelease":false}' \
    "$tag" "$tag" "$(printf '%s' "$notes" | sed 's/\\/\\\\/g; s/"/\\"/g' | awk '{printf "%s\\n", $0}')")
response=$(curl -sf -X POST -H "Authorization: Bearer $GITHUB_TOKEN" -H "Accept: application/vnd.github+json" \
    -d "$body" "$api" || true)
[ -n "$response" ] || { echo "creating the release failed" >&2; exit 1; }
upload=$(printf '%s' "$response" | python3 -c 'import json,sys; print(json.load(sys.stdin)["upload_url"].split("{")[0])')

for f in "$image" "$work/SHA256SUMS"; do
    base=$(basename "$f")
    type=application/octet-stream
    [ "$base" = SHA256SUMS ] && type=text/plain
    echo "uploading $base"
    curl -sf -X POST -H "Authorization: Bearer $GITHUB_TOKEN" -H "Content-Type: $type" \
        --data-binary "@$f" "$upload?name=$base" > /dev/null || { echo "uploading $base failed" >&2; exit 1; }
done

echo "published https://github.com/$repo/releases/tag/$tag"
echo "the update page offers it to any build stamped before its publication time"
