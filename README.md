# Skyward for the ThinkPhone (bronco)

A fork of [Skyward](https://github.com/kirklin/skyward) — a recovery written in
Rust — for one phone, the 2023 Motorola ThinkPhone (`bronco`). Verified on A16
`W1TBS36H.1-63-2-9`, flashed over `recovery_b`.

| works | |
|---|---|
| decrypt | the PIN opens `/data` and `/sdcard` (SPSS, keymint, gatekeeper) |
| WLAN | scans, joins, keeps the network and rejoins it by itself |
| settings | kept across restarts, including a boot that started in fastboot |
| backup, restore | the phone's partitions and `/data`, SHA-256 checked end to end |
| installs | zips with an install script and update packages, from storage or `adb sideload` |
| fastbootd | getvar, flash, fetch |
| updates | the update page reads this fork's GitHub releases, checks the SHA-256 and writes the running slot's recovery (and the other when it holds Skyward) |
| haptics, MTP, adb | a click on the motor's own driver, `mtp,adb`, adb |

## Build

The recovery rides on the phone's own recovery ramdisk: its init, adbd,
fastbootd, SELinux policy and libraries stay, and only `system/bin/recovery`
becomes Skyward. That base is not in this repository — use the recovery image
of the ROM the phone runs (`out/target/product/bronco/recovery.img` from a
LineageOS or Infinity-X build), or `recovery_b` out of a stock firmware package:

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk            # and an Android NDK r29 in ANDROID_NDK_HOME

scripts/build-bronco.sh --stock /path/to/stock-recovery_b.img
fastboot flash recovery_b out/skyward-bronco.img
```

Pushing a `bronco-*` tag builds and publishes the image with
`.github/workflows/release.yml`: it takes the recovery partition out of the
`additional_images` package the ROM publishes, checked against the sums
beside it, builds the image on it, and publishes the image and its
`SHA256SUMS`, which is what the update page reads.

The update page offers the newest `bronco-*` release.

## What a restore must not touch

`persist` stays out of the baseband set the backup page takes: it carries the
secure world's storage that the screen lock is bound to. Restoring an older copy
makes gatekeeper answer `-30` to the right PIN (`cmd lock_settings verify --old
…` → `didn't match`), and a factory reset does not help, because it leaves
`persist` alone. If that has happened:

```sh
kill $(pidof sec_nvm) $(pidof spdaemon)
rm -rf /mnt/vendor/persist/secnvm/* /mnt/vendor/persist/iar_db/*
sync
```

Then `persist/data/misc/keystore` if the trustlet still refuses a new
credential, `secdata` + `secdataBackup` only if it is worth the attestation
state, and never the whole `persist`, `spss_a/b` or `misc`.

## Credit

[Skyward](https://github.com/kirklin/skyward) is KirKlin's, GPL-3.0-or-later, and
does the work; this fork adds what one phone needed. The device facts come from
the AERA project's port of the same phone.
