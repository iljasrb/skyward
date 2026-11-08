//! The recovery's own releases, read from the fork's releases on GitHub: the
//! newest image built for this phone, where to download it, and the SHA-256
//! it is checked against.

use anyhow::{ensure, Context, Result};
use serde::Deserialize;

/// A release published for the recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The tag it was published under, such as `bronco-20261030`.
    pub tag: String,
    /// When it was published, written as GitHub writes a time, such as
    /// `2026-10-30T19:37:12Z`, for the page to show.
    pub published: String,
    /// The recovery image itself.
    pub image: Asset,
    /// The file of SHA-256 sums beside it, where the release has one.
    pub sums: Option<Asset>,
}

/// One file of a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// The SHA-256 GitHub computed for it, where it says.
    pub digest: Option<String>,
}

/// The newest release, as the page shows it: its tag, whether it is newer
/// than the build running, and the image for this phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Newest {
    pub version: String,
    pub newer: bool,
    pub build: Option<Build>,
}

/// The image a release carries for this phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub name: String,
    /// Where the image is downloaded from.
    pub url: String,
    /// The release's own link to it, kept for the page.
    pub link: String,
    pub size: u64,
    /// Its SHA-256, where the release names it; the sums file otherwise.
    pub sha256: String,
    /// Where the file of sums that names the image is, where the digest is
    /// not known.
    pub sums: Option<String>,
}

impl Release {
    /// Whether this release is newer than `build`, the stamp of the running
    /// recovery: the tag it was built from, or the time it was made when it
    /// was not built from one. Tags of this phone carry the day, so they sort
    /// as they are written and nothing here parses a time.
    ///
    /// The time a release is published is not compared: a build is made
    /// before its release is published, so every release would look newer
    /// than the build it was made from, and a phone on the newest one would
    /// be offered it for ever. A build that was never stamped cannot be
    /// compared with anything, and is taken as older than every release.
    pub fn is_newer_than(&self, build: &str) -> bool {
        if build == "unknown" {
            return true;
        }
        match (tag_key(&self.tag), tag_key(build)) {
            (Some(mine), Some(theirs)) => mine > theirs,
            _ => self.tag.as_str() > build,
        }
    }

    /// The SHA-256 the image is to be checked against: the release's own,
    /// or else the line for it in the sums file, once that has been read.
    pub fn sha256(&self, sums: Option<&str>) -> Option<String> {
        if let Some(digest) = &self.image.digest {
            return Some(digest.trim_start_matches("sha256:").to_lowercase());
        }
        sha256_in(sums?, &self.image.name)
    }
}

/// A tag's name, day and build number, so that `-10` comes after `-9`; none
/// for a tag that is not `name-yyyymmdd[-n]`.
fn tag_key(tag: &str) -> Option<(&str, u32, u32)> {
    let (head, last) = tag.rsplit_once('-')?;
    let number: u32 = last.parse().ok()?;
    if last.len() == 8 {
        Some((head, number, 0))
    } else {
        let (name, day) = head.rsplit_once('-')?;
        Some((name, day.parse().ok()?, number))
    }
}

/// The SHA-256 `sums` names for `name`, as `sha256sum` writes a line of it.
pub fn sha256_in(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let (hash, file) = (parts.next()?, parts.next()?);
        let file = file.trim_start_matches('*');
        (file == name && hash.len() == 64).then(|| hash.to_lowercase())
    })
}

/// Why the releases could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unanswered {
    /// The network joined wants a login in a browser first.
    Login,
    /// The network joined does not reach the internet: why.
    Offline(String),
    /// The server could not be asked, or its answer not read: why.
    Failed(String),
}

#[derive(Deserialize)]
struct Listed {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<ListedAsset>,
}

#[derive(Deserialize)]
struct ListedAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

/// Where the releases of `repo` are listed, newest first.
fn listing(repo: &str) -> String {
    format!("https://api.github.com/repos/{repo}/releases?per_page=10")
}

/// The name of the file of sums a release carries, if it does.
const SUMS: &str = "SHA256SUMS";

/// Read `repo`'s releases and take the newest published one that carries
/// `asset`; none while there is none such.
pub fn newest(agent: &ureq::Agent, repo: &str, asset: &str) -> Result<Option<Release>> {
    let url = listing(repo);
    let mut response = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "skyward-recovery")
        .call()
        .with_context(|| format!("ask {url}"))?;
    let status = response.status();
    ensure!(status.is_success(), "{url} answered {status}");
    let body = response.body_mut().read_to_string().with_context(|| format!("read the answer of {url}"))?;
    releases(&body, asset)
}

