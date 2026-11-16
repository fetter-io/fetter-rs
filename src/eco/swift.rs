//! Swift (SwiftPM) scanner.
//!
//! An origin is a directory containing a `Package.resolved` (a SwiftPM package beside its `Package.swift`, or an Xcode project's or workspace's `xcshareddata/swiftpm`); its site is that file (`SiteKind::Lockfile`). Formats 1 (`object.pins`, `repositoryURL`) and 2–3 (`pins`, `location`, `kind`) are read.
//!
//! OSV's `SwiftURL` ecosystem names a package by its repository URL without scheme or `.git` (`github.com/apple/swift-nio`), case-sensitively, so names are kept as written in `location` (normalized, not lowercased). A pin with a version takes it (OSV matches versions server-side); a branch or revision pin takes its revision SHA, which `osv_match::check_sha_pins` resolves to a tag where the commit is tagged, and is otherwise unaudited. Local pins (`localSourceControl`) are excluded. Registry pins (`kind: registry`, an identity `scope.name` rather than a URL) are kept with that identity, which OSV cannot match, so they are unaudited.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value as JsonValue;

use crate::eco::precommit::repo_name;
use crate::eco::Detect;
use crate::eco::DirItem;
use crate::eco::EcosystemScanner;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::origin_site::SiteKind;
use crate::osv_match::is_sha;
use crate::package::Package;
use crate::package_durl::DirectURL;
use crate::package_durl::VcsInfo;
use crate::path_shared::PathShared;
use crate::util::path_normalize;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::ScanConfig;
use crate::version_spec::VersionSpec;

const PACKAGE_RESOLVED: &str = "Package.resolved";
const PACKAGE_SWIFT: &str = "Package.swift";
/// SwiftPM's build directory, beside `Package.swift`; its `checkouts` hold dependencies' sources and resolved files.
const BUILD_DIR: &str = ".build";

/// True if a Swift package's name and version can be queried in OSV: a repository URL name (not a registry identity) and a version (not a revision SHA).
pub(crate) fn is_queryable(name: &str, version: &str) -> bool {
    name.contains('/') && !is_sha(version)
}

fn pin_package(pin: &JsonValue) -> Option<Package> {
    let kind = pin.get("kind").and_then(JsonValue::as_str);
    if kind == Some("localSourceControl") {
        return None;
    }
    let state = pin.get("state")?;
    let field = |k: &str| {
        state
            .get(k)
            .and_then(JsonValue::as_str)
            .filter(|s| !s.is_empty())
    };
    let revision = field("revision");
    let version = field("version");
    let location = pin
        .get("location")
        .or_else(|| pin.get("repositoryURL"))
        .and_then(JsonValue::as_str);
    if kind == Some("registry") {
        // a Swift package registry identity (`scope.name`)
        let identity = pin.get("identity").and_then(JsonValue::as_str)?;
        return Some(Package::new(
            Ecosystem::Swift,
            identity,
            VersionSpec::new(version.unwrap_or_default()),
            None,
        ));
    }
    let name = repo_name(location?)?;
    let direct_url = revision.map(|r| DirectURL {
        url: format!("https://{name}"),
        vcs_info: Some(VcsInfo {
            commit_id: r.to_string(),
            vcs: "git".to_string(),
            revision: version.or(field("branch")).map(str::to_string),
        }),
    });
    let version = version.or(revision)?;
    Some(Package::new(
        Ecosystem::Swift,
        &name,
        VersionSpec::new(version),
        direct_url,
    ))
}

/// The pins of a `Package.resolved` (format 1, 2 or 3).
fn packages_from_resolved(content: &str) -> Vec<Package> {
    let Ok(v) = serde_json::from_str::<JsonValue>(content) else {
        return Vec::new();
    };
    let pins = v
        .get("pins")
        .or_else(|| v.get("object").and_then(|o| o.get("pins")))
        .and_then(JsonValue::as_array);
    let unique: HashSet<Package> =
        pins.into_iter().flatten().filter_map(pin_package).collect();
    unique.into_iter().collect()
}

fn origin(dir: PathBuf) -> Origin {
    Origin::new(Ecosystem::Swift, PathShared::from(dir))
}

pub(crate) struct SwiftScanner;

impl EcosystemScanner for SwiftScanner {
    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect {
        let has = |name: &str| items.iter().any(|i| i.file_name() == Some(name));
        Detect {
            origins: if has(PACKAGE_RESOLVED) {
                vec![origin(dir.to_path_buf())]
            } else {
                Vec::new()
            },
            prune: false,
            skip_children: if has(PACKAGE_SWIFT) {
                vec![BUILD_DIR]
            } else {
                Vec::new()
            },
        }
    }

    fn accepts(&self, path: &Path) -> bool {
        let Ok(p) = path_normalize(path, false) else {
            return false;
        };
        if p.is_file() {
            return p
                .file_name()
                .is_some_and(|n| n == PACKAGE_RESOLVED || n == PACKAGE_SWIFT);
        }
        p.is_dir() && p.join(PACKAGE_RESOLVED).is_file()
    }

