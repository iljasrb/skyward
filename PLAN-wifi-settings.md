# Plan: settings and Wi-Fi persistence

## Goal

Wi-Fi credentials live only in the encrypted data partition. Settings stay
configurable per device: the vendor persist partition or the data partition.
A location is used only while it is actually mounted.

## Decisions

- Wi-Fi config is always `/data/skyward/wlan/wpa_supplicant.conf`.
- Settings are `/mnt/vendor/persist/skyward` or `/data/skyward`, chosen by
  `[settings] place = "vendor" | "data"` in the device config. Default is
  `data`; bronco keeps `vendor`.
- No `/sdcard` fallback, no persist fallback for Wi-Fi.
- Do not exclude Wi-Fi config from backups (by request).
- Do not clear Wi-Fi config on factory reset (wipe of `/data` does it).
- Do not re-apply WLAN file modes after the supplicant saves; `/data` is
  encrypted and the file stays in `wifi:wifi` with `0770` on the folder.
- Do not replace the passphrase with a derived PSK; `/data` is the fix.

## Steps

1. [x] Add `[settings] place` to the device config and validate it.
2. [x] Add `kept::VENDOR`, `kept::DATA`, and `kept::mounted`; remove the
   hardcoded persist/data/sdcard list.
3. [x] Read and write settings only from the configured, mounted place;
   update the remembered path from the successful write.
4. [x] After `/data` opens, re-read settings when the device keeps them on
   data. A change made before it opens is not written; it stays in the pages
   for this boot.
5. [x] Keep Wi-Fi config on mounted `/data` only, otherwise in memory; after
   `/data` opens, ask the running WLAN to join its saved networks again (no
   restart: a second supplicant in the same boot dies in its service
   registration), and turn it on if the saved settings say it was on.
6. [x] Add the WLAN page callout: saved networks connect after the data
   partition is decrypted.
7. [x] Run config, kept, UI and Android build checks.

## Validation

- `cargo test -p skyward-config` — 14 passed.
- `cargo test -p skyward-sys --lib kept` — 5 passed.
- `cargo test -p skyward-ui` — 302 passed.
- `ANDROID_NDK_HOME=~/toolchains/android-ndk-r29 cargo ndk -t arm64-v8a --platform 30 check -p skyward` — passed.

## Fixed after the first flash

The first flashed build looked one folder too high: `VENDOR` and `DATA`
became mount points (`/mnt/vendor/persist`, `/data`) but the path join did
not add the `skyward` folder. It read `/mnt/vendor/persist/settings/values`
instead of `/mnt/vendor/persist/skyward/settings/values`, so saved settings
were ignored and the WLAN stayed off. `folder`, `read_in` and `write_in` now
add `skyward`; the tests assert the full path.

## Fixed after the second flash

The first attempt after decryption restarted the WLAN, and that is what made
Wi-Fi go off and the `signal: 11` show up: the new supplicant was started
0.3 s after the old one was killed. See the root cause below.

## Root cause of the supplicant crash (`signal: 11`)

Measured on the last published build (`bronco-20261031-2`) with real taps:
Wi-Fi off/on there works, 4 out of 4, and 3 rapid adb restarts also came up.
So a restart is not what breaks — my earlier "one start per boot" reading was
wrong.

What does kill it, on both builds, is **two supplicants alive at once**:

```
servicemanager: Service '...ISupplicant/default' originally registered from
                PID 673 but it is being registered again from PID 698.
                Bad state? Late death notification?
```

then `signal: 11` (2 of 3 such attempts). That is what this change hit: it
started a WLAN 0.3 s after killing one, and the new process could register on
top of the dying one. The vendor binary also ends with SIGSEGV whenever its
nl80211 init fails (`wpa_supplicant -i wlan9` on no such interface), so a bad
start is silent.

The restart is gone rather than made to wait:

- `Command::Off` is a `DISCONNECT` (with the address given back); the
  process and the interface stay, and the supplicant does not reconnect on
  its own until asked.
- `Command::On` on a WLAN that is already running is `resume()` + scan.
- `stop()` — the old `off()` body — ends the supplicant, and is used only
  when the worker is let go (`Inbox::End`) or the supplicant is gone
  (`lost`).

Verified on the device with the same taps: 3 off/on toggles, one supplicant
throughout, no restart, no `signal: 11`.

## Skipped

- Backup exclusion and factory-reset clearing, per the review decision.
- Dedicated PSK storage and post-save permission rewrites.
- Migrating or deleting an old `persist/skyward/wlan` copy. Existing devices
  keep that credential there until it is removed by hand.
