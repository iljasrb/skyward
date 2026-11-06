//! Where the cloud storage is and who it signs in as, as the data
//! partition keeps it.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroize;

/// A WebDAV server and the folder under it the recovery keeps its files in.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// The server's address, such as `https://dav.example.com/remote.php/dav/files/me/`.
    pub url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// A folder under the address, such as `Skyward`; empty for the address
    /// itself.
    #[serde(default)]
    pub folder: String,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("folder", &self.folder)
            .finish()
    }
}

impl Drop for Config {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}

impl Config {
    /// Whether the address and the rest can be used as they stand.
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.url.trim().is_empty(), "the address is empty");
        ensure!(self.url.len() <= 2048, "the address is too long");
        ensure!(!self.url.contains(['\r', '\n', '\0']), "the address holds a control character");
        let url = url::Url::parse(self.url.trim()).context("the address is not a URL")?;
        ensure!(matches!(url.scheme(), "http" | "https"), "the address must start with http:// or https://");
        ensure!(url.host().is_some(), "the address names no host");
        ensure!(url.username().is_empty() && url.password().is_none(), "put the user name in its own field");
        ensure!(self.username.len() <= 256, "the user name is too long");
        ensure!(!self.username.contains(['\r', '\n', '\0']), "the user name holds a control character");
        ensure!(self.password.len() <= 1024, "the password is too long");
        ensure!(!self.password.contains(['\r', '\n', '\0']), "the password holds a control character");
        ensure!(self.folder.len() <= 512, "the folder is too long");
        ensure!(!self.folder.contains(['\r', '\n', '\0']), "the folder holds a control character");
        for part in self.folder.split('/').filter(|p| !p.is_empty()) {
            ensure!(part != "." && part != "..", "the folder may not climb out of the address");
        }
        Ok(())
    }

    /// The address as a URL, with a trailing slash and no query or fragment.
    pub fn base_url(&self) -> Result<url::Url> {
        let mut url = url::Url::parse(self.url.trim()).context("the address is not a URL")?;
        url.set_query(None);
        url.set_fragment(None);
        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }
        Ok(url)
    }

    /// The folder under the address, without leading or trailing slashes.
    pub fn folder(&self) -> &str {
        self.folder.trim_matches('/')
    }

    /// Whether the address is plain HTTP, whose password anyone on the way
    /// can read.
    pub fn plain_http(&self) -> bool {
        self.url.trim().to_ascii_lowercase().starts_with("http://")
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string(self).context("write the cloud settings")
    }

    pub fn from_toml(text: &str) -> Result<Config> {
        let config: Config = toml::from_str(text).context("read the cloud settings")?;
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            url: "https://dav.example.com/remote.php/dav/files/me".into(),
            username: "me".into(),
            password: "secret".into(),
            folder: "/Skyward/".into(),
        }
    }

    #[test]
    fn a_config_round_trips_and_keeps_its_password_out_of_debug() {
        let c = config();
        let text = c.to_toml().unwrap();
        assert!(!format!("{c:?}").contains("secret"));
        assert_eq!(Config::from_toml(&text).unwrap(), c);
    }

    #[test]
    fn the_address_gains_a_trailing_slash_and_the_folder_loses_its_edges() {
        let c = config();
        assert_eq!(c.base_url().unwrap().as_str(), "https://dav.example.com/remote.php/dav/files/me/");
        assert_eq!(c.folder(), "Skyward");
    }

    #[test]
    fn a_folder_that_climbs_out_is_refused() {
        let mut c = config();
        c.folder = "Skyward/../elsewhere".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn plain_http_is_known() {
        let mut c = config();
        assert!(!c.plain_http());
        c.url = "http://192.168.1.2:8080/dav".into();
        assert!(c.plain_http());
    }
}
