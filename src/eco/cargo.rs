//! Cargo (Rust) scanner.
//!
//! Origins are (1) project directories containing a `Cargo.lock` (a workspace root), and (2) the Cargo home directory (`$CARGO_HOME`, default `~/.cargo`) if it records `cargo install`ed crates. Sites are the `Cargo.lock` file (`SiteKind::Lockfile`) and the install record `.crates2.json` or `.crates.toml` (`SiteKind::Installed`).

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::eco::Detect;
use crate::eco::DirItem;
use crate::eco::EcosystemScanner;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::origin_site::SiteKind;
use crate::package::Package;
use crate::package_durl::DirectURL;
use crate::package_durl::VcsInfo;
use crate::path_shared::PathShared;
use crate::util::path_home;
use crate::util::path_normalize;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::ScanConfig;
use crate::version_spec::VersionSpec;

const CARGO_LOCK: &str = "Cargo.lock";
const CARGO_TOML: &str = "Cargo.toml";
/// Install records written by `cargo install`; `.crates2.json` (current) has precedence over `.crates.toml` (v1).
const CRATES2_JSON: &str = ".crates2.json";
const CRATES_TOML: &str = ".crates.toml";

/// Source strings that denote crates.io; packages from crates.io have no `direct_url`.
const CRATES_IO_SOURCES: [&str; 2] = [
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
];

//------------------------------------------------------------------------------
fn env_or_home(var: &str, home_child: &str) -> Option<PathBuf> {
    match env::var_os(var) {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => path_home().map(|h| h.join(home_child)),
    }
}

/// The Cargo home directory (`$CARGO_HOME`, default `~/.cargo`).
fn cargo_home() -> Option<&'static Path> {
    static CARGO_HOME: OnceLock<Option<PathBuf>> = OnceLock::new();
    CARGO_HOME
        .get_or_init(|| env_or_home("CARGO_HOME", ".cargo"))
        .as_deref()
}

/// The rustup home directory (`$RUSTUP_HOME`, default `~/.rustup`).
fn rustup_home() -> Option<&'static Path> {
    static RUSTUP_HOME: OnceLock<Option<PathBuf>> = OnceLock::new();
    RUSTUP_HOME
        .get_or_init(|| env_or_home("RUSTUP_HOME", ".rustup"))
        .as_deref()
}

/// Return the install record file in a directory, if present.
fn install_record(dir: &Path) -> Option<PathBuf> {
    [CRATES2_JSON, CRATES_TOML]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
}

/// Search `dir` and its ancestors for the `Cargo.lock` that governs it (a workspace member uses its workspace root's lockfile).
fn find_lock_dir(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|d| d.join(CARGO_LOCK).is_file())
        .map(|d| d.to_path_buf())
}

fn origin(dir: PathBuf) -> Origin {
    Origin::new(Ecosystem::Cargo, PathShared::from(dir))
}

//------------------------------------------------------------------------------
/// Convert a Cargo `source` string to provenance. crates.io is `None`; other registries carry their URL; git sources (`git+URL?rev=X#SHA`) carry the URL, commit, and requested ref.
fn source_to_direct_url(source: &str) -> Option<DirectURL> {
    if CRATES_IO_SOURCES.contains(&source) {
        return None;
    }
    if let Some(rest) = source.strip_prefix("git+") {
        let (url_query, commit) = match rest.split_once('#') {
            Some((u, c)) => (u, Some(c)),
            None => (rest, None),
        };
        let (url, query) = match url_query.split_once('?') {
            Some((u, q)) => (u, Some(q)),
            None => (url_query, None),
        };
        // query is one of branch=, tag=, rev=
        let revision = query.and_then(|q| q.split_once('=').map(|(_, v)| v.to_string()));
        return Some(DirectURL {
            url: url.to_string(),
            vcs_info: commit.map(|c| VcsInfo {
                commit_id: c.to_string(),
                vcs: "git".to_string(),
                revision,
            }),
        });
    }
    let url = source
        .strip_prefix("registry+")
        .or_else(|| source.strip_prefix("sparse+"))
        .unwrap_or(source);
    Some(DirectURL {
        url: url.to_string(),
        vcs_info: None,
    })
}

fn package(name: &str, version: &str, source: Option<&str>) -> Package {
    Package::new(
        Ecosystem::Cargo,
        name,
        VersionSpec::new(version),
        source.and_then(source_to_direct_url),
    )
}

#[derive(Deserialize)]
struct LockFile {
    #[serde(default)]
    package: Vec<LockPackage>,
}

#[derive(Deserialize)]
struct LockPackage {
    name: String,
    version: String,
    source: Option<String>,
}

