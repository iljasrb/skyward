# FolkPatch in Skyward

Status: implementation plan. No code has been written yet.

This adds an optional FolkPatch (KernelPatch) root installer to Skyward. A build
without the feature is byte-for-byte the recovery it is today. A build with it
gets a **Root** page that patches the selected `boot` slot(s), saves the original
partitions, and keeps the SuperKey it patched with.

The kernel-side binary is KernelPatch 0.13.9 as repackaged by FolkPatch 115404.
Skyward does not run the FolkPatch recovery ZIP or any install script from the
APK; it invokes the pinned `kptools` directly and writes the image itself. The
manager is the owner's to install in Android, from its own release: nothing of it
is carried, saved or run here.

## Scope

### In

- Patch `boot` with KernelPatch `kpimg`, using a generated or user-entered
  SuperKey.
- Choose Current, Other, or Both slots, default Current, with the existing
  picker.
- Save the untouched `boot` partition of every selected slot before writing.
- Save the SuperKey where the recovery and the user can read it, but other apps
  cannot, and keep a copy beside the run only when the user asks for one.
- A dedicated Root page reached from the home page.
- A compile-time cargo feature that omits the installer, the embedded tools and
  the UI when off.

### Out

- Unpatch, restore, or key-reset UI. Backups are restorable manually.
- Installing the manager from recovery. Recovery has no package manager, and the
  APK is not carried.
- Module, KPM, or root-app management.
- Non-A/B devices.
- `init_boot`, `vendor_boot`, `recovery`, `vbmeta`, or super changes.
- Bootloader unlocking, AVB bypass, or OEM-specific unlock workarounds.
- Network download of a newer manager or KernelPatch.
- OTA integration. A system update overwrites the patched boot; root must be
  installed again afterwards.

## Decisions

| Topic | Decision |
| --- | --- |
| Integration | Cargo feature `folkpatch`, off by default |
| Installer location | New crate `crates/skyward-folkpatch` |
| Assets | Fetched by `build.rs` from the pinned release into a cache, hashes checked, embedded with `include_bytes!` |
| Manager source | FolkPatch 115404, package `me.yuki.folk` |
| Kernel source | The `kpimg`/`kptools` bundled in that APK (its own build; not the KernelPatch release assets) |
| Patch target | `boot_a` / `boot_b` only |
| Slot choice | Existing `Pick::{Current, Other, Both}`, default Current |
| SuperKey | Generated 24-character alphanumeric key, or user-entered 8-63 character alphanumeric key containing letters and digits |
| Key storage | Shown on screen; saved root-only under `/data/local/tmp`; never in shared storage |
| Backups | Full raw partition image per selected slot, plus manifest |
| Restore | Manual `fastboot flash boot_a/boot_b ...`; no v1 UI |
| Storage prerequisite | Owner's internal storage must be open (`Data::storage_open()`) |
| Locked bootloader / pending OTA | Warning only; neither is a refusal |
| Cancellation | None after start; the page cannot be left until the work ends |
| Packer | No changes; its existing AVB fit check protects the recovery partition |

## User flow

1. Boot Skyward recovery.
2. Home -> **Root**.
3. Pick Current, Other, or Both. Current is the default.
4. Pick **Generate**, **Enter** (and type the key), or **Use the last key**.
5. Swipe to install.
6. Skyward checks the battery, kernel config, free space, and that each selected
   `boot` is a stock, unpatched boot image. An image that carries the patch
   already stops the run, and the page then offers to patch over it anyway.
7. It writes the original partitions to
   `/sdcard/Download/Skyward/FolkPatch/<stamp>/backup/`, then patches, verifies,
   flashes, and reads back each slot.
8. The run prints its steps in a log window as it goes; the success page keeps the
   last of them and shows the SuperKey on a line of its own.
9. Reboot Android, open FolkPatch, and enter the SuperKey.

The page shows the key in the monospace face, and offers to keep a copy in internal
storage with the cost said plainly: anything with storage permission can read it,
and the key is root-equivalent. The newest key an earlier run left on this phone
can be reused and shown, so patching again after a stock update keeps the key the
phone had. The root-only file remains available through Skyward's file manager or
terminal.

