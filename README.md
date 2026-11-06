![Skyward Recovery](assets/hero.jpg)

# Skyward

Skyward is a recovery written in Rust, from scratch. This is a fork's generic
branch: it carries the code, not one phone's files. Device branches add a
`devices/<codename>.toml`, a build script and a release workflow; the ThinkPhone
(`bronco`) branch is `bronco`.

## Features

- Decrypt the data partition
- Install zips, full OTA packages, payloads and partition images, from internal
  storage, a USB drive or adb sideload
- Take boot, init_boot, vendor_boot, recovery, dtbo and vbmeta images out of a
  system package
- Online system updates
- Backup and restore
- Wipe data and factory reset
- Mount partitions, repair and format filesystems
- File manager and terminal
- WLAN
- MTP file transfer, adb and fastbootd
- Reboot to system, recovery, fastboot, bootloader or EDL, and switch slots
- Light and dark themes; English

## Building

Skyward builds with the stable Rust that `rust-toolchain.toml` names, the
`aarch64-linux-android` target, the Android NDK r29 and
[cargo-ndk](https://github.com/bbqsrc/cargo-ndk).

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk

cargo ndk -t arm64-v8a --platform 30 build --release -p skyward
cargo build --release -p skyward-pack
```

The service is built for the phone and the packer for the computer. Packing an
image needs the phone's stock recovery and a device configuration; a device
branch supplies both. See `scripts/build-bronco.sh` on the `bronco` branch for
an example.

Tests and lints run on the computer; what runs only on the phone is linted for
it:

```sh
cargo test --workspace --features roots
cargo clippy --workspace --all-targets --features roots -- -D warnings
cargo ndk -t arm64-v8a --platform 30 clippy -p skyward -p skyward-install -- -D warnings
```

## Languages

The pages' texts live in `locales/en.json`, one flat key for each text, in a
format Crowdin imports and exports. See `locales/README.md`.

## License

[GPL-3.0-or-later](LICENSE). Skyward is KirKlin's.
