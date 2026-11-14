#!/usr/bin/env bash
# Flash an Odin tar from Linux, the way Odin does on Windows: the phone's PIT
# names the file each partition expects (recovery takes recovery.img), so the
# files inside the tar are matched by name and checked against the size the
# partition has room for before anything is written.
set -euo pipefail

tarfile=${1:?usage: odin-flash.sh <file.tar> [heimdall flash flags]}
shift
extra=("$@")

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

echo "waiting for the phone in download mode, then reading its PIT"
heimdall print-pit --no-reboot --wait >"$work/pit" 2>&1 || {
    echo "no download-mode phone answered; put it in download mode (Volume Up + Volume Down, then USB)" >&2
    exit 1
}

# What the PIT says about one partition: "blocks name" for the file the tar
# would carry, or nothing when no partition asks for it.
claim() {
    awk -v want="$1" '
        /Partition Block Count:/ {blocks = $4}
        /Partition Name:/ {name = $3}
        /Flash Filename:/ {
            file = $3
            if (file != "" && tolower(file) == tolower(want)) print blocks, name
        }
    ' "$work/pit"
}

members=$(tar -tf "$tarfile" | grep -v '/$')
args=()
for member in $members; do
    base=$(basename "$member")
    read -r blocks part < <(claim "$base") || true
    if [ -z "${part:-}" ]; then
        echo "the PIT has no partition for $base" >&2
        exit 1
    fi
    tar -xf "$tarfile" -C "$work" "$member"
    size=$(stat -c%s "$work/$member")
    room=$((blocks * 4096))
    if [ "$size" -gt "$room" ]; then
        echo "$base is $size bytes; $part holds $room" >&2
        exit 1
    fi
    echo "$base -> $part ($size of $room bytes)"
    args+=("--$part" "$work/$member")
done

[ ${#args[@]} -gt 0 ] || { echo "the tar is empty" >&2; exit 1; }
echo "flashing"
heimdall flash --resume "${args[@]}" "${extra[@]}"