## Architecture

```text
crates/skyward-folkpatch/        new, optional
  assets/licenses/               GPL-2.0 and GPL-3.0 texts of the pinned releases
  build.rs                       fetch, unpack and SHA-256 check of the two tools
  src/lib.rs                     constants, key, options, outcomes, refusals
  src/install.rs                 preflight, backup, patch, flash
  src/backup.rs                  raw partition backup and manifest
  src/key.rs                     generation and validation

crates/skyward-ui/
  src/root.rs                    Root page, RootState, RootAction
  src/app.rs                     feature-gated Screen/Action/Confirm/Effect/State
  src/pages.rs                   render arm, home row, picker title visibility
  src/search.rs                  feature-gated Root search entry

crates/skyward/
  src/root.rs                    feature-gated worker thread
  src/ui.rs                      effect handling, event draining, MTP withhold
  src/main.rs                    feature-gated module declaration
```

`skyward-folkpatch` depends on `skyward-install` for `image::detect`,
`image::device`, `image::flash` and its readback; it does not duplicate the
flashing code. It does not depend on `skyward-ui`, which keeps the dependency
graph acyclic.

## Cargo feature

`crates/skyward-folkpatch/Cargo.toml` is a normal workspace member.

`crates/skyward/Cargo.toml`:

```toml
[features]
default = []
folkpatch = ["dep:skyward-folkpatch", "skyward-ui/folkpatch"]
```

`crates/skyward-ui/Cargo.toml`:

```toml
[features]
default = []
folkpatch = ["dep:skyward-folkpatch"]
```

`skyward-folkpatch` is an optional dependency in both. Feature off means:

- no `skyward-folkpatch` compile;
- no tools fetched or embedded;
- no `Screen::Root`, `Screen::RootKey`, `Action::Root`, `Confirm::RootPatch`,
  `Effect::Root`, or `State.root`;
- no home row and no search entry.

Feature on is selected only by the builder:

```sh
cargo ndk -t arm64-v8a --platform 30 build --release -p skyward --features folkpatch
```

The packer is unchanged. It receives the feature-on binary and does its existing
AVB partition-fit check, failing clearly if the image no longer fits.

## Installer API

```rust
pub const BOOT: &str = "boot";
pub const KPIMG_VERSION: &str = "0.13.9";

pub struct Options {
    pub device: String,            // the phone's own name
    pub current: Slot,             // the slot the phone started from
    pub slots: Vec<Slot>,          // one or both, deduplicated
    pub battery: Battery,          // the charge the recovery read
    pub key: Key,                  // Generate, or Enter(SuperKey) already validated
    pub repatch: bool,             // patch an image that carries the patch already
    pub partition_dir: PathBuf,    // /dev/block/by-name
    pub storage: PathBuf,          // /sdcard
    pub private: PathBuf,          // /data/local/tmp/skyward-folkpatch
    pub stamp: String,             // "20261101-123456"
}

pub enum Key {
    Generate,
    Enter(SuperKey),
}

pub fn install(options: Options, tell: &mut dyn FnMut(Stage)) -> Outcome;

pub enum Stage {
    Checking,
    Copying { done: u64, total: u64 },
    BackingUp { slot: Slot, done: u64, total: u64 },
    Patching { slot: Slot },
    Verifying { slot: Slot },
    Flashing { slot: Slot, done: u64, total: u64 },
}

pub enum Outcome {
    Installed(Installed),
    Refused(Refusal),
    Failed(Failure),
}
```

`Installed` carries the selected slots, the SuperKey, the key file path, the
backup directory, and whether `CONFIG_KALLSYMS_ALL` was set.

`Failure` carries the stage, the slot if one was being worked on, the reason, the
backups completed so far, the slots that may have been written, and the saved
key if one had already been generated. `Refusal` means nothing was written to a
boot partition.

