# Skyward for the ThinkPhone (bronco)

A fork of [Skyward](https://github.com/kirklin/skyward), a recovery written in
Rust, for the Motorola ThinkPhone (`bronco`), based on the fork's generic
`main` branch. The pages are English; their texts live in `locales/en.json`.

## Status

Works on Android 16 `W1TBS36H.1-63-2-9`: decrypt, WLAN, settings, backup and
restore, installs from storage and `adb sideload`, fastbootd, recovery
updates, haptics, MTP and adb. ROM updates are not offered on this branch.

## Fork changes

- Payload/OTA tooling
- Root boot image patching
- Recovery updates from the fork's own releases
- Installer compatibility/hardening, with `7za`, `zstd` and `unzip` built in
- Settings persist
- Safety fixes
- Separated locales
- An older build or security patch is agreed to by a swipe, not refused
- Payload data is checked before it is written, and an install waits on
  update_engine itself rather than on helpers that keep its pipes open
- Install scripts run where a package cannot clear them, and a partition that
  stays read-only says why
- A build.prop that cannot be read is left out instead of stopping the recovery
- Programs another system keeps elsewhere are looked for, and a missing one no
  longer stops the recovery

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

## Flash

```sh
adb reboot bootloader
fastboot flash recovery_<slot> out/skyward-bronco.img
fastboot reboot recovery
```

Use `recovery_a` or `recovery_b` for the slot the phone runs from.

## Notes

- `persist` carries the secure world's state the screen lock is bound to.
  Restoring an old copy can make the PIN unverifiable.
- Releases are published from `bronco-*` tags by
  `.github/workflows/release.yml`.

## Credit

Skyward is KirKlin's, GPL-3.0-or-later. Device facts come from the AERA port;
some fixes were ported from [huime180/skyward](https://github.com/huime180/skyward)
and [Rinrroy/skyward](https://github.com/Rinrroy/skyward).
