//! Swift (SwiftPM) scanner.
//!
//! An origin is a directory containing a `Package.resolved` (a SwiftPM package beside its `Package.swift`, or an Xcode project's or workspace's `xcshareddata/swiftpm`); its site is that file (`SiteKind::Lockfile`). Formats 1 (`object.pins`, `repositoryURL`) and 2–3 (`pins`, `location`, `kind`) are read.
//!
//! Dependencies that were checked out are read from SwiftPM's `workspace-state.json` (`SiteKind::Installed`): `.build/workspace-state.json` beside `Package.swift`, or, for an Xcode project, `~/Library/Developer/Xcode/DerivedData/<Name>-<hash>/SourcePackages/workspace-state.json`, whose folder's `info.plist` names the project (`WorkspacePath`). Each dependency is read as a `Package.resolved` pin is; edited and local (`fileSystem`) dependencies are excluded.
//!
//! OSV's `SwiftURL` ecosystem names a package by its repository URL without scheme or `.git` (`github.com/apple/swift-nio`), case-sensitively, so names are kept as written in `location` (normalized, not lowercased). A pin with a version takes it (OSV matches versions server-side); a branch or revision pin takes its revision SHA, which `osv_match::check_sha_pins` resolves to a tag where the commit is tagged, and is otherwise unaudited. Local pins (`localSourceControl`) are excluded. Registry pins (`kind: registry`, an identity `scope.name` rather than a URL) are kept with that identity, which OSV cannot match, so they are unaudited.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::SystemTime;

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
use crate::util::path_home;
use crate::util::path_normalize;
use crate::util::path_users;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::ScanConfig;
use crate::version_spec::VersionSpec;

const PACKAGE_RESOLVED: &str = "Package.resolved";
const PACKAGE_SWIFT: &str = "Package.swift";
/// SwiftPM's build directory, beside `Package.swift`; its `checkouts` hold dependencies' sources and resolved files.
const BUILD_DIR: &str = ".build";
/// SwiftPM's record of the dependencies it checked out (in `.build`, or in an Xcode `SourcePackages` directory).
const WORKSPACE_STATE: &str = "workspace-state.json";

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

/// A `workspace-state.json` dependency as a `Package.resolved` pin, or `None` for local or edited code. The state is `sourceControlCheckout` (`checkout` before SwiftPM 5.6) with a `checkoutState`, or `registryDownload` with a `version`.
fn state_dependency_pin(dep: &JsonValue) -> Option<JsonValue> {
    let pref = dep.get("packageRef")?;
    let state = dep.get("state")?;
    let kind = match pref.get("kind").and_then(JsonValue::as_str)? {
        "remoteSourceControl" | "remote" => "remoteSourceControl",
        "registry" => "registry",
        _ => return None,
    };
    let pin_state = match state.get("name").and_then(JsonValue::as_str)? {
        "sourceControlCheckout" | "checkout" => state.get("checkoutState")?.clone(),
        "registryDownload" => serde_json::json!({"version": state.get("version")?}),
        _ => return None,
    };
    Some(serde_json::json!({
        "identity": pref.get("identity"),
        "kind": kind,
        "location": pref.get("location").or_else(|| pref.get("path")),
        "state": pin_state,
    }))
}

/// The checked-out dependencies of a `workspace-state.json`.
fn packages_from_workspace_state(content: &str) -> Vec<Package> {
    let Ok(v) = serde_json::from_str::<JsonValue>(content) else {
        return Vec::new();
    };
    let deps = v
        .get("object")
        .and_then(|o| o.get("dependencies"))
        .and_then(JsonValue::as_array);
    let unique: HashSet<Package> = deps
        .into_iter()
        .flatten()
        .filter_map(state_dependency_pin)
        .filter_map(|p| pin_package(&p))
        .collect();
    unique.into_iter().collect()
}

/// The `WorkspacePath` of an XML `info.plist` (as Xcode writes in each DerivedData folder).
fn plist_workspace_path(content: &str) -> Option<PathBuf> {
    let rest = &content[content.find("<key>WorkspacePath</key>")? + 24..];
    let start = rest.find("<string>")? + 8;
    let end = rest[start..].find("</string>")? + start;
    let raw = rest[start..end]
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&");
    Some(PathBuf::from(raw))
}