`Key` has a redacting `Debug` implementation. The key is never logged, never put
in the manifest, and never passed on a command line.

## Preflight and refusals

All checks run before any boot write. Refuse when:

- the phone has no A/B slot, or a selected `boot_a`/`boot_b` is missing;
- the device is not aarch64;
- the battery is below Skyward's install threshold, reusing
  `skyward_install::battery_refusal`;
- a selected partition is not an Android boot image with a kernel, or its
  ramdisk is a recovery's;
- KernelPatch reports the image already patched. `kptools -l` reads a **kernel**,
  not a boot image, so the kernel is taken out of the image first (version 3 and
  4 headers: 4096-byte page, `kernel_size` at offset 8) and listed from there;
- `kptools -f` does not report `CONFIG_KALLSYMS=y`;
- the recovery lacks `/system/bin/linker64` or the libraries `kptools` needs
  (`libc`, `libm`, `libdl`, `libz`);
- the owner's internal storage is not open, or the run directory cannot be
  created;
- there is not enough free space for every selected full partition backup and an
  8 MiB margin;
- the SuperKey is invalid.

`CONFIG_KALLSYMS_ALL` is a warning, not a refusal. FolkPatch supports kernels
without it, but the page says the phone may not boot and points at the backups.

A locked bootloader is also a warning, not a refusal. The patch can be written,
but AVB will not boot it until the bootloader is unlocked. The page keeps the
warning and the backup path prominent.

A pending or merging update (`snapshot_state() != None`) is a warning too. The
install is allowed, but the update may overwrite the patched boot or interact
badly with a snapshot. The page says to finish or cancel the update first and
keeps the backup path prominent.

The preflight also warns when the boot image may already carry another root
solution. Skyward only checks for KernelPatch; Magisk/KernelSU patches are not
detected. The documented requirement is a stock boot image.

## Run layout

```text
/sdcard/Download/Skyward/FolkPatch/<stamp>/
  manifest.txt
  backup/
    boot_a.img
    boot_b.img

/data/local/tmp/skyward-folkpatch/<stamp>/
  superkey.txt                    mode 0600, directory 0700
  install.log                     mode 0600
  tools/                          transient kptools and kpimg
```

`manifest.txt` holds the device codename, current slot, selected slots, kpimg
version/hash, `kptools` hash, backup paths/sizes/hashes, and the key file path. It
never holds the key or a hash of it.

The key is deliberately **not** written in shared storage unless the user asks for
a copy there and is told what it means.
Android's `/sdcard` FUSE layer can expose a root-owned file to apps with storage
permission; a SuperKey is root-equivalent. The success page shows the key, and
the root-only file can be read from Skyward's file manager or terminal. The
manifest points at it.

## Backup

For each selected slot:

1. Open `image::device(partition_dir, "boot", slot)`.
2. Read the whole partition, hashing as it is written to `boot_<letter>.img.part`.
3. `fsync`, read the file back, compare its SHA-256 with the source hash.
4. Rename `.part` to `.img`.
5. Record size and hash in `manifest.txt`.

A backup is never reused or overwritten by a later run. If a run is interrupted,
its files stay; the next run gets a new stamp directory and refuses if the boot
image is already patched, since a key it cannot read may open that patch. The
page then offers to patch over it anyway, for a user who means to; the earlier
run's backup is the way back.

Backups are full partition images, not just the image payload, so
`fastboot flash boot_a boot_a.img` restores exactly what was there.

## Patching

`kptools` and `kpimg` are written from the binary to the transient `tools/`
directory, mode 0700, and removed after the run. `kptools` is a
dynamically linked aarch64 PIE using `/system/bin/linker64`; the preflight checks
that interpreter and its libraries.

For each selected slot:

```text
kptools -p -i /dev/block/by-name/boot_a -k <kpimg> -o <patched> --root-skey-fd <fd>
```

- `--root-skey-fd` is used instead of `-S <key>` so the key never appears in
  `/proc/<pid>/cmdline`. Exactly the key bytes are written to a pipe whose read
  end is inherited by `kptools`; no newline is added. Phase 2 verifies this
  against the pinned binary.