/// Read packages from a `Cargo.lock` (all format versions share `[[package]]` with `name`, `version`, and optional `source`). Entries without a `source` are workspace members or path dependencies: they are the project's own crates, not resolved dependencies, and are excluded.
fn packages_from_lock(content: &str) -> ResultDynError<Vec<Package>> {
    let lock: LockFile = toml::from_str(content)?;
    Ok(lock
        .package
        .iter()
        .filter_map(|p| {
            p.source
                .as_deref()
                .map(|s| package(&p.name, &p.version, Some(s)))
        })
        .collect())
}

/// Parse an install record key: `"name version (source)"`.
fn parse_install_key(key: &str) -> Option<Package> {
    let (name_version, source) = match key.split_once(" (") {
        Some((nv, rest)) => (nv, rest.strip_suffix(')')),
        None => (key, None),
    };
    let (name, version) = name_version.split_once(' ')?;
    // `path+file://` sources are local builds, not dependencies from a registry or VCS
    if source.is_some_and(|s| s.starts_with("path+")) {
        return None;
    }
    Some(package(name, version, source))
}

#[derive(Deserialize)]
struct Crates2 {
    #[serde(default)]
    installs: HashMap<String, serde_json::Value>,
}

fn packages_from_crates2_json(content: &str) -> ResultDynError<Vec<Package>> {
    let c: Crates2 = serde_json::from_str(content)?;
    Ok(c.installs
        .keys()
        .filter_map(|k| parse_install_key(k))
        .collect())
}

#[derive(Deserialize)]
struct CratesToml {
    #[serde(default)]
    v1: HashMap<String, toml::Value>,
}

fn packages_from_crates_toml(content: &str) -> ResultDynError<Vec<Package>> {
    let c: CratesToml = toml::from_str(content)?;
    Ok(c.v1.keys().filter_map(|k| parse_install_key(k)).collect())
}

//------------------------------------------------------------------------------
pub(crate) struct CargoScanner;

impl EcosystemScanner for CargoScanner {
    /// The Cargo home is found without walking, so that a non-default `$CARGO_HOME` is included.
    fn default_origins(&self) -> Vec<Origin> {
        cargo_home()
            .filter(|h| install_record(h).is_some())
            .map(|h| origin(h.to_path_buf()))
            .into_iter()
            .collect()
    }

    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect {
        // Cargo and rustup homes contain hundreds of `Cargo.lock` files from downloaded crate sources and toolchains; these are not projects. Claim the Cargo home's installs (if any) and do not descend.
        if cargo_home() == Some(dir) || rustup_home() == Some(dir) {
            let origins = if cargo_home() == Some(dir) {
                self.default_origins()
            } else {
                Vec::new()
            };
            return Detect {
                origins,
                prune: true,
                ..Default::default()
            };
        }
        let has = |name: &str| items.iter().any(|i| i.file_name() == Some(name));
        let origins = if has(CARGO_LOCK) {
            vec![origin(dir.to_path_buf())]
        } else {
            Vec::new()
        };
        // build output is large and contains no projects
        let skip_children = if has(CARGO_TOML) {
            vec!["target"]
        } else {
            Vec::new()
        };
        Detect {
            origins,
            prune: false,
            skip_children,
        }
    }

    /// Accept a `Cargo.lock` or `Cargo.toml` file, a directory containing either, or a directory with a `cargo install` record (a Cargo home).
    fn accepts(&self, path: &Path) -> bool {
        let Ok(p) = path_normalize(path, false) else {
            return false;
        };
        if p.is_file() {
            return p
                .file_name()
                .is_some_and(|n| n == CARGO_LOCK || n == CARGO_TOML);
        }
        p.is_dir()
            && (p.join(CARGO_LOCK).is_file()
                || p.join(CARGO_TOML).is_file()
                || install_record(&p).is_some())
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
        if install_record(&dir).is_some() {
            return Ok(origin(dir));
        }
        find_lock_dir(&dir).map(origin).ok_or_else(|| {
            format!("No Cargo.lock found for {dir:?} or its parents; run `cargo generate-lockfile`")
                .into()
        })
    }

    fn origin_to_sites(
        &self,
        origin: &Origin,
        _config: &ScanConfig,
        _log: FlagLog,
    ) -> Vec<Site> {
        let dir = origin.as_path();
        let mut sites = Vec::new();
        if let Some(record) = install_record(dir) {
            sites.push(Site::new(
                Ecosystem::Cargo,
                SiteKind::Installed,
                PathShared::from(record),
            ));
        }
        let lock = dir.join(CARGO_LOCK);
        if lock.is_file() {
            sites.push(Site::new(
                Ecosystem::Cargo,
                SiteKind::Lockfile,
                PathShared::from(lock),
            ));
        }
        sites
    }

