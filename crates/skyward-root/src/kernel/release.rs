//! What a kernel repository's `sources.env` pins: the version of its release,
//! the kernel it is built on, and the manager branch and revision.

/// What a repository's `sources.env` says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    pub project: Option<String>,
    pub line: Option<String>,
    pub kernel: Option<String>,
    pub manager: Option<String>,
    pub manager_repo: Option<String>,
    pub manager_branch: Option<String>,
}

impl Sources {
    /// The release tag, such as `v1.2.3`.
    pub fn tag(&self) -> Option<String> {
        self.project.as_ref().map(|version| format!("v{version}"))
    }

    /// The boot image's name in the release, from the device's own pattern.
    pub fn image(&self, pattern: &str) -> Option<String> {
        self.project.as_ref().map(|version| pattern.replace("{version}", version))
    }

    /// Where the boot image is downloaded from.
    pub fn image_url(&self, repo: &str, pattern: &str) -> Option<String> {
        Some(format!("https://github.com/{repo}/releases/download/{}/{}", self.tag()?, self.image(pattern)?))
    }

    /// Where the sums file beside the boot image is downloaded from.
    pub fn sums_url(&self, repo: &str, pattern: &str) -> Option<String> {
        Some(format!("{}.sha", self.image_url(repo, pattern)?))
    }

    /// The nightly link of the manager's latest CI build on the branch the
    /// kernel's manifest pins.
    pub fn manager_url(&self) -> Option<String> {
        let repo = self.manager_repo.as_deref()?.trim_end_matches(".git").strip_prefix("https://github.com/")?;
        let branch = self.manager_branch.as_deref()?;
        Some(format!("https://nightly.link/{repo}/workflows/build-manager/{branch}/Manager-release.zip"))
    }

    /// The same manager from the main branch's latest CI, where the pinned
    /// branch has no artifact left.
    pub fn manager_url_main(&self) -> Option<String> {
        let repo = self.manager_repo.as_deref()?.trim_end_matches(".git").strip_prefix("https://github.com/")?;
        Some(format!("https://nightly.link/{repo}/workflows/build-manager/main/Manager-release.zip"))
    }
}

/// Read the `KEY="value"` lines of a repository's `sources.env`.
pub fn sources(text: &str) -> Sources {
    let mut sources = Sources::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim().trim_matches('"');
        match key.trim() {
            "PROJECT_VERSION" => sources.project = Some(value.to_string()),
            "LINEAGE_BRANCH" => sources.line = Some(value.to_string()),
            "KERNEL_REVISION" => sources.kernel = Some(value.to_string()),
            "BAKASU_REVISION" => sources.manager = Some(value.to_string()),
            "BAKASU_REPOSITORY" => sources.manager_repo = Some(value.to_string()),
            "BAKASU_BRANCH" => sources.manager_branch = Some(value.to_string()),
            _ => {}
        }
    }
    sources
}

/// The SHA-256 a sums file gives for `image`, as `sha256sum` writes it.
pub fn sums_sha256(body: &str, image: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let (hash, name) = (parts.next()?, parts.next()?);
        let name = name.trim_start_matches('*');
        (name == image && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> &'static str {
        "PROJECT_VERSION=\"9.9.9\"\n\
         LINEAGE_BRANCH=\"lineage-24.0\"\n\
         KERNEL_REVISION=\"abcdef1234567890\"\n\
         BAKASU_REVISION=\"1234567890abcdef\"\n\
         BAKASU_REPOSITORY=\"https://github.com/owner/manager.git\"\n\
         BAKASU_BRANCH=\"main\"\n"
    }

    #[test]
    fn the_repository_manifest_pins_the_version_and_the_manager() {
        let pinned = sources(manifest());
        assert_eq!(pinned.project.as_deref(), Some("9.9.9"));
        assert_eq!(pinned.line.as_deref(), Some("lineage-24.0"));
        assert_eq!(pinned.kernel.as_deref(), Some("abcdef1234567890"));
        assert_eq!(pinned.manager.as_deref(), Some("1234567890abcdef"));
        assert_eq!(pinned.tag().as_deref(), Some("v9.9.9"));
        assert_eq!(pinned.image("kernel-{version}.img").as_deref(), Some("kernel-9.9.9.img"));
        assert_eq!(
            pinned.image_url("owner/kernel", "kernel-{version}.img").as_deref(),
            Some("https://github.com/owner/kernel/releases/download/v9.9.9/kernel-9.9.9.img")
        );
        assert_eq!(
            pinned.sums_url("owner/kernel", "kernel-{version}.img").as_deref(),
            Some("https://github.com/owner/kernel/releases/download/v9.9.9/kernel-9.9.9.img.sha")
        );
        assert_eq!(
            pinned.manager_url().as_deref(),
            Some("https://nightly.link/owner/manager/workflows/build-manager/main/Manager-release.zip")
        );
        assert_eq!(
            pinned.manager_url_main().as_deref(),
            Some("https://nightly.link/owner/manager/workflows/build-manager/main/Manager-release.zip")
        );
    }

    #[test]
    fn a_manifest_without_a_management_branch_names_no_manager() {
        let pinned = sources("PROJECT_VERSION=\"9.9.9\"\nBAKASU_REPOSITORY=\"https://github.com/owner/manager\"\n");
        assert_eq!(pinned.manager_url(), None);
    }

    #[test]
    fn the_sums_file_gives_the_hash_of_the_image_it_names() {
        let hash = "A".repeat(64);
        let body = format!("{hash}  kernel-9.9.9.img\n{}  other.img\n", "b".repeat(64));
        assert_eq!(sums_sha256(&body, "kernel-9.9.9.img"), Some("a".repeat(64)));
        assert_eq!(sums_sha256(&body, "missing.img"), None);
        assert_eq!(sums_sha256("not a sums file", "image.img"), None);
    }
}
