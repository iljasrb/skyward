//! adbd listened to on the network the phone joined, so a computer reaches the recovery with `adb connect` and
//! no cable at all.

use anyhow::Result;

/// The port `adb connect` takes when it is given the phone and no port.
pub const PORT: u16 = skyward_ui::app::ADB_WIFI_PORT;

/// The tables the kernel lists its sockets in.
const TABLES: [&str; 2] = ["/proc/net/tcp", "/proc/net/tcp6"];

/// Ask init for adbd to listen on the network too: the property is where adbd reads the port from, and the
/// restart is what makes it read it.
pub fn listen() -> Result<()> {
    skyward_sys::set_prop("service.adb.tcp.port", &PORT.to_string())?;
    skyward_sys::set_prop("ctl.restart", "adbd")
}

/// Ask init for adbd to listen on the USB port alone again.
pub fn stop() -> Result<()> {
    skyward_sys::set_prop("service.adb.tcp.port", "")?;
    skyward_sys::set_prop("ctl.restart", "adbd")
}

/// Whether adbd holds [`PORT`]: what init made of the restart is read from the kernel rather than taken on
/// trust, since a recovery whose init has no adbd service to restart says nothing of it.
pub fn listening() -> bool {
    TABLES.iter().any(|t| std::fs::read_to_string(t).is_ok_and(|table| listens(&table, PORT)))
}

/// Whether a socket in `table`, as `/proc/net/tcp` writes it, listens on `port`.
fn listens(table: &str, port: u16) -> bool {
    table.lines().skip(1).any(|line| {
        let f: Vec<&str> = line.split_whitespace().collect();
        // `sl local_address rem_address st`, the address and port in hex, and `0A` for a socket that listens.
        f.len() > 3 && f[3] == "0A" && f[1].rsplit_once(':').is_some_and(|(_, hex)| u16::from_str_radix(hex, 16) == Ok(port))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:15B3 00000000:0000 0A 00000000:00000000 00:00000000 00000000  2000        0 1 1 0000000000000000 100 0 0 10 0
   1: 0100007F:0035 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1021        0 2 1 0000000000000000 100 0 0 10 0
   2: 0100007F:15B3 0100007F:C350 01 00000000:00000000 00:00000000 00000000  2000        0 3 1 0000000000000000 20 4 30 10 -1
";

    #[test]
    fn the_port_adbd_listens_on_is_read_from_the_kernel() {
        assert!(listens(TABLE, PORT), "adbd listens on every address");
        assert!(listens("  sl  local_address rem_address   st\n   0: 00000000:15b3 00000000:0000 0A 0:0 00:0 0 0 1\n", PORT), "the kernel writes hex in either case");
        assert!(!listens(TABLE, 4444), "a port nothing listens on");
        assert!(!listens(TABLE, 5556), "the port next to it");
    }

    #[test]
    fn a_socket_connected_to_the_port_is_not_one_listening_on_it() {
        let connected = "  sl  local_address rem_address   st\n   0: 0100007F:15B3 0100007F:C350 01 0:0 00:0 0 0 1\n";
        assert!(!listens(connected, PORT));
    }
}
