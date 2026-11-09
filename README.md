![Skyward Recovery](assets/hero.jpg)

# Skyward for the Galaxy S20 FE 5G (r8q)

A fork of [Skyward](https://github.com/kirklin/skyward), a recovery written in
Rust, for the Samsung Galaxy S20 FE 5G (`r8q`, SM-G781B). The pages are English;
their texts live in `locales/en.json`.

This branch builds **without the root methods**: no `--features roots`, no root
page, and no `[[root]]` or `[ksu]` in the device configuration.

## Status

The image rides on the LineageOS recovery for r8q (boot header version 2: a
kernel, a gzip ramdisk, the recovery dtbo and the dtb). It is packed with
`devices/r8q.toml` and `scripts/build-r8q.sh`.

What this first build carries:

- The Skyward screen, touch, keys, MTP, adb and fastbootd
- Mounting the phone's logical partitions from `super`, which r8q keeps
  without slots
- Backup and restore of the boot set, the baseband's partitions and the data
  partition
- Installing zips, partition images and install scripts
- Wiping data and factory reset
- WLAN, through Samsung's own driver and the phone's AIDL `wpa_supplicant`
- The recovery's own updates from the fork's releases

What it does **not** carry yet:

- **Decryption.** The r8q vendor has Samsung's Keymaster 4.0 (HIDL) and
  gatekeeper `mdfpp`, while Skyward's decrypt code speaks KeyMint AIDL. The
  OrangeFox r8q tree shows the services and the property dance a Keymaster 4.0
  path needs; that is a separate change.
- **Second system, snapshots and A/B updates.** r8q is not an A/B phone
  (`ro.build.ab_update=false`, no slot suffix), so the pages for a second
  system, a snapshot merge and a slot switch are not offered.
- **ROM updates.** `[rom]` is omitted; the phone's system is LineageOS, whose
  update feed is not the shape the ROM page reads.

## Build

The image rides on the LineageOS recovery ramdisk, which is not in this
repository. The build script downloads the nightly of 2026-11-07 and checks it
against its SHA-256:

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk

scripts/build-r8q.sh
```

With the recovery image already on disk:

```sh
scripts/build-r8q.sh --stock /path/to/recovery.img
```

The script builds the recovery service without `--features roots`, the packer,
and the second system's first program, which is linked statically, then packs
them onto the LineageOS recovery. The output is `out/skyward-r8q.img` and its
`.sha256`.

It needs an Android NDK r29 or newer in `ANDROID_NDK_HOME` (or under
`~/toolchains` or `/opt`). The ramdisk is packed with gzip, which fits the
82,694,144-byte recovery partition; the stock recovery's own screen resources
are left out, and the UI typeface is a Latin subset of MiSans.

## Flash

The recovery partition is `recovery`; there is no slot suffix. Use Odin or
Heimdall with the phone in download mode (Volume Up + Volume Down, then the
USB cable), or:

```sh
heimdall flash --RECOVERY out/skyward-r8q.img
```

To start the recovery, hold Volume Up + Power while the phone is connected to a
computer by USB.

## Notes

- `persist` carries the secure world's state the screen lock is bound to.
  Restoring an old copy can make the PIN unverifiable. It is deliberately not
  in the baseband backup.
- The build's `[update]` section points at `iljasrb/skyward` and the
  `skyward-r8q.img` asset. Releases are published from `r8q-*` tags by
  `.github/workflows/release.yml`.
- The phone's vendor is Samsung's Android 13 tree; the installed system is
  LineageOS 23.2 (Android 16). The recovery's build properties are taken from
  the installed system once its partitions are mounted, as the secure world
  expects.

## Credit

Skyward is KirKlin's, GPL-3.0-or-later. Device facts come from the LineageOS
trees for [r8q](https://github.com/LineageOS/android_device_samsung_r8q) and
[sm8250-common](https://github.com/LineageOS/android_device_samsung_sm8250-common),
the [OrangeFox r8q tree](https://github.com/ForcePushWarning/android_device_samsung_r8q),
and the [stock recovery unpacked](https://github.com/SavedByLight/android_device_samsung_r8q_stock_recovery_unpacked).
Some fixes were ported from [huime180/skyward](https://github.com/huime180/skyward)
and [Rinrroy/skyward](https://github.com/Rinrroy/skyward).
