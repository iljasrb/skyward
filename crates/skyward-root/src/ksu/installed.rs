//! The manager of the KernelSU family installed in Android, if there is one:
//! its package, the APK it runs from, and which manager it is. A run can
//! take that APK's `ksud` instead of downloading a release, and keep the same
//! APK as the copy to install.

use crate::ksu::Flavor;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The manager installed under `root`, as `/data/app`, the newest where more
/// than one is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub flavor: Flavor,
    pub package: String,
    /// The `base.apk` the manager runs from.
    pub path: PathBuf,
}

/// Find every manager of the family installed under `root`, newest first.
pub fn find_all(root: &Path) -> Vec<Installed> {
    let mut found: Vec<(u64, Installed)> = Vec::new();
    walk(root, 0, &mut |path| {
        if path.file_name().is_some_and(|name| name == "base.apk") {
            let Some(package) = package_of(path) else { return };
            let Some(flavor) = Flavor::of_package(&package) else { return };
            let stamp = path.metadata().map(|m| m.mtime().max(0) as u64).unwrap_or(0);
            found.push((stamp, Installed { flavor, package, path: path.to_path_buf() }));
        }
    });
    found.sort_by(|(a, a_manager), (b, b_manager)| b.cmp(a).then_with(|| a_manager.path.cmp(&b_manager.path)));
    found.into_iter().map(|(_, installed)| installed).collect()
}

/// Find the newest manager of the family installed under `root`.
pub fn find(root: &Path) -> Option<Installed> {
    find_all(root).into_iter().next()
}

/// The package a `base.apk` under `/data/app` belongs to, from the folder it
/// is in: `me.weishu.kernelsu-abc==` names it before the dash.
fn package_of(apk: &Path) -> Option<String> {
    let folder = apk.parent()?.file_name()?.to_str()?;
    let package = folder.split('-').next()?;
    (!package.is_empty()).then(|| package.to_string())
}

/// Visit the files under `dir`, a few folders deep: the APK is under a
/// package's folder, which may itself be under a `~~` group.
fn walk(dir: &Path, depth: usize, visit: &mut dyn FnMut(&Path)) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            walk(&path, depth + 1, visit);
        } else if kind.is_file() {
            visit(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apk(root: &Path, package: &str, group: Option<&str>) -> PathBuf {
        let folder = root.join(group.unwrap_or("")).join(format!("{package}-abc=="));
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("base.apk");
        fs::write(&path, b"apk").unwrap();
        path
    }

    #[test]
    fn the_manager_installed_in_android_is_found_under_its_group() {
        let dir = tempfile::tempdir().unwrap();
        let path = apk(dir.path(), "com.sukisu.ultra", Some("~~AbCdEf"));
        let found = find(dir.path()).expect("the manager");
        assert_eq!(found.flavor, Flavor::SukiSu);
        assert_eq!(found.package, "com.sukisu.ultra");
        assert_eq!(found.path, path);
    }

    #[test]
    fn one_installed_under_an_older_name_is_still_the_same_manager() {
        let dir = tempfile::tempdir().unwrap();
        apk(dir.path(), "com.baka.su", None);
        assert_eq!(find(dir.path()).unwrap().flavor, Flavor::BakaSu);
    }

    #[test]
    fn every_installed_manager_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let ksu = apk(dir.path(), "me.weishu.kernelsu", None);
        let sukisu = apk(dir.path(), "com.sukisu.ultra", Some("~~AbCdEf"));
        let all = find_all(dir.path());
        assert_eq!(all.len(), 2);
        for path in [ksu, sukisu] {
            assert!(all.iter().any(|manager| manager.path == path), "{} is listed", path.display());
        }
    }

    #[test]
    fn no_manager_of_the_family_is_not_another_app() {
        let dir = tempfile::tempdir().unwrap();
        apk(dir.path(), "com.example.other", None);
        assert_eq!(find(dir.path()), None);
    }
}
