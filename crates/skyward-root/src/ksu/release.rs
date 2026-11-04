//! Reading a manager's releases from GitHub: which release to take for the
//! tier chosen, and which of its files is the APK built for this phone.

use crate::ksu::{Tier, Flavor};
use anyhow::{Context, Result};
use serde::Deserialize;

/// A release published for a manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The tag it was published under, such as `v3.3.1`.
    pub tag: String,
    /// When it was published, written as GitHub writes a time.
    pub published: String,
    pub prerelease: bool,
    /// The APK chosen for this phone, with the manager inside it.
    pub asset: Asset,
}

/// One file of a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// The SHA-256 GitHub computed for it, as `sha256:<hex>`, where it says.
    pub digest: Option<String>,
}

impl Asset {
    /// The SHA-256 the download is checked against, lowercased and without
    /// the scheme GitHub writes before it.
    pub fn sha256(&self) -> Option<String> {
        let digest = self.digest.as_deref()?.trim();
        let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
        (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| hex.to_ascii_lowercase())
    }
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

/// Where `flavor`'s releases are listed, newest first.
pub fn listing(flavor: Flavor) -> String {
    listing_repo(flavor.repo())
}

/// Where the releases of `repo` are listed, newest first.
pub fn listing_repo(repo: &str) -> String {
    format!("https://api.github.com/repos/{repo}/releases?per_page=20")
}

/// The newest published release in `body` that carries an APK for an arm64
/// phone, at the tier asked for; none while there is none such. A stable
/// tier passes over prereleases, which is why a manager that has published
/// only prereleases has none.
pub fn pick(body: &str, tier: Tier) -> Result<Option<Release>> {
    let listed: Vec<Listed> = serde_json::from_str(body).context("read the releases GitHub lists")?;
    let mut published: Vec<Listed> = listed
        .into_iter()
        .filter(|r| !r.draft && r.published_at.is_some() && (tier == Tier::Latest || !r.prerelease))
        .collect();
    published.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    for release in published {
        let Some(asset) = release.assets.iter().filter_map(asset).min_by_key(|(rank, _)| *rank).map(|(_, a)| a) else {
            continue;
        };
        if asset.size == 0 {
            continue;
        }
        return Ok(Some(Release {
            tag: release.tag_name,
            published: release.published_at.expect("a release with no time was filtered out"),
            prerelease: release.prerelease,
            asset: Asset {
                name: asset.name.clone(),
                url: asset.browser_download_url.clone(),
                size: asset.size,
                digest: asset.digest.clone(),
            },
        }));
    }
    Ok(None)
}

/// The APK to take from a release, if it has one for this phone, and how
/// likely it is the right one: arm64 or aarch64 first, a universal APK next,
/// any other APK after them.
fn asset(listed: &ListedAsset) -> Option<(u8, &ListedAsset)> {
    let name = listed.name.to_ascii_lowercase();
    if !name.ends_with(".apk") || name.contains("debug") || name.contains("spoofed") {
        return None;
    }
    // A build for another ABI would install, but its `ksud` would not run.
    if ["armeabi", "armv7", "x86", "riscv64"].iter().any(|abi| name.contains(abi)) {
        return None;
    }
    let rank = if name.contains("arm64-v8a") || name.contains("aarch64") {
        0
    } else if name.contains("universal") {
        1
    } else {
        2
    };
    Some((rank, listed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing_json() -> String {
        serde_json::json!([
            {
                "tag_name": "v4.3.0-r1",
                "draft": false,
                "prerelease": false,
                "published_at": "2026-11-03T15:17:47Z",
                "assets": [
                    { "name": "ksud-aarch64-linux-android", "browser_download_url": "https://example.invalid/ksud", "size": 1u64, "digest": "sha256:aa" },
                    { "name": "SukiSU_v4.3.0-r1_41140-release.apk", "browser_download_url": "https://example.invalid/stable.apk", "size": 19693021u64, "digest": format!("sha256:{}", "B".repeat(64)) }
                ]
            },
            {
                "tag_name": "v4.3.0",
                "draft": false,
                "prerelease": false,
                "published_at": "2026-10-29T07:28:35Z",
                "assets": [
                    { "name": "SukiSU_v4.3.0_41100-release-spoofed.apk", "browser_download_url": "https://example.invalid/spoofed.apk", "size": 1u64, "digest": null },
                    { "name": "SukiSU_v4.3.0_41100-release.apk", "browser_download_url": "https://example.invalid/older.apk", "size": 19219289u64, "digest": null }
                ]
            },
            {
                "tag_name": "v4.4.0-rc1",
                "draft": false,
                "prerelease": true,
                "published_at": "2026-11-04T10:00:00Z",
                "assets": [
                    { "name": "ReSukiSU_v4.4.0-rc1_9-arm64-v8a-release.apk", "browser_download_url": "https://example.invalid/arm64.apk", "size": 100u64, "digest": "sha256:cccc" },
                    { "name": "ReSukiSU_v4.4.0-rc1_9-universal-release.apk", "browser_download_url": "https://example.invalid/universal.apk", "size": 200u64, "digest": "sha256:dddd" },
                    { "name": "ReSukiSU_v4.4.0-rc1_9-x86_64-release.apk", "browser_download_url": "https://example.invalid/x86.apk", "size": 300u64, "digest": "sha256:eeee" }
                ]
            },
            {
                "tag_name": "v5.0.0",
                "draft": true,
                "prerelease": false,
                "published_at": "2026-11-05T10:00:00Z",
                "assets": [
                    { "name": "SukiSU_v5-debug.apk", "browser_download_url": "https://example.invalid/debug.apk", "size": 1u64, "digest": null }
                ]
            }
        ])
        .to_string()
    }

    #[test]
    fn the_stable_tier_takes_the_newest_release_that_is_not_a_prerelease() {
        let release = pick(&listing_json(), Tier::Stable).unwrap().expect("a release");
        assert_eq!(release.tag, "v4.3.0-r1");
        assert_eq!(release.asset.name, "SukiSU_v4.3.0-r1_41140-release.apk");
        assert_eq!(release.asset.sha256(), Some("b".repeat(64)));
        assert!(!release.prerelease);
    }

    #[test]
    fn the_latest_tier_includes_a_prerelease_and_prefers_the_arm64_apk() {
        let release = pick(&listing_json(), Tier::Latest).unwrap().expect("a release");
        assert_eq!(release.tag, "v4.4.0-rc1");
        assert_eq!(release.asset.name, "ReSukiSU_v4.4.0-rc1_9-arm64-v8a-release.apk");
        assert!(release.prerelease);
    }

    #[test]
    fn a_manager_with_only_prereleases_has_no_stable_release() {
        let only = serde_json::json!([{
            "tag_name": "v4.2.0-rc3",
            "draft": false,
            "prerelease": true,
            "published_at": "2026-09-22T19:39:16Z",
            "assets": [{ "name": "ReSukiSU_arm64-v8a-release.apk", "browser_download_url": "u", "size": 9u64, "digest": "sha256:abcd" }]
        }])
        .to_string();
        assert_eq!(pick(&only, Tier::Stable).unwrap(), None);
        assert!(pick(&only, Tier::Latest).unwrap().is_some());
    }

    #[test]
    fn a_release_with_no_arm64_apk_is_passed_over() {
        let only = serde_json::json!([{
            "tag_name": "v1",
            "draft": false,
            "prerelease": false,
            "published_at": "2026-09-22T19:39:16Z",
            "assets": [{ "name": "manager-x86_64-release.apk", "browser_download_url": "u", "size": 9u64, "digest": null }]
        }])
        .to_string();
        assert_eq!(pick(&only, Tier::Stable).unwrap(), None);
    }

    #[test]
    fn a_spoofed_or_debug_apk_is_not_taken() {
        let one = serde_json::json!([{
            "tag_name": "v1",
            "draft": false,
            "prerelease": false,
            "published_at": "2026-09-22T19:39:16Z",
            "assets": [
                { "name": "manager-spoofed-release.apk", "browser_download_url": "u1", "size": 9u64, "digest": null },
                { "name": "manager-debug.apk", "browser_download_url": "u2", "size": 9u64, "digest": null },
                { "name": "manager-release.apk", "browser_download_url": "u3", "size": 9u64, "digest": null }
            ]
        }])
        .to_string();
        let release = pick(&one, Tier::Stable).unwrap().expect("a release");
        assert_eq!(release.asset.name, "manager-release.apk");
    }

    #[test]
    fn a_digest_is_only_read_when_it_is_a_sha256() {
        let asset = |digest: Option<&str>| Asset {
            name: "a.apk".into(),
            url: "u".into(),
            size: 1,
            digest: digest.map(str::to_string),
        };
        assert_eq!(asset(Some("sha256:ABCD")).sha256(), None, "too short");
        assert_eq!(asset(Some(&format!("sha256:{}", "A".repeat(64)))).sha256(), Some("a".repeat(64)));
        assert_eq!(asset(None).sha256(), None);
    }
}
