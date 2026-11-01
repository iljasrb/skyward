#!/usr/bin/env bash
# Build the Skyward recovery image for the Motorola ThinkPhone (bronco).
#
# Skyward rides on the phone's own recovery image: its init, adbd, fastbootd,
# SELinux policy and libraries stay, and only system/bin/recovery becomes
# Skyward. That base is the stock recovery partition, which is the phone
# maker's to hand out and so is not in this repository:
#
#   scripts/build-bronco.sh --stock /path/to/stock-recovery.img [--out IMG] [--root]
#
# --root compiles FolkPatch's installer in, which is opt-in: without it the image
# is the recovery it has always been. The page it adds is marked beta, and the
# image is named ...-root.img so the two are never taken for each other.
#
# Needs rustup's aarch64-linux-android target, cargo-ndk, and an Android NDK
# r29 or newer in ANDROID_NDK_HOME (or one under ~/toolchains).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

stock=${STOCK_RECOVERY:-}
suffix=
features=
while [ $# -gt 0 ]; do
    case $1 in
        --stock) stock=$2; shift 2 ;;
        --out) out=$2; shift 2 ;;
        --root) suffix=-root; features='--features folkpatch'; shift ;;
        -h|--help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done
out=${out:-$root/out/skyward-bronco$suffix.img}
[ -n "$stock" ] || { echo "no stock recovery: pass --stock (see --help)" >&2; exit 2; }
[ -f "$stock" ] || { echo "no stock recovery at $stock" >&2; exit 1; }

ndk=${ANDROID_NDK_HOME:-}
if [ -z "$ndk" ]; then
    ndk=$( { ls -d "$HOME"/toolchains/android-ndk-* "$HOME"/Android/Sdk/ndk/* 2>/dev/null || true; } | tail -n 1)
fi
[ -n "$ndk" ] || { echo "no NDK: set ANDROID_NDK_HOME" >&2; exit 1; }
export ANDROID_NDK_HOME=$ndk

# The build stamps itself with the time it was made, written as a release's
# publication time comes back from GitHub, which is what the update page
# compares; nothing at runtime parses a time.
export SKYWARD_BUILD=${SKYWARD_BUILD:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}

cargo ndk -t arm64-v8a --platform 30 build --release $features -p skyward
cargo build --release -p skyward-pack

mkdir -p "$(dirname "$out")"
target/release/skyward-pack recovery \
    --stock "$stock" \
    --binary "$root/target/aarch64-linux-android/release/skyward" \
    --config "$root/devices/bronco.toml" \
    --font "$root/resources/fonts/MiSansVF.ttf" \
    --mono "$root/resources/fonts/KirkMono-Regular.ttf" \
    --mono-license "$root/resources/fonts/KirkMono-OFL.txt" \
    --out "$out"

( cd "$(dirname "$out")" && sha256sum "$(basename "$out")" >"$(basename "$out").sha256" )
echo "built $out for build $SKYWARD_BUILD"
echo "flash it with: fastboot flash recovery_<slot> $out"
