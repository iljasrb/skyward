//! The `avbctl` command install scripts call, written here so it needs no
//! `libbase.so` and no binary from another recovery. It turns verification
//! and dm-verity on and off in the slot's top-level vbmeta header.

use anyhow::{bail, Result};
use skyward_block::BY_NAME;
use skyward_install::avb::{self, Header};
use std::path::Path;

const USAGE: &str = "\
avbctl - command-line tool for AVB.

Usage:
  avbctl [--force] COMMAND

Commands:
  avbctl get-verity           - Prints whether verity is enabled in current slot.
  avbctl disable-verity       - Disable verity in current slot.
  avbctl enable-verity        - Enable verity in current slot.
  avbctl get-verification     - Prints whether verification is enabled in current slot.
  avbctl disable-verification - Disable verification in current slot.
  avbctl enable-verification  - Enable verification in current slot.";

pub fn run() -> Result<()> {
    let mut force = false;
    let mut command: Option<String> = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--force" => force = true,
            "get-verity" | "disable-verity" | "enable-verity" | "get-verification" | "disable-verification" | "enable-verification" => {
                if command.replace(arg).is_some() {
                    bail!("more than one command\n\n{USAGE}");
                }
            }
            other => bail!("unknown argument {other:?}\n\n{USAGE}"),
        }
    }
    let Some(command) = command else { bail!("{USAGE}") };
    let suffix = skyward_block::slot_suffix()?.unwrap_or_default();
    let mut header = avb::find(Path::new(BY_NAME), &suffix)?;
    match command.as_str() {
        "get-verification" => println!("verification is {}", said(header.verification_enabled())),
        "get-verity" => println!("verity is {}", said(header.verity_enabled())),
        "enable-verification" | "disable-verification" => {
            let enable = command == "enable-verification";
            set(&mut header, enable, force, Header::verification_enabled, Header::set_verification, "verification")?;
        }
        "enable-verity" | "disable-verity" => {
            let enable = command == "enable-verity";
            set(&mut header, enable, force, Header::verity_enabled, Header::set_verity, "verity")?;
        }
        _ => unreachable!("the command was checked above"),
    }
    Ok(())
}

fn said(enabled: bool) -> &'static str {
    if enabled {
        "enabled"
    } else {
        "disabled"
    }
}

fn set(
    header: &mut Header,
    enable: bool,
    force: bool,
    enabled: fn(&Header) -> bool,
    put: fn(&mut Header, bool),
    what: &str,
) -> Result<()> {
    if enabled(header) == enable {
        println!("{what} is already {}", said(enable));
        return Ok(());
    }
    refuse_locked(force)?;
    put(header, enable);
    header.write()?;
    println!("Successfully {} {what}. Reboot the device for changes to take effect.", said(enable));
    Ok(())
}

fn refuse_locked(force: bool) -> Result<()> {
    if force {
        return Ok(());
    }
    if skyward_block::boot_param("androidboot.vbmeta.device_state")?.as_deref() == Some("locked") {
        eprintln!(
            "Manipulating vbmeta on a LOCKED device will likely cause the\n\
             device to fail booting with little chance of recovery.\n\n\
             If you really want to do this, use the --force option.\n\n\
             ONLY DO THIS IF YOU KNOW WHAT YOU ARE DOING."
        );
        bail!("the device is locked");
    }
    Ok(())
}