- `-S` semantics are correct for FolkPatch: the boot image keeps a hash of the
  root key, so the manager can change the SuperKey later. The literal `su`
  placeholder from the FolkPatch script is never used.
- The source is the block device itself, as FolkPatch's own script does; no
  temporary copy of the original is needed before patching.

After `kptools` exits successfully:

- require a non-empty output no larger than the partition;
- require `ANDROID!` and a kernel in the output;
- run `kptools -l` on the kernel taken out of the patched image and require
  `patched=true` (same extraction as the preflight);
- require the output's last 64 bytes to be `AVBf` when the source ended that
  way, so the bootloader still finds a footer where it expects one;
- then flash.

The patched image is a boot image, never `init_boot` or `vendor_boot`.

## Flashing

Use the existing `skyward_install::image::flash` for each slot:

- writes the image, syncs, drops the block device's cached pages;
- reads the partition back and compares SHA-256;
- moves an AVB footer to the partition's end when the image is smaller than the
  partition, as fastboot does;
- reports bytes written plus read back through the progress callback.

One slot is patched and flashed at a time, after all backups are complete. This
keeps temporary storage small. If the second slot fails, the first may already
be patched; `Failure.written` says so, and both backups exist.

No boot-control slot is changed. Patching Other does not switch to it; the user
uses the existing Slot page. Patching Both leaves the current slot active.

## The key

After backups, before any boot write:

1. Generate or validate the key.
2. Create the private directory, write `superkey.txt` mode 0600 via `create_new`,
   `fsync`, and read it back.
3. Write `manifest.txt` with paths and hashes only.

If either write fails, boot is untouched. Nothing of the manager is saved: the
owner installs it in Android from its own release and enters the key there.

Generated keys are 24 characters from `[A-Za-z0-9]`, drawn from `/dev/urandom`
with rejection sampling, and rejected unless they contain at least one letter
and one digit. Entered keys are validated against the same rules APatch states:
8-63 characters, ASCII letters and digits, at least one of each, no spaces or
symbols. The crate revalidates whatever the UI passes.

The backups go in the owner's storage, which a locked data partition keeps shut,
so the Root page requires it to be open. If it is not, the page says so and
offers the Decrypt page; it does not offer a run whose backup has nowhere to go.

## UI

### Home

The home page gains one row, **Root**. It opens `Screen::Root`, a hub that says
these are the root methods built into this recovery and lists them; the one there
is is **KernelPatch (FolkPatch)**, which opens `Screen::Apatch`. Search finds it
under KernelPatch, FolkPatch, APatch, root, and pinyin keys.

### KernelPatch (FolkPatch)

One screen, laid out so nothing moves as the run goes:

- **Bundled** card: the KernelPatch release, as FolkPatch builds it, with both
  tool hashes in the card's own text;
- **Partitions**: Current / Other / Both;
- **Key**: Generate / Enter a key / Use the last key (dim when no key is on
  file), and under them the SuperKey card, which shows where the key stands and
  opens the superkey page;
- the page's own footnotes: whether the image carries the patch already, what
  stands in the way, or that nothing does, and the cost of the patch;
- a swipe to start, whose label becomes "patch it again" when the slot already
  carries the patch.

The superkey page types the key on the normal keyboard once *Enter a key* is
chosen, and shows the key the run would use otherwise, with the key an earlier
run made under it.

### The run's page

A page of its own (`Screen::ApatchJob`), so nothing of the setup sits under it:

- **Log**: the window the run prints into, keeping its size from the first line
  to the last, the newest at its foot and older lines falling off the top;
- **Key**: the key it made, in a card of its own, in the monospace face;
- the swipe that follows the run: a copy of the key, or writing the image again
  when a run stopped at one that carries the patch already;
- Done, once it has ended. Back is refused while it runs.

### State and actions