    fn normalize_origin(&self, path: &Path) -> ResultDynError<Origin> {
        let p = path_normalize(path, true)?;
        let dir = if p.is_file() {
            p.parent()
                .ok_or("Cannot get parent directory")?
                .to_path_buf()
        } else {
            p
        };
        if dir.join(PACKAGE_RESOLVED).is_file() {
            Ok(origin(dir))
        } else {
            Err(format!(
                "No Package.resolved found in {dir:?}; run `swift package resolve`"
            )
            .into())
        }
    }

    fn origin_to_sites(
        &self,
        origin: &Origin,
        _config: &ScanConfig,
        _log: FlagLog,
    ) -> Vec<Site> {
        let fp = origin.as_path().join(PACKAGE_RESOLVED);
        if fp.is_file() {
            vec![Site::new(
                Ecosystem::Swift,
                SiteKind::Lockfile,
                PathShared::from(fp),
            )]
        } else {
            Vec::new()
        }
    }

    fn site_to_packages(&self, site: &Site) -> Vec<Package> {
        fs::read_to_string(site.as_path())
            .map(|c| packages_from_resolved(&c))
            .unwrap_or_default()
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const V3: &str = r#"{
  "originHash" : "1c9c",
  "pins" : [
    {"identity" : "sparkle", "kind" : "remoteSourceControl",
     "location" : "https://github.com/sparkle-project/Sparkle",
     "state" : {"revision" : "5581748cef2bae787496fe6d61139aebe0a451f6", "version" : "2.8.1"}},
    {"identity" : "commander", "kind" : "remoteSourceControl",
     "location" : "https://github.com/steipete/Commander.git",
     "state" : {"revision" : "9e349575c8e3c6745e81fe19e5bb5efa01b078ce", "version" : "0.2.1"}},
    {"identity" : "peekaboo", "kind" : "remoteSourceControl",
     "location" : "https://github.com/steipete/Peekaboo.git",
     "state" : {"branch" : "main", "revision" : "bace59f90bb276f1c6fb613acfda3935ec4a7a90"}},
    {"identity" : "local", "kind" : "localSourceControl", "location" : "/Users/x/local", "state" : {}},
    {"identity" : "mona.linkedlist", "kind" : "registry", "location" : "", "state" : {"version" : "1.2.0"}}
  ],
  "version" : 3
}"#;

    const V1: &str = r#"{
  "object": {"pins": [
    {"package": "swift-nio", "repositoryURL": "https://github.com/apple/swift-nio.git",
     "state": {"branch": null, "revision": "a5db99e37b8c4f1e4f6d1fa6a7eef4ac4c0a6c9b", "version": "2.0.0"}}
  ]},
  "version": 1
}"#;

    fn names(packages: &[Package]) -> Vec<String> {
        let mut out: Vec<String> = packages.iter().map(|p| p.to_string()).collect();
        out.sort();
        out
    }

    #[test]
    fn test_packages_from_resolved_v3_a() {
        let packages = packages_from_resolved(V3);
        assert_eq!(
            names(&packages),
            vec![
                "github.com/sparkle-project/Sparkle-2.8.1",
                "github.com/steipete/Commander-0.2.1",
                "github.com/steipete/Peekaboo-bace59f90bb276f1c6fb613acfda3935ec4a7a90",
                "mona.linkedlist-1.2.0",
            ]
        );
        let branch = packages
            .iter()
            .find(|p| p.name.ends_with("Peekaboo"))
            .unwrap();
        let vcs = branch
            .direct_url
            .as_ref()
            .unwrap()
            .vcs_info
            .as_ref()
            .unwrap();
        assert_eq!(vcs.revision.as_deref(), Some("main"));
        assert!(!is_queryable(&branch.name, &branch.version.to_string()));
        assert!(!is_queryable("mona.linkedlist", "1.2.0"));
        assert!(is_queryable("github.com/sparkle-project/Sparkle", "2.8.1"));
    }

    #[test]
    fn test_packages_from_resolved_v1_a() {
        assert_eq!(
            names(&packages_from_resolved(V1)),
            vec!["github.com/apple/swift-nio-2.0.0"]
        );
        assert!(packages_from_resolved("{}").is_empty());
        assert!(packages_from_resolved("not json").is_empty());
    }

    fn items_of(dir: &Path) -> Vec<DirItem> {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| DirItem {
                path: e.path(),
                file_type: e.file_type().unwrap(),
            })
            .collect()
    }

    #[test]
    fn test_scanner_a() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(PACKAGE_RESOLVED), V3).unwrap();
        fs::write(dir.path().join(PACKAGE_SWIFT), "// swift-tools-version:5.9").unwrap();
        let d = SwiftScanner.detect(dir.path(), &items_of(dir.path()));
        assert_eq!(d.origins.len(), 1);
        assert_eq!(d.skip_children, vec![".build"]);
        assert!(SwiftScanner.accepts(&dir.path().join(PACKAGE_SWIFT)));
        let o = SwiftScanner.normalize_origin(dir.path()).unwrap();
        let sites = SwiftScanner.origin_to_sites(
            &o,
            &ScanConfig::new(false, false),
            FlagLog(false),
        );
        assert_eq!(sites[0].kind, SiteKind::Lockfile);
        assert_eq!(SwiftScanner.site_to_packages(&sites[0]).len(), 4);
        let empty = tempdir().unwrap();
        assert!(SwiftScanner.normalize_origin(empty.path()).is_err());
    }
}
