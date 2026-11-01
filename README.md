![Skyward Recovery](assets/hero.jpg)

# Skyward for the ThinkPhone (bronco)

A fork of [Skyward](https://github.com/kirklin/skyward) — a recovery written in
Rust, from scratch — for the 2023 Motorola ThinkPhone, `bronco`. Upstream builds
for Xiaomi phones; this fork builds for one phone, and carries the fixes that
phone needed.

## What works on the phone

Verified on an A16 build, `W1TBS36H.1-63-2-9`, with the recovery flashed over
`recovery_b`:

| | |
|---|---|
| Boot to the pages | ~7 s |
| Metadata (DE) decryption | `/data` opens by itself |
| Credential (CE) unlock | the PIN opens the user's files; `/sdcard` mounts |
| WLAN | on, scan, join; the network joined is kept for the next boot |
| File transfer, adb, fastbootd | `mtp,adb` and `fastbootd` both bind |
| Recovery updates | checked, downloaded and written from this fork's releases |
| Settings | kept between boots: brightness, theme, language, screen-off, time zone, vibration, file transfer, WLAN |
| Backup, wipe, installs, audio | not exercised yet |

## Building

The recovery rides on the phone's own recovery image: its init, adbd,
fastbootd, SELinux policy and libraries stay, and only `system/bin/recovery`
becomes Skyward. That base is Motorola's to hand out, so it is not in this
repository — take `recovery_b` out of a stock firmware package:

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk            # and an Android NDK r29 in ANDROID_NDK_HOME

scripts/build-bronco.sh --stock /path/to/stock-recovery_b.img
fastboot flash recovery_b out/skyward-bronco.img
```

The build stamps itself with the time it was made; that stamp is what the
update page compares against the published releases.

## Updating the recovery

The update page asks this fork's releases on GitHub for the newest image of
this phone, downloads it into `/sdcard/Download/Skyward/`, checks it against the
SHA-256 the release publishes, and writes it over the recovery partitions of
both slots — the running one always, and the other when it holds this recovery,
since after a slot switch that one is what boots. Every partition is read back
and compared with what was written.

Nothing is published for this phone yet, so the page says it is the newest one
until a release is made (see `scripts/release.sh`).

## What the recovery remembers

Changed a setting and the recovery writes it a couple of seconds later, whole
and flushed, to the first of these it can:

1. `/mnt/vendor/persist/skyward/` — the partition the phone keeps its own
   calibration in, which the recovery mounts as it comes up and no reset wipes;
2. `/data/skyward/` — opens once the decrypt page is answered;
3. `/sdcard/Skyward/` — waits on the user's screen lock as well.

Which of them is used is decided by what is there: the newest copy is the one
read, and it is written back where it was found. The file is `settings/values`,
one `name = value` per line; a value that is missing, unreadable or out of range
falls back to the default instead of failing. `wlan/wpa_supplicant.conf` beside
it belongs to the supplicant, which writes back the network it joined — the one
joined last, and it joins it again on the next boot.

## What this fork changes

- **The ThinkPhone's own fixes**, each one written against the phone: the
  panel's backlight, the runtime bundle that cannot be looped from the
  recovery's domain, adb kept when a USB configuration is unknown, file
  transfer bound, and the credential service's own answer reported.
- **A recovery that updates itself**, instead of checking the phone maker's
  server for a system update it cannot install.
- **English by default** when a boot names no locale — fastbootd's command
  carries none and its partitions are not mounted yet, so the pages used to
  open in Chinese and stay in it.
- **Settings that survive a restart**, on the owner's storage beside the logs,
  and the WLAN's network kept with them.
- **One device**: the other phones' configurations, the stock OTA check and its
  tests are gone.

## Credit

[Skyward](https://github.com/kirklin/skyward) is KirKlin's, GPL-3.0-or-later,
and does all the work; this fork only adds what one phone needed. The device
configurations of the Samsung/Motorola ports, and the portable recovery work
behind them, come from the AERA project's port of the same phone.