    fn site_to_packages(&self, site: &Site) -> Vec<Package> {
        let Ok(content) = fs::read_to_string(site.as_path()) else {
            return Vec::new();
        };
        let parsed = match site.as_path().file_name().and_then(|n| n.to_str()) {
            Some(CARGO_LOCK) => packages_from_lock(&content),
            Some(CRATES2_JSON) => packages_from_crates2_json(&content),
            Some(CRATES_TOML) => packages_from_crates_toml(&content),
            _ => Ok(Vec::new()),
        };
        // a malformed file yields no packages rather than failing the whole scan
        parsed.unwrap_or_default()
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const LOCK_V4: &str = r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "myapp"
version = "0.1.0"
dependencies = [
 "serde",
 "utest",
]

[[package]]
name = "mylib"
version = "0.2.0"

[[package]]
name = "serde"
version = "1.0.200"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ddc6f9cc94d67c0e21aaf7eda3a010fd3af78ebf6e096aa6e2e13c79749cce4f"

[[package]]
name = "serde_json"
version = "1.0.117"
source = "sparse+https://index.crates.io/"

[[package]]
name = "utest"
version = "0.1.0"
source = "git+https://github.com/japaric/utest?branch=main#41301d9d61f2e0ad2b6f2a3f3d2c0b1e3c7e0d8a"

[[package]]
name = "internal"
version = "3.1.4"
source = "registry+https://my.registry.example/index"
"#;

    /// Format version 1 (no `version` header; checksums in `[metadata]`).
    const LOCK_V1: &str = r#"[[package]]
name = "time"
version = "0.1.43"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "myapp"
version = "0.1.0"

[metadata]
"checksum time 0.1.43 (registry+https://github.com/rust-lang/crates.io-index)" = "ca8a50ef2360fbd1eeb0ecd46795a87a19024eb4b53c5dc916ca1fd95fe62438"
"#;

    const CRATES2: &str = r#"{"installs":{"ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)":{"version_req":null,"bins":["rg"],"features":[],"all_features":false,"no_default_features":false,"profile":"release","target":"aarch64-apple-darwin","rustc":"rustc 1.79.0"},"mytool 0.1.0 (path+file:///Users/me/src/mytool)":{"bins":["mytool"]},"cargo-x 0.3.0 (git+https://github.com/o/cargo-x#abc123)":{"bins":["cargo-x"]}}}"#;

    const CRATES_TOML_V1: &str = r#"[v1]
"sqlx-cli 0.8.6 (registry+https://github.com/rust-lang/crates.io-index)" = [
    "cargo-sqlx",
    "sqlx",
]
"espup 0.14.1 (registry+https://github.com/rust-lang/crates.io-index)" = ["espup"]
"#;

    fn names(packages: &[Package]) -> Vec<String> {
        let mut v: Vec<String> = packages.iter().map(|p| p.to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn test_packages_from_lock_v4_a() {
        let packages = packages_from_lock(LOCK_V4).unwrap();
        // workspace members without a source are excluded
        assert_eq!(
            names(&packages),
            vec![
                "internal-3.1.4",
                "serde-1.0.200",
                "serde_json-1.0.117",
                "utest-0.1.0"
            ]
        );
        assert!(packages.iter().all(|p| p.ecosystem == Ecosystem::Cargo));
    }

    #[test]
    fn test_packages_from_lock_sources_a() {
        let packages = packages_from_lock(LOCK_V4).unwrap();
        let get = |n: &str| packages.iter().find(|p| p.name == n).unwrap();
        // crates.io, via either index protocol
        assert_eq!(get("serde").direct_url, None);
        assert_eq!(get("serde_json").direct_url, None);
        // git source
        let durl = get("utest").direct_url.clone().unwrap();
        assert_eq!(durl.url, "https://github.com/japaric/utest");
        let vcs = durl.vcs_info.unwrap();
        assert_eq!(vcs.vcs, "git");
        assert_eq!(vcs.commit_id, "41301d9d61f2e0ad2b6f2a3f3d2c0b1e3c7e0d8a");
        assert_eq!(vcs.revision.as_deref(), Some("main"));
        // alternative registry
        let durl = get("internal").direct_url.clone().unwrap();
        assert_eq!(durl.url, "https://my.registry.example/index");
        assert!(durl.vcs_info.is_none());
    }

    #[test]
    fn test_packages_from_lock_v1_a() {
        let packages = packages_from_lock(LOCK_V1).unwrap();
        assert_eq!(names(&packages), vec!["time-0.1.43"]);
    }

    #[test]
    fn test_packages_from_lock_invalid_a() {
        assert!(packages_from_lock("this is [[ not toml").is_err());
        assert!(packages_from_lock("").unwrap().is_empty());
    }

    #[test]
    fn test_packages_from_crates2_json_a() {
        let packages = packages_from_crates2_json(CRATES2).unwrap();
        // local `path+` installs are excluded
        assert_eq!(names(&packages), vec!["cargo-x-0.3.0", "ripgrep-14.1.0"]);
        let cx = packages.iter().find(|p| p.name == "cargo-x").unwrap();
        let durl = cx.direct_url.clone().unwrap();
        assert_eq!(durl.url, "https://github.com/o/cargo-x");
        assert_eq!(durl.vcs_info.unwrap().commit_id, "abc123");
    }

    #[test]
    fn test_packages_from_crates_toml_a() {
        let packages = packages_from_crates_toml(CRATES_TOML_V1).unwrap();
        assert_eq!(names(&packages), vec!["espup-0.14.1", "sqlx-cli-0.8.6"]);
    }

    #[test]
    fn test_parse_install_key_a() {
        assert!(parse_install_key("nospace").is_none());
        let p = parse_install_key("bat 0.24.0").unwrap();
        assert_eq!(p.to_string(), "bat-0.24.0");
        assert_eq!(p.direct_url, None);
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
    fn test_detect_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(CARGO_TOML), "[package]\nname = \"x\"\n").unwrap();
        fs::write(root.join(CARGO_LOCK), LOCK_V4).unwrap();
        fs::create_dir(root.join("target")).unwrap();

        let d = CargoScanner.detect(root, &items_of(root));
        assert_eq!(d.origins.len(), 1);
        assert_eq!(d.origins[0].as_path(), root);
        assert!(!d.prune);
        assert_eq!(d.skip_children, vec!["target"]);

        // a workspace member: Cargo.toml without Cargo.lock is not an origin, but target is still skipped
        let member = root.join("member");
        fs::create_dir(&member).unwrap();
        fs::write(member.join(CARGO_TOML), "").unwrap();
        let d = CargoScanner.detect(&member, &items_of(&member));
        assert!(d.origins.is_empty());
        assert_eq!(d.skip_children, vec!["target"]);
    }

    #[test]
    fn test_origin_to_sites_and_packages_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(CARGO_LOCK), LOCK_V4).unwrap();
        let o = origin(root.to_path_buf());
        let config = ScanConfig::new(false, false);
        let sites = CargoScanner.origin_to_sites(&o, &config, FlagLog(false));
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, SiteKind::Lockfile);
        assert_eq!(CargoScanner.site_to_packages(&sites[0]).len(), 4);
    }

    #[test]
    fn test_cargo_home_origin_a() {
        // a directory with an install record is a Cargo home origin, with an Installed site
        let dir = tempdir().unwrap();
        let home = dir.path();
        fs::write(home.join(CRATES_TOML), CRATES_TOML_V1).unwrap();
        fs::write(home.join(CRATES2_JSON), CRATES2).unwrap();
        assert!(CargoScanner.accepts(home));
        let o = CargoScanner.normalize_origin(home).unwrap();
        let config = ScanConfig::new(false, false);
        let sites = CargoScanner.origin_to_sites(&o, &config, FlagLog(false));
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, SiteKind::Installed);
        // .crates2.json has precedence over .crates.toml
        assert!(sites[0].as_path().ends_with(CRATES2_JSON));
        assert_eq!(CargoScanner.site_to_packages(&sites[0]).len(), 2);
    }

    #[test]
    fn test_normalize_origin_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(CARGO_TOML), "").unwrap();
        fs::write(root.join(CARGO_LOCK), LOCK_V4).unwrap();
        let member = root.join("crates/member");
        fs::create_dir_all(&member).unwrap();
        fs::write(member.join(CARGO_TOML), "").unwrap();

        // the lockfile, the manifest, the root dir, and a member dir all resolve to the workspace root
        for p in [
            root.join(CARGO_LOCK),
            root.join(CARGO_TOML),
            root.to_path_buf(),
            member.clone(),
            member.join(CARGO_TOML),
        ] {
            assert!(CargoScanner.accepts(&p), "{p:?}");
            let o = CargoScanner.normalize_origin(&p).unwrap();
            assert_eq!(o.as_path(), root, "{p:?}");
        }
        assert!(!CargoScanner.accepts(&root.join("crates")));
    }

    #[test]
    fn test_normalize_origin_no_lock_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(CARGO_TOML), "").unwrap();
        assert!(CargoScanner.accepts(root));
        let err = CargoScanner.normalize_origin(root).unwrap_err().to_string();
        assert!(err.contains("No Cargo.lock"), "{err}");
    }
}