```rust
#[cfg(feature = "folkpatch")]
pub struct RootState {
    pub pick: Pick,
    pub key_mode: KeyMode,       // Generate | Enter
    pub entered: String,
    pub job: RootJob,            // Idle | Running(Option<Stage>) | Finished(Outcome)
}

#[cfg(feature = "folkpatch")]
pub enum RootAction {
    Pick(Pick),
    KeyMode(KeyMode),
    EditKey,
    ShowLast,
}
```

`Action::Root(RootAction)` carries those. `Confirm::RootPatch` starts the work and
`Confirm::RootRepatch` patches over an image that has the patch already.
`Effect::Root { pick, key, repatch }` carries what the runtime needs;
the key has the crate's redacting `Debug`.

`Screen::Root` and `Screen::RootKey` are feature-gated. `App` gains:

- `root_stage(stage)` updating the progress state and printing the step once;
- `root_finished(outcome, now)` storing the result and waking the panel;
- `field_mut` and keyboard handling for `Screen::RootKey`;
- a `Screen::Root` arm in `back` that refuses while running and clears the
  finished result on the way out;
- a `Screen::Root` arm in `pages::render`;
- `Confirm::RootPatch` in `confirm_label` and the confirm handler.

The page does not start if an install is already running. The runtime also guards
this, because the UI state can lag the worker.

## Runtime wiring

`crates/skyward/src/root.rs` owns a small worker:

```rust
struct Worker {
    rx: Receiver<Told>,
    thread: JoinHandle<()>,
}

enum Told {
    Stage(skyward_folkpatch::Stage),
    Finished(skyward_folkpatch::Outcome),
}
```

The worker runs `skyward_folkpatch::install` on its own thread and sends stages.
It has no wake pipe; while it exists, the UI poll timeout is capped at 100 ms,
which is enough for smooth progress and keeps the change small.

`Effect::Root(request)` handling in `ui.rs`:

1. Refuse if an `Installer` or root worker already exists.
2. `withhold(&mut transfer, ...)` so MTP is not serving internal storage while
   the run writes to it.
3. Build `Options` from `/dev/block/by-name`, `/sdcard`,
   `/data/local/tmp/skyward-folkpatch`, and a stamp from the current time.
4. Start the worker; log slot count and outcomes to kmsg, never the key.
5. Drain events every loop into `App::root_stage` / `App::root_finished`.

The root install does not use `Detach`: it writes `boot`, not the system
partitions the running recovery uses. It does not call `Kept` or the recovery
reflash path.

## Failure handling and rollback

- Preflight refusal: nothing written; show why.
- Backup failure: no boot write; show the completed backups.
- Patch or verify failure: no boot write; the key may already be on storage, and
  the page shows the saved key path if one exists.
- Flash failure: that slot may be partially written; mark it in `written`, show
  both backups, and tell the user not to reboot into it.
- Interrupted run: the next run refuses an already-patched image; use the earlier
  backup.

Manual restore, from a computer with fastboot:

```sh
adb pull /sdcard/Download/Skyward/FolkPatch/<stamp>/backup/boot_a.img
adb pull /sdcard/Download/Skyward/FolkPatch/<stamp>/backup/boot_b.img
fastboot flash boot_a boot_a.img
fastboot flash boot_b boot_b.img
fastboot reboot
```

`kptools -u` exists but is not the rollback path; the raw backup is authoritative.

## Pinned upstream

| Asset | Source | Size | SHA-256 |
| --- | --- | ---: | --- |
| `kptools` | FolkPatch 115404 `lib/arm64-v8a/libkptools.so` | 249,040 | `ea06a9a46dc1f210c71a61a9f07556fa9b739c327847c7ab16ccbb46fdb7631c` |
| `kpimg` | FolkPatch 115404 `assets/kpimg` | 430,816 | `7a7aa4b930d1a9d684cbb426a3999f75cb2e2cead26673dcbbc5f70871f01141` |

- FolkPatch 115404: <https://github.com/LyraVoid/FolkPatch/releases/tag/115404>,
  package `me.yuki.folk`, versionCode 115404, versionName `0f3f42ba`.
