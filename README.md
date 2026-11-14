![Skyward Recovery](assets/hero.jpg)

# Skyward

Skyward is a recovery written in Rust, from scratch. This is a fork's generic
branch: it carries the code, not one phone's files. Device branches add a
`devices/<codename>.toml`, a build script and a release workflow; the ThinkPhone
(`bronco`) branch is `device/bronco`, and the Galaxy S20 FE 5G (`r8q`) branch is `device/r8q`.


## Fork changes

- Payload/OTA tooling
- Root boot image patching, behind the `roots` feature and off for now
- Recovery updates from the fork's own releases
- Installer compatibility/hardening, with `7za`, `zstd` and `unzip` built in
- Settings persist
- Safety fixes
- Separated locales, with `scripts/locales.py` to take a merge of upstream's
  three-language texts into `locales/en.json` (see `locales/README.md`)
- An older build or security patch is agreed to by a swipe, not refused
- Payload data is checked before it is written, and an install waits on
  update_engine itself rather than on helpers that keep its pipes open
- Install scripts run where a package cannot clear them, and a partition that
  stays read-only says why
- A build.prop that cannot be read is left out instead of stopping the recovery
- Programs another system keeps elsewhere are looked for, and a missing one no
  longer stops the recovery
- Pages a phone cannot use are left out: the ROM update page without a feed,
  the charge limit without its switch, and A/B updates without slots


## Features

- Decrypt the data partition
- Install zips, full OTA packages, payloads and partition images, from internal
  storage, a USB drive or adb sideload
- Take boot, init_boot, vendor_boot, recovery, dtbo and vbmeta images out of a
  system package
- `7za` and `zstd` for install scripts, which ROMs with a super image compressed by zstd use to flash it, and an `unzip` whose `-p` streams the image as it reads it
- Install a second system beside the installed one, and switch between them
- Online system updates
- Skyward's own updates, from its newest release on GitHub
- Backup and restore, to internal storage, a USB drive or the card in the
  microSD tray, held read and written at `/mnt/sdcard`
- Wipe data and factory reset
- Mount partitions, repair and format filesystems
- File manager and terminal
- WLAN
- MTP file transfer, adb and fastbootd
- Reboot to system, recovery, fastboot, bootloader or EDL, and switch slots
- Light and dark themes; English

## Building

Skyward builds with the version of Rust that `rust-toolchain.toml` names, with its
`aarch64-linux-android` target, the Android NDK r29 and
[cargo-ndk](https://github.com/bbqsrc/cargo-ndk).

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk

cargo ndk -t arm64-v8a --platform 30 build --release -p skyward
cargo build --release -p skyward-pack
```

The second system's first program runs before any library is there to load, so it
is built as a static program:

```sh
RUSTFLAGS="-C target-feature=+crt-static" cargo ndk -t arm64-v8a --platform 30 build --release -p skyward-system-init --target-dir target/system-init
```

The service is built for the phone and the packer for the computer. Packing an
image needs the phone's stock recovery and a device configuration; a device
branch supplies both. See `scripts/build-bronco.sh` on the `device/bronco` branch and
`scripts/build-r8q.sh` on the `device/r8q` branch for examples.

Tests and lints run on the computer; what runs only on the phone is linted for
it:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo ndk -t arm64-v8a --platform 30 clippy --workspace --all-targets -- -D warnings
```

The tests whose names start with `on_the_phone` make filesystems, mount them and
set encryption keys, which only a phone does. With a phone in Skyward attached over
adb, its data partition open, `cargo ndk` pushes each test program to the phone and
runs it there, one test at a time:

```sh
cargo ndk -t arm64-v8a --platform 30 test -p skyward -p skyward-block -p skyward-files -p skyward-mtp on_the_phone -- --test-threads=1
```

## Languages

The pages' texts live in `locales/en.json`, one flat key for each text, in a
format Crowdin imports and exports. See `locales/README.md`.

## License

[GPL-3.0-or-later](LICENSE). Skyward is KirKlin's.
