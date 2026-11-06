//! The `resetprop` command install scripts call, written here so it needs no
//! `libresetprop.so` and no binary from another recovery. It reads and
//! writes properties through the property service, as `getprop` and
//! `setprop` do.

use anyhow::{bail, ensure, Context, Result};
use std::ffi::CString;
use std::process::Command;
use std::time::Duration;

const USAGE: &str = "\
resetprop - read and change Android properties.

Usage:
  resetprop NAME                 - print the property
  resetprop NAME VALUE           - set the property
  resetprop -n NAME VALUE        - set the property; the flag is accepted
  resetprop -w NAME VALUE        - wait until it has that value, then set it
  resetprop --delete NAME        - empty the property
  resetprop -p                   - print every property";

pub fn run() -> Result<()> {
    let mut delete = false;
    let mut wait = false;
    let mut print_all = false;
    let mut rest = Vec::new();
    let mut flags = true;
    for arg in std::env::args().skip(1) {
        if flags {
            match arg.as_str() {
                "-n" | "-v" => continue,
                "-w" => {
                    wait = true;
                    continue;
                }
                "--delete" => {
                    delete = true;
                    continue;
                }
                "-p" => {
                    print_all = true;
                    continue;
                }
                "-h" | "--help" => {
                    println!("{USAGE}");
                    return Ok(());
                }
                other if other.starts_with('-') => bail!("unknown option {other:?}\n\n{USAGE}"),
                _ => flags = false,
            }
        }
        rest.push(arg);
    }
    if print_all {
        let status = Command::new("getprop").status().context("run getprop")?;
        ensure!(status.success(), "getprop exited with {status}");
        return Ok(());
    }
    let Some(name) = rest.first() else { bail!("{USAGE}") };
    if delete {
        ensure!(rest.len() == 1, "delete takes one property name\n\n{USAGE}");
        set(name, "")?;
        return Ok(());
    }
    match rest.len() {
        1 => println!("{}", get(name)?),
        2 => {
            let value = &rest[1];
            if wait {
                wait_for(name, value)?;
            }
            set(name, value)?;
        }
        _ => bail!("{USAGE}"),
    }
    Ok(())
}

fn get(name: &str) -> Result<String> {
    let name = CString::new(name).context("property name")?;
    let mut value = [0u8; 4096];
    let len = unsafe { libc::__system_property_get(name.as_ptr(), value.as_mut_ptr().cast()) };
    ensure!(len >= 0, "read property");
    Ok(String::from_utf8_lossy(&value[..len as usize]).into_owned())
}

fn set(name: &str, value: &str) -> Result<()> {
    let cname = CString::new(name).context("property name")?;
    let cvalue = CString::new(value).context("property value")?;
    let rc = unsafe { libc::__system_property_set(cname.as_ptr(), cvalue.as_ptr()) };
    ensure!(rc == 0, "set property {name}");
    Ok(())
}

fn wait_for(name: &str, value: &str) -> Result<()> {
    while get(name)? != value {
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
