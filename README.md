# Skyward for the ThinkPhone (bronco)

A fork of [Skyward](https://github.com/kirklin/skyward), a recovery written in
Rust, for the Motorola ThinkPhone (`bronco`), based on the fork's generic
`main` branch. The pages are English; their texts live in `locales/en.json`.

## Status

Works on Android 16 `W1TBS36H.1-63-2-9`: decrypt, WLAN, settings, backup and
restore, installs from storage and `adb sideload`, fastbootd, recovery
updates, haptics, MTP and adb. ROM updates are not offered on this branch, and
this build carries no root methods.

## Fork changes

- Payload/OTA tooling
- Recovery updates from the fork's own releases
- Installer compatibility/hardening, with `7za`, `zstd` and `unzip` built in
- Settings persist
- Safety fixes
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
- Light and dark themes
- 15 languages: 简体中文, 繁體中文, English, Türkçe, Русский, Українська, Español,
  Português (Brasil), Bahasa Indonesia, Tiếng Việt, Deutsch, Français, Italiano,
  Polski and 日本語. Skyward starts in the system's language where it speaks it,
  and in English otherwise

## Build

The image rides on the phone's stock recovery ramdisk, which is not in this
repository. Build the recovery service and the packer, then pack the image on
that stock recovery:

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk

scripts/build-bronco.sh --stock /path/to/stock-recovery.img
```

The script builds the recovery service, the packer and the second system's
first program, which is linked statically, and packs them onto the stock
recovery. The output is `out/skyward-bronco.img`.

Tests and lints run on the computer; what runs only on the phone is linted for
it:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo ndk -t arm64-v8a --platform 30 clippy --workspace --all-targets -- -D warnings
```

## Flash

```sh
adb reboot bootloader
fastboot flash recovery_<slot> out/skyward-bronco.img
fastboot reboot recovery
```

Use `recovery_a` or `recovery_b` for the slot the phone runs from.

## Languages

Each language's texts are one file in `crates/skyward-ui/locales`, named by its
locale: `tr.toml`, `pt-BR.toml`. `en.toml` is the reference, with a comment on
where each text shows. The build reads every file and stops on a text that is
missing, or that names a value in braces the English text does not.

To correct a translation, change its text in the language's file. To check how it
looks, draw the pages in that language with `skyward-preview --lang`. With each
page, it says which texts did not fit their place: `cut short in` for a text that
ends in an ellipsis, `set at 80% in` for a label set smaller to fit its key, button
or slider, `past the edge in` for a text that runs off the screen, and `overlap in`
for two texts that run into each other.

## Notes

- `persist` carries the secure world's state the screen lock is bound to.
  Restoring an old copy can make the PIN unverifiable.
- Releases are published from `bronco-*` tags by
  `.github/workflows/release.yml`.

## Credit

Skyward is KirKlin's, GPL-3.0-or-later. Device facts come from the AERA port;
some fixes were ported from [huime180/skyward](https://github.com/huime180/skyward)
and [Rinrroy/skyward](https://github.com/Rinrroy/skyward).