/// For each Xcode project or workspace with resolved packages in a DerivedData directory, its newest `SourcePackages/workspace-state.json`.
fn derived_data_states(derived_data_dirs: &[PathBuf]) -> HashMap<PathBuf, PathBuf> {
    let mut found: HashMap<PathBuf, (SystemTime, PathBuf)> = HashMap::new();
    for dd in derived_data_dirs {
        for e in fs::read_dir(dd).into_iter().flatten().flatten() {
            let dir = e.path();
            let state = dir.join("SourcePackages").join(WORKSPACE_STATE);
            let Some(modified) = fs::metadata(&state).and_then(|m| m.modified()).ok()
            else {
                continue;
            };
            let Some(workspace) = fs::read_to_string(dir.join("info.plist"))
                .ok()
                .and_then(|c| plist_workspace_path(&c))
            else {
                continue;
            };
            match found.get(&workspace) {
                Some((t, _)) if *t >= modified => {}
                _ => {
                    found.insert(workspace, (modified, state));
                }
            }
        }
    }
    found.into_iter().map(|(w, (_, s))| (w, s)).collect()
}

/// Xcode's DerivedData directories of the current user and, if readable, of every user.
fn derived_data_dirs() -> Vec<PathBuf> {
    let mut homes: Vec<PathBuf> = path_home().into_iter().collect();
    if let Some(users) = path_users() {
        homes.extend(
            fs::read_dir(users)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path()),
        );
    }
    homes.sort();
    homes.dedup();
    homes
        .into_iter()
        .map(|h| h.join("Library/Developer/Xcode/DerivedData"))
        .filter(|d| d.is_dir())
        .collect()
}

/// The DerivedData `workspace-state.json` of each Xcode project or workspace, read once per process.
fn xcode_states() -> &'static HashMap<PathBuf, PathBuf> {
    static STATES: OnceLock<HashMap<PathBuf, PathBuf>> = OnceLock::new();
    STATES.get_or_init(|| derived_data_states(&derived_data_dirs()))
}

/// The checked-out state of a Swift origin: `.build/workspace-state.json` beside `Package.swift`, or, for an origin within an `.xcodeproj` or `.xcworkspace`, its DerivedData state in `states`.
fn workspace_state(dir: &Path, states: &HashMap<PathBuf, PathBuf>) -> Option<PathBuf> {
    let fp = dir.join(BUILD_DIR).join(WORKSPACE_STATE);
    if fp.is_file() {
        return Some(fp);
    }
    dir.ancestors()
        .filter(|a| {
            a.extension()
                .is_some_and(|e| e == "xcodeproj" || e == "xcworkspace")
        })
        .find_map(|a| states.get(a))
        .filter(|s| s.is_file())
        .cloned()
}

fn origin(dir: PathBuf) -> Origin {
    Origin::new(Ecosystem::Swift, PathShared::from(dir))
}

pub(crate) struct SwiftScanner;