/// The newest published release in `body` that carries `asset`; none while
/// there is none such.
pub fn releases(body: &str, asset: &str) -> Result<Option<Release>> {
    let listed: Vec<Listed> = serde_json::from_str(body).context("read the releases GitHub lists")?;
    let mut published: Vec<Listed> = listed.into_iter().filter(|r| !r.draft && !r.prerelease && r.published_at.is_some()).collect();
    // The list comes newest first, but a time is what the comparison is on.
    published.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    for release in published {
        let found = |name: &str| release.assets.iter().find(|a| a.name == name).map(|a| Asset {
            name: a.name.clone(),
            url: a.browser_download_url.clone(),
            size: a.size,
            digest: a.digest.clone(),
        });
        let Some(image) = found(asset) else { continue };
        ensure!(image.size > 0, "the release {} names an empty {asset}", release.tag_name);
        return Ok(Some(Release {
            tag: release.tag_name,
            published: release.published_at.expect("a release with no time was filtered out"),
            image,
            sums: found(SUMS),
        }));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing_json() -> String {
        serde_json::json!([
            {
                "tag_name": "bronco-20261101",
                "draft": false,
                "prerelease": false,
                "published_at": "2026-11-01T10:00:00Z",
                "assets": [
                    { "name": "skyward-bronco.img", "browser_download_url": "https://example.invalid/new.img", "size": 104857600u64, "digest": "sha256:ABCDEF" },
                    { "name": "SHA256SUMS", "browser_download_url": "https://example.invalid/SHA256SUMS", "size": 120u64, "digest": null }
                ]
            },
            {
                "tag_name": "bronco-20261030",
                "draft": false,
                "prerelease": false,
                "published_at": "2026-10-30T19:37:12Z",
                "assets": [
                    { "name": "skyward-bronco.img", "browser_download_url": "https://example.invalid/old.img", "size": 104857600u64, "digest": null }
                ]
            },
            {
                "tag_name": "bronco-20261105",
                "draft": true,
                "prerelease": false,
                "published_at": "2026-11-05T10:00:00Z",
                "assets": [
                    { "name": "skyward-bronco.img", "browser_download_url": "https://example.invalid/draft.img", "size": 1u64, "digest": null }
                ]
            }
        ])
        .to_string()
    }

    #[test]
    fn the_newest_published_release_is_taken_and_a_draft_is_not() {
        let r = releases(&listing_json(), "skyward-bronco.img").unwrap().expect("a release");
        assert_eq!(r.tag, "bronco-20261101");
        assert_eq!(r.image.url, "https://example.invalid/new.img");
        assert!(r.sums.is_some());
        assert_eq!(r.sha256(None).as_deref(), Some("abcdef"));
    }

    #[test]
    fn a_stamp_decides_whether_a_release_is_newer() {
        let r = releases(&listing_json(), "skyward-bronco.img").unwrap().expect("a release");
        // The phone built from this release is on the newest one already, even
        // though it was built before the release was published.
        assert!(!r.is_newer_than("bronco-20261101"), "the tag it was built from is not newer");
        assert!(r.is_newer_than("bronco-20261031"));
        assert!(!r.is_newer_than("bronco-20261102"));
        // A build made from no tag compares as a time, which sorts before any tag.
        assert!(r.is_newer_than("2026-10-31T00:00:00Z"));
        // A build that was never stamped is older than anything published.
        assert!(r.is_newer_than("unknown"));
        // The build number is a number, not the text it is written as.
        let mut r = r;
        r.tag = "bronco-20261104-10".into();
        assert!(r.is_newer_than("bronco-20261104-9"));
        assert!(!r.is_newer_than("bronco-20261104-10"));
        assert!(r.is_newer_than("bronco-20261104"));
    }

    #[test]
    fn a_release_without_a_digest_is_checked_against_its_sums_file() {
        let one = serde_json::json!([{
            "tag_name": "t", "draft": false, "prerelease": false, "published_at": "2026-10-30T19:37:12Z",
            "assets": [{ "name": "skyward-bronco.img", "browser_download_url": "u", "size": 4096u64, "digest": null }]
        }])
        .to_string();
        let r = releases(&one, "skyward-bronco.img").unwrap().expect("a release");
        assert_eq!(r.sha256(None), None, "nothing to check against yet");
        let sums = format!("{}  skyward-bronco.img\n{} *other.img\n", "a".repeat(64), "b".repeat(64));
        assert_eq!(r.sha256(Some(&sums)), Some("a".repeat(64)));
        assert_eq!(r.sha256(Some("nonsense\n")), None);
    }

    #[test]
    fn a_release_that_carries_no_image_for_this_phone_is_passed_over() {
        let one = serde_json::json!([{
            "tag_name": "t", "draft": false, "prerelease": false, "published_at": "2026-10-30T19:37:12Z",
            "assets": [{ "name": "nuwa.img", "browser_download_url": "u", "size": 4096u64, "digest": null }]
        }])
        .to_string();
        assert_eq!(releases(&one, "skyward-bronco.img").unwrap(), None);
    }
}
