# Flashing images out of a package

Status: implementation plan. No code has been written yet.

This adds one thing Skyward does not have: flashing the images inside a package
(a `payload.bin` OTA zip, or a zip of `.img` files) straight onto the partitions
they belong to, without running the package's own installer. It is the unexciting
subset of AERA's "Fast flash (experimental)": serial, one image at a time,
through code Skyward already has.

It is not an OTA installer and does not replace one.

## What AERA's fast flash is, and why it is not copied

AERA's version (`ReviewFastFlash` → `Job::kFlashPayload` with `payload_direct`)
streams `payload.bin` into the block devices through a bundled engine called
otaripper: no temporary extraction, a worker pool writing several partitions at
once, each image read back and SHA-256'd. It overwrites the **current** slot in
place (no slot switch, no snapshot rollback, no postinstall), refuses while OTA
snapshots exist, requires an ARB acknowledgement, and its own sheet says an
interruption may leave the current slot unbootable.

That is a second installer, with its own trust model, in exchange for speed. It is
not adopted here:

- Skyward's OTA path is the phone's own `update_engine_sideload`
  (`skyward-install/src/ota.rs`), which brings snapshots, rollback, postinstall and
  verification, and switches slots rather than overwriting the booting one.
- Concurrency, payload streaming and a second verification story are a large,
  risky feature for a convenience win.
- A recovery should not be able to brick a slot to save a few minutes.

What is worth having is the repair case: a phone whose other slot is also broken,
or whose snapshot state is wedged, where the only useful thing left is to write
known-good images onto the current slot. That is what this plan builds.

## Scope

### In

- Pick a package, list the images inside it, choose which to write.
- Flash each chosen image serially with `skyward_install::image::flash`: write,
  sync, drop cached pages, read back, compare SHA-256.
- Refuse an image larger than its partition; move an AVB footer to the partition's
  end as `image::flash` already does.
- Say plainly that the package's installer, its compatibility checks, its
  postinstall and its slot switch are not run.

### Out

- A `payload.bin` parser. If the package is an A/B OTA with `payload.bin` inside,
  it is installed through `update_engine_sideload` as today; this plan does not
  read payloads. (Add it only if a real device needs it — see "Later".)
- Concurrency of any kind.
- Slot switching, snapshot handling, ARB checks, postinstall.
- Writing any image whose partition the phone's package metadata marks protected.
- Changing the existing single-image flash (`Effect::Flash`).

## Shape

`crates/skyward-install/src/package.rs` gains reading, not writing: what images a
zip holds, and each one's size and hash.

```rust
/// An image inside a package, and the partition it names.
pub struct Held {
    pub name: &'static str,   // "boot", "dtbo", ...
    pub file: String,         // "boot.img" inside the zip
    pub size: u64,
    pub sha256: String,
}

/// The images `zip` holds that this phone has partitions for.
pub fn held(zip: &Path, partition_dir: &Path) -> Result<Vec<Held>>;
```

`crates/skyward/src/ui.rs` gains one work item and one worker call: flash the
chosen entries from the zip, one at a time, through
`skyward_install::image::flash`, telling the page which entry is under way. No new
crate, no new dependency, no new binary.

The page reuses the existing install progress card and the existing `Pick` is not
shown: each image names its own partition, and both slots get it when the phone
has both, exactly as a picked `.img` does today.

## Tests

- `held` lists only entries whose names are partitions this phone has, and skips
  everything else in the zip.
- An entry larger than its partition refuses by name.
- A flash that fails the readback says which partition may be half written.
- Feature-neutral: no UI change when no package is picked.

Manual device matrix:

- flash one image, then both slots' images, then a mixed selection;
- interrupt mid-write and confirm the next boot falls back to the other slot;
- restore the same images with `fastboot flash` and confirm the phone matches.

## Later

- Payload reading (`payload.bin` + blockmap) — only if a device arrives whose
  other slot cannot boot and whose only firmware is an OTA zip.
- Concurrency — only if a measured flash of a full package is slow enough to
  matter.

## Risks and limits

- The user can write mismatched firmware onto the current slot. Said plainly on
  the page, with the other slot's backstop named.
- No ARB awareness: a package that trips anti-rollback would leave the phone
  worse. Refusing packages that carry `xbl_config` is the cheap guard.