- KernelPatch 0.13.9: <https://github.com/bmax121/KernelPatch/releases/tag/0.13.9>.
  FolkPatch repackaged that source, but its bundled pair is not byte-identical to
  the KernelPatch 0.13.9 release assets (`kptools-android` is 248,016 bytes,
  `kpimg-android` 296,336), so the tools are taken from the FolkPatch release and
  stay from one release.

`build.rs` fetches that release with `curl` the first time a machine builds this
crate, unpacks `kptools` and `kpimg` from it, and hashes both, failing the build
if one is not what the table above pins. The package is only the carrier: it is
not embedded, saved or run. The fetched package and the two unpacked tools wait in
`$XDG_CACHE_HOME/skyward/folkpatch` (`~/.cache/...` without it), so a later build
reads and re-checks them instead of pulling 17 MiB again, and `cargo clean` does
not throw them away. The tree carries no assets besides the license texts: the
cost is about 680 KiB in the recovery binary only, and a first build on a machine
needs the network.

### Verified against the pinned binary, on a ThinkPhone in recovery

- `kptools --root-skey-fd N` takes the key down an inherited pipe; the key never
  appears in `/proc/<pid>/cmdline`. Checked by patching the real `boot_a` into a
  file in `/tmp` and listing the result.
- `kptools -l` fails on a boot image (`kernel image magic error`) and works on the
  kernel taken out of it: `patched=false` before, `patched=true` after.
- `kptools -p` does **not** refuse an already patched image; it patches again and
  the image grows. The already-patched refusal is therefore this recovery's own
  job.
- `kptools -f` reads the boot image directly and prints `CONFIG_KALLSYMS=y` /
  `CONFIG_KALLSYMS_ALL=y`; bronco has both.
- The output of `-p` is exactly the partition's size, with the AVB footer moved to
  its end, so the size and footer checks above hold.

Note that this was a recovery other than Skyward's; the same probes must be
repeated under a Skyward build before a release, since the tools' libraries and
`/tmp` are the recovery's.

`kptools` is an aarch64 PIE built for Android 33, with `BIND_NOW`. It uses
`__fread_chk`/`__fwrite_chk` from the `LIBC_N` version set, so the recovery's libc
must be at least that new. Current supported devices are all newer. A future
device on an older recovery needs a KernelPatch tools build with a lower
`minSdk`, not a workaround here.

## Compression

UPX 5.3.0 was tested against the pinned assets:

| Input | Original | UPX | Result |
| --- | ---: | ---: | --- |
| `kptools` | 249,040 | 120,924 | 48.6%; runs under the recovery's `linker64` via qemu |
| `kpimg` | 430,816 | — | rejected, not an executable |
| Skyward `recovery` | 7,966,392 | 3,457,064 | 43.4%; not device-tested |

Decision: do not use UPX. The only usable asset win is 128 KB. Packing Skyward
itself would put a decompression stub ahead of the Rust runtime, change crash and
debug behavior, and need its own device matrix; the 4.5 MB saving does not justify
that risk. If a recovery partition is too small, rebuild `kptools` from source with
LTO and strip rather than repacking the pinned release.

The pinned `kptools` stays byte-identical to the release asset, so `build.rs`
continues to check the upstream hash.

## Licensing and security

- KernelPatch is GPL-2.0; FolkPatch and Skyward are GPL-3.0. Include the full
  license texts and source links in `crates/skyward-folkpatch/assets/licenses/`
  and a root `THIRD_PARTY_NOTICES.md`. Release review must confirm that the tools
  remain separate extracted files (mere aggregation), not linked into Skyward.
- The SuperKey is root-equivalent. It is generated from a CSPRNG, never logged,
  never put in argv, never stored in shared storage, and saved mode 0600 in a
  0700 directory.
- Warn, do not refuse, on a locked bootloader: the patched image will not boot
  until it is unlocked. Do not touch `vbmeta`; do not add
  `--disable-verity`/`--disable-verification` guidance.
- Warn, do not refuse, while an OTA is pending: the update can overwrite the
  patched boot. Do not run at the same time as Skyward's installer, a script, or
  a partition job.
