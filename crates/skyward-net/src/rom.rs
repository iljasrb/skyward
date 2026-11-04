//! The system the phone's ROM publishes: the newest full package, where it
//! is, and the build it is, read from the manifest the publisher writes.

use crate::release::Asset;
use anyhow::{ensure, Context, Result};
use serde::Deserialize;

/// The newest system a ROM's feed publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// What the build calls itself, such as `Project Infinity X 4.1 · 01.11.2026`.
    pub build: String,
    /// The start of the build's day, in seconds since 1970, which is what
    /// the installed system's `ro.build.version.incremental` counts.
    pub timestamp: u64,
    /// The package to download.
    pub image: Asset,
}

impl Release {
    /// Whether this release is newer than `installed`, the phone's
    /// `ro.build.version.incremental`. A system that does not write a
    /// number there is taken as older than anything published.
    pub fn is_newer_than(&self, installed: &str) -> bool {
        match installed.trim().parse::<u64>() {
            Ok(installed) => self.timestamp > installed,
            Err(_) => true,
        }
    }
}

#[derive(Deserialize)]
struct Manifest {
    build: String,
    ota: Ota,
}

#[derive(Deserialize)]
struct Ota {
    url: String,
    size: u64,
    sha256: String,
}

/// The two shapes the ROM's publisher writes: the mirror's manifest beside
/// the package, and the OTA feed the system's own updater reads.
#[derive(Deserialize)]
#[serde(untagged)]
enum Feed {
    Mirror(Manifest),
    Ota(OtaFeed),
}

#[derive(Deserialize)]
struct OtaFeed {
    response: Vec<OtaEntry>,
}

#[derive(Deserialize)]
struct OtaEntry {
    filename: String,
    download: String,
    size: u64,
    timestamp: u64,
    #[serde(default)]
    version: Option<String>,
}

/// Read the manifest at `feed` and give the system it names.
pub fn newest(agent: &ureq::Agent, feed: &str) -> Result<Release> {
    let mut response = agent.get(feed).call().with_context(|| format!("ask {feed}"))?;
    let status = response.status();
    ensure!(status.is_success(), "{feed} answered {status}");
    let body = response.body_mut().read_to_string().with_context(|| format!("read the answer of {feed}"))?;
    parse(&body)
}

/// The system `body`, a manifest the publisher writes, names.
pub fn parse(body: &str) -> Result<Release> {
    let feed: Feed = serde_json::from_str(body).context("read the system's update feed")?;
    match feed {
        Feed::Mirror(manifest) => {
            let name = package_name(&manifest.ota.url)?;
            ensure!(manifest.ota.size > 0, "the update feed names an empty package");
            let timestamp = timestamp_of(&manifest.build)
                .with_context(|| format!("the build {:?} has no date in it", manifest.build))?;
            Ok(Release {
                build: manifest.build,
                timestamp,
                image: Asset {
                    name,
                    url: manifest.ota.url,
                    size: manifest.ota.size,
                    digest: Some(manifest.ota.sha256.to_lowercase()),
                },
            })
        }
        Feed::Ota(feed) => {
            let entry = feed.response.into_iter().next().context("the update feed names no build")?;
            ensure!(entry.size > 0, "the update feed names an empty package");
            let name = package_name(&entry.download)?;
            let date = date_in(&entry.filename).unwrap_or_default();
            let build = match entry.version.as_deref().filter(|version| !version.is_empty()) {
                Some(version) => format!("{version} · {date}"),
                None => date.to_string(),
            };
            Ok(Release { build, timestamp: entry.timestamp, image: Asset { name, url: entry.download, size: entry.size, digest: None } })
        }
    }
}

/// The file a package's URL names, without its folder or query.
fn package_name(url: &str) -> Result<String> {
    url.rsplit('/')
        .next()
        .and_then(|name| name.split('?').next())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .context("the update feed names no package")
}

/// The first `dd.mm.yyyy` day written in `text`.
fn date_in(text: &str) -> Option<&str> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.')).find(|token| is_date(token))
}

