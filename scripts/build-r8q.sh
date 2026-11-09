#!/usr/bin/env bash
# Build the Skyward recovery image for the Samsung Galaxy S20 FE 5G (r8q).
#
# Skyward rides on the phone's own recovery image: its init, adbd, fastbootd,
# SELinux policy and libraries stay, and only system/bin/recovery becomes
# Skyward. For r8q that base is the LineageOS recovery image, which is boot
# header version 2: a kernel, a gzip ramdisk, the recovery dtbo and the dtb.
#
# This branch builds without the root methods: no --features roots, and the
# device configuration names no root method.
#
#   scripts/build-r8q.sh [--stock /path/to/recovery.img] [--out IMG]
#
# Without --stock the LineageOS recovery image is downloaded and checked
# against the SHA-256 below. Needs rustup's aarch64-linux-android target,
# cargo-ndk, an Android NDK r29 or newer in ANDROID_NDK_HOME (or one under
# ~/toolchains or /opt).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# The LineageOS recovery this image is made on: the nightly of 2026-11-07,
# the last one published for r8q before this branch was made.
stock_url=${STOCK_URL:-https://mirrorbits.lineageos.org/full/r8q/20261107/recovery.img}
stock_sha256=${STOCK_SHA256:-fb9f0c95f32c856268d99fe775e4a8f6e6720080d6364bd5372e15f4ed13856d}
stock=${STOCK_RECOVERY:-}
out=${OUT:-$root/out/skyward-r8q.img}

while [ $# -gt 0 ]; do
    case $1 in
        --stock) stock=$2; shift 2 ;;
        --stock-url) stock_url=$2; shift 2 ;;
        --stock-sha256) stock_sha256=$2; shift 2 ;;
        --out) out=$2; shift 2 ;;
        -h|--help) sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

if [ -z "$stock" ]; then
    stock=$root/out/stock-recovery-r8q.img
    if [ ! -f "$stock" ] || ! echo "$stock_sha256  $stock" | sha256sum -c - >/dev/null 2>&1; then
        mkdir -p "$(dirname "$stock")"
        echo "downloading $stock_url"
        curl -fL --retry 3 -o "$stock.part" "$stock_url"
        echo "$stock_sha256  $stock.part" | sha256sum -c -
        mv "$stock.part" "$stock"
    fi
fi
[ -f "$stock" ] || { echo "no stock recovery at $stock" >&2; exit 1; }
echo "$stock_sha256  $stock" | sha256sum -c - >/dev/null 2>&1 || {
    echo "$stock does not have the SHA-256 this build expects ($stock_sha256)" >&2
    exit 1
}

ndk=${ANDROID_NDK_HOME:-}
if [ -z "$ndk" ]; then
    ndk=$( { ls -d "$HOME"/toolchains/android-ndk-* "$HOME"/Android/Sdk/ndk/* /opt/android-ndk* 2>/dev/null || true; } | tail -n 1)
fi
[ -n "$ndk" ] || { echo "no NDK: set ANDROID_NDK_HOME" >&2; exit 1; }
export ANDROID_NDK_HOME=$ndk

# The build stamps itself with the time it was made, written as a release's
# publication time comes back from GitHub, which is what the update page
# compares; nothing at runtime parses a time.
export SKYWARD_BUILD=${SKYWARD_BUILD:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}
# The binaries keep the path of every file they were built from, for the
# messages a panic prints: the crates from the cargo home, and the workspace's
# own. Left as they are, they name the machine and the user who built the
# image, which is nobody's business once it is published. Remapped to a name
# that says nothing, the longest prefix first.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$root=/build/skyward --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/build/cargo --remap-path-prefix=$HOME=/build"

# No --features roots: this branch is the recovery without the root methods.
cargo ndk -t arm64-v8a --platform 30 build --release -p skyward
cargo build --release -p skyward-pack

# The second system's first program runs before any library is there to
# load, so it is linked statically and built into a target directory of its
# own, where the static flags do not reach the rest.
RUSTFLAGS="$RUSTFLAGS -C target-feature=+crt-static" \
    cargo ndk -t arm64-v8a --platform 30 build --release -p skyward-system-init \
    --target-dir target/system-init

mkdir -p "$(dirname "$out")"
target/release/skyward-pack recovery \
    --stock "$stock" \
    --binary "$root/target/aarch64-linux-android/release/skyward" \
    --system-init "$root/target/system-init/aarch64-linux-android/release/system-init" \
    --config "$root/devices/r8q.toml" \
    --font "$root/resources/fonts/MiSansVF-r8q.ttf" \
    --mono "$root/resources/fonts/KirkMono-Regular.ttf" \
    --mono-license "$root/resources/fonts/KirkMono-OFL.txt" \
    --gzip \
    --remove res \
    --remove system/lib64/librecovery_ui.so \
    --remove system/lib64/librecovery_ui_ext.so \
    --remove system/lib64/libncurses_recovery.so \
    --out "$out"

( cd "$(dirname "$out")" && sha256sum "$(basename "$out")" >"$(basename "$out").sha256" )
echo "built $out for build $SKYWARD_BUILD"
echo "flash it with: fastboot flash recovery $out  (or Odin/Heimdall)"