- Only `boot` is written. `init_boot`, `vendor_boot`, `recovery`, `dtbo`,
  `vbmeta`, super, and user data are untouched.
- The KernelPatch tools are pinned by hash. Skyward does not execute the
  FolkPatch package's scripts or any downloaded code.
- Recovery adb is already an unauthenticated root shell; the Root page does not
  widen that boundary, but it does make the SuperKey the thing to protect.

## Tests

Feature off:

- `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D
  warnings`;
- `cargo ndk -t arm64-v8a --platform 30 clippy -p skyward -p skyward-install -- -D
  warnings`;
- no Root home row or search entry, and the binary carries no tools.

Feature on:

- `cargo test -p skyward-folkpatch`:
  - generated keys are 24 alphanumeric characters with a letter and a digit;
  - entered keys accept 8-63 alphanumeric with both, and reject empty, short,
    long, spaces, punctuation, digits-only, letters-only, and non-ASCII;
  - slot lists deduplicate and reject empty;
  - backup naming, `.part` rename, manifest contents, and absence of the key;
  - a run directory collision gets a distinct suffix.
- `cargo test -p skyward-ui --features folkpatch`:
  - home row opens Root;
  - default pick is Current, and Other/Both titles follow the slot;
  - Enter opens `RootKey`, types, and validates;
  - confirm is absent while storage is closed, the battery is low, or no A/B
    slot exists; a locked bootloader or pending update shows a warning but keeps
    confirm;
  - starting passes the expected `RootRequest` and slots;
  - stage events update progress; success shows the key and paths; failure shows
    written slots, backups, and restore commands;
  - back is refused while running and works after finishing.
- `cargo ndk ... build --release -p skyward --features folkpatch` and a pack of
  at least one device, checking the existing AVB fit error stays clear.

Manual device matrix:

- current slot only, other slot only, both;
- generated and entered key;
- data locked (no confirm, no writes), then unlocked;
- locked bootloader and pending OTA warnings (confirm stays available), low
  battery refusal;
- boot the patched slot, install the manager in Android, enter the key, confirm
  root;
- restore from backup with fastboot and confirm the stock boot returns;
- power loss mid-run, then confirm the next run refuses already-patched boot,
  offers to patch over it, and points at the earlier backup.

## Rollout

1. **Crate skeleton:** asset fetch and hash check, key/validation, manifest and
   backup code. Feature wiring; no UI. Feature-off build must be unchanged.
2. **Installer:** kptools extraction, FD key, patch/verify, flash/readback, key
   and manifest. Device-test on one unlocked phone before UI work.
3. **UI/runtime:** Root page, home row, search entry, effect, worker, failure and
   success states, tests.
4. **Release gate:** device matrix, per-device image size check, license review,
   README/README.zh-CN build instructions, and a `THIRD_PARTY_NOTICES.md`.
5. **Release policy:** standard images stay feature-off. Root-enabled images are
   built separately (for example with a `-root` suffix) only for devices whose
   recovery partition has room and that passed the matrix. CI gains a compile and
   one-device pack check for the feature-on build; it does not need to publish
   root images until the gate passes.

## Risks and limits

- The tools add about 680 KiB to the recovery binary, and the manager is not
  carried at all. A recovery partition with room for Skyward has room for this;
  the packer's AVB fit check is the hard stop.
- `kptools` is an opaque root-level binary. Pinning its hash and not executing
  the package's scripts limits the trust surface, not the consequences of a bad
  patch.
- OEM bootloader/AVB behavior varies. A locked bootloader is allowed with a
  warning, but the patched image will not boot until it is unlocked; no vbmeta
  workaround is attempted.
- The SuperKey cannot be recovered if both the success screen and the root-only
  file are lost. There is no reset UI in v1.
- A system update overwrites boot and loses root; it must be installed again.
- Other root solutions on the boot image are not detected; a stock boot is
  required.
- No automatic rollback. The raw backup and fastboot commands are the safety
  net.