/// Whether `token` is a `dd.mm.yyyy` day that exists.
fn is_date(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    let [day, month, year] = parts.as_slice() else { return false };
    let (Ok(day), Ok(month), Ok(year)) = (day.parse::<u32>(), month.parse::<u32>(), year.parse::<i64>()) else { return false };
    (1..=12).contains(&month) && (1..=31).contains(&day) && days_from_civil(year, month, day) >= 0
}

/// The start of the first `dd.mm.yyyy` day written in `text`, in seconds
/// since 1970.
fn timestamp_of(text: &str) -> Option<u64> {
    let date = date_in(text)?;
    let parts: Vec<&str> = date.split('.').collect();
    let [day, month, year] = parts.as_slice() else { return None };
    let (day, month, year) = (day.parse().ok()?, month.parse().ok()?, year.parse().ok()?);
    Some(days_from_civil(year, month, day) as u64 * 86_400)
}

/// The day `year-month-day` is, counted from 1970-01-01.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day = i64::from(day);
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> String {
        serde_json::json!({
            "build": "Project Infinity X 4.1 · 01.11.2026",
            "ota": {
                "url": "https://rom.flac.moe/bronco/latest/Project_Infinity-X-4.1-bronco-01.11.2026-VANILLA-UNOFFICIAL.zip",
                "size": 1_866_720_728u64,
                "sha256": "11CC3F4C3C7C73FB4D574A791CBA07BC71E95AD241C6760E99417CFB27BBFA96"
            }
        })
        .to_string()
    }

    fn ota_feed() -> String {
        serde_json::json!({
            "response": [{
                "filename": "Project_Infinity-X-4.1-bronco-01.11.2026-VANILLA-UNOFFICIAL.zip",
                "download": "https://rom.flac.moe/bronco/latest/Project_Infinity-X-4.1-bronco-01.11.2026-VANILLA-UNOFFICIAL.zip",
                "size": 1_866_720_728u64,
                "timestamp": 1_793_556_859u64,
                "version": "4.1",
                "md5": "e9d542571ff14e193afbdecd02321689"
            }]
        })
        .to_string()
    }

    #[test]
    fn the_feed_names_the_package_and_the_day_it_was_built() {
        let r = parse(&manifest()).unwrap();
        assert_eq!(r.build, "Project Infinity X 4.1 · 01.11.2026");
        assert_eq!(r.timestamp, 1_793_491_200, "the start of 2026-11-01 UTC");
        assert_eq!(r.image.name, "Project_Infinity-X-4.1-bronco-01.11.2026-VANILLA-UNOFFICIAL.zip");
        assert_eq!(r.image.size, 1_866_720_728);
        assert_eq!(r.image.digest.as_deref(), Some("11cc3f4c3c7c73fb4d574a791cba07bc71e95ad241c6760e99417cfb27bbfa96"));
    }

    #[test]
    fn the_ota_feed_the_system_updater_reads_is_read_too() {
        let r = parse(&ota_feed()).unwrap();
        assert_eq!(r.build, "4.1 · 01.11.2026");
        assert_eq!(r.timestamp, 1_793_556_859, "the build's own time, which the installed system counts");
        assert_eq!(r.image.name, "Project_Infinity-X-4.1-bronco-01.11.2026-VANILLA-UNOFFICIAL.zip");
        assert_eq!(r.image.digest, None, "the feed has no SHA-256; the sums beside the package are read instead");
    }

    #[test]
    fn a_build_is_newer_than_an_older_number_and_not_than_its_own() {
        let r = parse(&manifest()).unwrap();
        assert!(r.is_newer_than("1792972800"), "a build from the day before");
        assert!(!r.is_newer_than("1793491200"), "the same day");
        assert!(!r.is_newer_than("1793556859"), "a build later that day");
        assert!(r.is_newer_than("CP2A.260605.016"), "a system with no date is older than any published");
    }

    #[test]
    fn a_feed_without_a_date_is_refused() {
        let body = serde_json::json!({
            "build": "Project Infinity X 4.1",
            "ota": { "url": "https://example.invalid/a.zip", "size": 1u64, "sha256": "00" }
        })
        .to_string();
        assert!(parse(&body).is_err());
    }

    #[test]
    fn the_days_between_1970_and_a_build_are_counted_right() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 11, 1), 20_758);
        assert_eq!(timestamp_of("4.1 · 01.11.2026"), Some(1_793_491_200));
    }
}