impl EcosystemScanner for SwiftScanner {
    fn primary_site_kinds(&self) -> &'static [SiteKind] {
        // the dependencies checked out by SwiftPM or Xcode
        &[SiteKind::Installed]
    }

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
        let mut sites = Vec::new();
        if let Some(state) = workspace_state(origin.as_path(), xcode_states()) {
            sites.push(Site::new(
                Ecosystem::Swift,
                SiteKind::Installed,
                PathShared::from(state),
            ));
        }
        let fp = origin.as_path().join(PACKAGE_RESOLVED);
        if fp.is_file() {
            sites.push(Site::new(
                Ecosystem::Swift,
                SiteKind::Lockfile,
                PathShared::from(fp),
            ));
        }
        sites
    }

    fn site_to_packages(&self, site: &Site) -> Vec<Package> {
        let Ok(content) = fs::read_to_string(site.as_path()) else {
            return Vec::new();
        };
        match site.kind {
            SiteKind::Installed => packages_from_workspace_state(&content),
            _ => packages_from_resolved(&content),
        }
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

    /// The checked-out equivalent of `V3`, plus an edited and a local dependency, and a pre-5.6 `remote`/`checkout` entry.
    const STATE: &str = r#"{
  "object" : {
    "artifacts" : [],
    "dependencies" : [
      {"basedOn" : null, "packageRef" : {"identity" : "sparkle", "kind" : "remoteSourceControl",
        "location" : "https://github.com/sparkle-project/Sparkle", "name" : "Sparkle"},
       "state" : {"checkoutState" : {"revision" : "5581748cef2bae787496fe6d61139aebe0a451f6", "version" : "2.8.1"},
                  "name" : "sourceControlCheckout"}, "subpath" : "Sparkle"},
      {"basedOn" : null, "packageRef" : {"identity" : "commander", "kind" : "remote",
        "path" : "https://github.com/steipete/Commander.git", "name" : "Commander"},
       "state" : {"checkoutState" : {"revision" : "9e349575c8e3c6745e81fe19e5bb5efa01b078ce", "version" : "0.2.1"},
                  "name" : "checkout"}, "subpath" : "Commander"},
      {"basedOn" : null, "packageRef" : {"identity" : "peekaboo", "kind" : "remoteSourceControl",
        "location" : "https://github.com/steipete/Peekaboo.git", "name" : "Peekaboo"},
       "state" : {"checkoutState" : {"branch" : "main", "revision" : "bace59f90bb276f1c6fb613acfda3935ec4a7a90"},
                  "name" : "sourceControlCheckout"}, "subpath" : "Peekaboo"},
      {"basedOn" : null, "packageRef" : {"identity" : "mona.linkedlist", "kind" : "registry",
        "location" : "mona.linkedlist", "name" : "LinkedList"},
       "state" : {"name" : "registryDownload", "version" : "1.2.0"}, "subpath" : "mona/LinkedList"},
      {"basedOn" : null, "packageRef" : {"identity" : "local", "kind" : "fileSystem",
        "location" : "/Users/x/local", "name" : "local"},
       "state" : {"name" : "fileSystem", "path" : "/Users/x/local"}, "subpath" : "local"},
      {"basedOn" : null, "packageRef" : {"identity" : "edited", "kind" : "remoteSourceControl",
        "location" : "https://github.com/o/edited", "name" : "edited"},
       "state" : {"name" : "edited", "path" : null}, "subpath" : "edited"}
    ]
  },
  "version" : 6
}"#;

    #[test]
    fn test_packages_from_workspace_state_a() {
        // the same packages as the equivalent Package.resolved
        let state = packages_from_workspace_state(STATE);
        let mut resolved = packages_from_resolved(V3);
        resolved.sort();
        let mut state_sorted = state.clone();
        state_sorted.sort();
        assert_eq!(state_sorted, resolved);
        assert!(packages_from_workspace_state("{}").is_empty());
        assert!(packages_from_workspace_state("not json").is_empty());
    }

    #[test]
    fn test_plist_workspace_path_a() {
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
	<key>LastAccessedDate</key><date>2026-11-01T10:00:00Z</date>
	<key>WorkspacePath</key>
	<string>/Users/me/src/R&amp;D/App.xcodeproj</string>
</dict></plist>"#;
        assert_eq!(
            plist_workspace_path(plist),
            Some(PathBuf::from("/Users/me/src/R&D/App.xcodeproj"))
        );
        assert_eq!(plist_workspace_path("<plist/>"), None);
    }

    #[test]
    fn test_workspace_state_swiftpm_a() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(PACKAGE_RESOLVED), V3).unwrap();
        fs::write(dir.path().join(PACKAGE_SWIFT), "").unwrap();
        let config = ScanConfig::new(false, false);
        let o = origin(dir.path().to_path_buf());
        // not yet resolved: only the lockfile
        let sites = SwiftScanner.origin_to_sites(&o, &config, FlagLog(false));
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, SiteKind::Lockfile);

        fs::create_dir(dir.path().join(BUILD_DIR)).unwrap();
        fs::write(dir.path().join(BUILD_DIR).join(WORKSPACE_STATE), STATE).unwrap();
        let sites = SwiftScanner.origin_to_sites(&o, &config, FlagLog(false));
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].kind, SiteKind::Installed);
        assert_eq!(SwiftScanner.site_to_packages(&sites[0]).len(), 4);
    }

    #[test]
    fn test_workspace_state_xcode_a() {
        let dir = tempdir().unwrap();
        let proj = dir.path().join("src/App.xcodeproj");
        let origin_dir = proj.join("project.xcworkspace/xcshareddata/swiftpm");
        fs::create_dir_all(&origin_dir).unwrap();
        fs::write(origin_dir.join(PACKAGE_RESOLVED), V3).unwrap();

        let dd = dir.path().join("DerivedData");
        let plist = |p: &Path| {
            format!(
                "<plist><dict><key>WorkspacePath</key><string>{}</string></dict></plist>",
                p.display()
            )
        };
        // this project's folder, and another project's
        for (name, workspace) in [
            ("App-abc", proj.clone()),
            ("Other-def", dir.path().join("Other.xcodeproj")),
        ] {
            let sp = dd.join(name).join("SourcePackages");
            fs::create_dir_all(&sp).unwrap();
            fs::write(dd.join(name).join("info.plist"), plist(&workspace)).unwrap();
            fs::write(sp.join(WORKSPACE_STATE), STATE).unwrap();
        }
        // a folder without resolved packages is ignored
        fs::create_dir_all(dd.join("Empty-ghi")).unwrap();
        fs::write(dd.join("Empty-ghi/info.plist"), plist(&proj)).unwrap();

        let states = derived_data_states(std::slice::from_ref(&dd));
        assert_eq!(states.len(), 2);
        assert_eq!(
            workspace_state(&origin_dir, &states),
            Some(dd.join("App-abc/SourcePackages").join(WORKSPACE_STATE))
        );
        // an origin of a project Xcode has not opened has no state
        let unopened = dir.path().join("New.xcworkspace/xcshareddata/swiftpm");
        fs::create_dir_all(&unopened).unwrap();
        assert_eq!(workspace_state(&unopened, &states), None);
    }
}
