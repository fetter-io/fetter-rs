//! Cargo (Rust) scanner.
//!
//! Origins are (1) project directories containing a `Cargo.lock` (a workspace root), and (2) the Cargo home directory (`$CARGO_HOME`, default `~/.cargo`) if it records `cargo install`ed crates. Sites are the `Cargo.lock` file (`SiteKind::Lockfile`), the project's build directory `target` (`SiteKind::Installed`; see below), and the install record `.crates2.json` or `.crates.toml` (`SiteKind::Installed`).
//!
//! Crates compiled into a project are found from Cargo's dep-info files (`target/<profile>/deps/*.d`, and `target/<triple>/<profile>/deps/*.d` for cross builds), which list the source files of each compiled unit. A source under `registry/src/<index>/<name>-<version>/` is a registry crate; one under `git/checkouts/<repo>/<rev>/` is a git dependency, named by the nearest `Cargo.toml`. A unit counts only if it produced a build product (not only `.rmeta`, as `cargo check` and rust-analyzer do). Built crates are matched against the project's `Cargo.lock`, so that units left over from before a `cargo update` are not reported, and each package takes the lockfile's provenance. A non-default target directory (`CARGO_TARGET_DIR`, `build.target-dir`) and vendored sources (`cargo vendor`) are not read.

use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::eco::Artifacts;
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
/// The default build directory of a Cargo project.
const TARGET: &str = "target";
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

/// True if `dir` is the Cargo home, or is laid out as one: a `.cargo` directory with downloaded sources (`registry`, `git`) or a `cargo install` record.
fn is_cargo_home(dir: &Path, items: &[DirItem]) -> bool {
    if cargo_home() == Some(dir) {
        return true;
    }
    dir.file_name().is_some_and(|n| n == ".cargo")
        && items.iter().any(|i| {
            let name = i.file_name();
            (i.file_type.is_dir() && matches!(name, Some("registry") | Some("git")))
                || (i.file_type.is_file()
                    && matches!(name, Some(CRATES2_JSON) | Some(CRATES_TOML)))
        })
}

/// True if `dir` is the rustup home, or is laid out as one: a `.rustup` directory with `toolchains`.
fn is_rustup_home(dir: &Path, items: &[DirItem]) -> bool {
    rustup_home() == Some(dir)
        || (dir.file_name().is_some_and(|n| n == ".rustup")
            && items
                .iter()
                .any(|i| i.file_type.is_dir() && i.file_name() == Some("toolchains")))
}

/// The default directory of `cargo vendor`.
const VENDOR: &str = "vendor";

/// True if `dir` holds crates vendored by `cargo vendor`: each crate directory has a `.cargo-checksum.json`. Only the first few entries are checked.
fn is_cargo_vendor(dir: &Path) -> bool {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .take(8)
        .any(|e| e.path().join(".cargo-checksum.json").is_file())
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

//------------------------------------------------------------------------------
/// A crate whose sources a compiled unit read, from a source path.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
enum BuiltRef {
    /// A registry crate's source directory name, `name-version`.
    Registry(String),
    /// A git dependency's package name and checkout revision (an abbreviated commit).
    Git { name: String, rev: String },
}

/// One compiled unit, from one dep-info file: the crates it read and the files it wrote to its `deps` directory.
#[derive(Debug, Default)]
struct BuiltUnit {
    refs: HashSet<BuiltRef>,
    outputs: Vec<PathBuf>,
}

/// The `deps` directories of a target directory: `target/<profile>/deps`, and `target/<triple>/<profile>/deps` for cross builds.
fn target_deps_dirs(target: &Path) -> Vec<PathBuf> {
    let subdirs = |d: &Path| -> Vec<PathBuf> {
        fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect()
    };
    let mut out = Vec::new();
    for a in subdirs(target) {
        let deps = a.join("deps");
        if deps.is_dir() {
            out.push(deps);
            continue;
        }
        for b in subdirs(&a) {
            let deps = b.join("deps");
            if deps.is_dir() {
                out.push(deps);
            }
        }
    }
    out.sort();
    out
}

/// Split a dep-info path list on whitespace; Make escapes spaces in paths as `\ `.
fn split_make_paths(s: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&' ') {
            cur.push(' ');
            chars.next();
        } else if c.is_whitespace() {
            if !cur.is_empty() {
                out.push(PathBuf::from(std::mem::take(&mut cur)));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(PathBuf::from(cur));
    }
    out
}

/// The rules of a dep-info file: each `output: sources…` line, without comments (`# env-dep:`).
fn dep_info_rules(content: &str) -> Vec<(PathBuf, Vec<PathBuf>)> {
    content
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            // a Windows drive (`C:\`) is not followed by a space; an empty rule ends with `:`
            let (out, srcs) = match l.find(": ") {
                Some(i) => (&l[..i], &l[i + 2..]),
                None => (l.strip_suffix(':')?, ""),
            };
            let out = split_make_paths(out).pop()?;
            Some((out, split_make_paths(srcs)))
        })
        .collect()
}

/// True if `output` is a file Cargo wrote to a `deps` directory.
fn is_deps_output(output: &Path) -> bool {
    output
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "deps")
}

/// True if `output` is a build product (a library or executable), not metadata (`.rmeta`, written alone by `cargo check`) or the dep-info file itself.
fn is_build_product(output: &Path) -> bool {
    is_deps_output(output)
        && !output.extension().is_some_and(|e| e == "rmeta" || e == "d")
}

#[derive(Deserialize)]
struct ManifestPackage {
    name: String,
}

#[derive(Deserialize)]
struct Manifest {
    package: Option<ManifestPackage>,
}

/// The package name of the crate containing `src` in a git checkout: the nearest `Cargo.toml` with a `[package]`, no higher than `checkout`.
fn git_crate_name(
    src: &Path,
    checkout: &Path,
    names: &mut HashMap<PathBuf, Option<String>>,
) -> Option<String> {
    for dir in src.ancestors().skip(1) {
        if !dir.starts_with(checkout) {
            break;
        }
        let name = names
            .entry(dir.to_path_buf())
            .or_insert_with(|| {
                fs::read_to_string(dir.join(CARGO_TOML))
                    .ok()
                    .and_then(|c| toml::from_str::<Manifest>(&c).ok())
                    .and_then(|m| m.package)
                    .map(|p| p.name)
            })
            .clone();
        if name.is_some() {
            return name;
        }
    }
    None
}

/// The crate a source path belongs to, if it is a registry or git dependency; sources elsewhere are the project's own crates.
fn source_ref(
    src: &Path,
    names: &mut HashMap<PathBuf, Option<String>>,
) -> Option<BuiltRef> {
    let comps: Vec<&std::ffi::OsStr> = src.iter().collect();
    for i in 0..comps.len().saturating_sub(3) {
        if comps[i] == "registry" && comps[i + 1] == "src" {
            return Some(BuiltRef::Registry(comps[i + 3].to_str()?.to_string()));
        }
        if comps[i] == "git" && comps[i + 1] == "checkouts" {
            let rev = comps[i + 3].to_str()?.to_string();
            let checkout: PathBuf = comps[..=i + 3].iter().collect();
            let name = git_crate_name(src, &checkout, names)?;
            return Some(BuiltRef::Git { name, rev });
        }
    }
    None
}

/// The compiled units of a target directory that produced a build product.
fn built_units(target: &Path) -> Vec<BuiltUnit> {
    let mut names = HashMap::new();
    let mut units = Vec::new();
    for deps in target_deps_dirs(target) {
        let Ok(entries) = fs::read_dir(&deps) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "d") {
                continue;
            }
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            let rules = dep_info_rules(&content);
            if !rules.iter().any(|(out, _)| is_build_product(out)) {
                continue;
            }
            let mut unit = BuiltUnit::default();
            for (out, srcs) in rules {
                if is_deps_output(&out) {
                    unit.outputs.push(out);
                }
                for src in srcs {
                    if let Some(r) = source_ref(&src, &mut names) {
                        unit.refs.insert(r);
                    }
                }
            }
            if !unit.refs.is_empty() {
                units.push(unit);
            }
        }
    }
    units
}

/// True if a lockfile entry is a crate the unit read: a registry crate by `name-version`, a git crate by name and a commit that starts with the checkout revision.
fn unit_has(unit: &BuiltUnit, lp: &LockPackage) -> bool {
    let Some(source) = lp.source.as_deref() else {
        return false;
    };
    if source.starts_with("git+") {
        let commit = source.rsplit_once('#').map(|(_, c)| c).unwrap_or_default();
        unit.refs.iter().any(|r| match r {
            BuiltRef::Git { name, rev } => {
                *name == lp.name && !rev.is_empty() && commit.starts_with(rev.as_str())
            }
            BuiltRef::Registry(_) => false,
        })
    } else {
        unit.refs
            .contains(&BuiltRef::Registry(format!("{}-{}", lp.name, lp.version)))
    }
}

/// Built crates of a project's target directory that are in its `Cargo.lock`, with the files of the units that compiled each.
fn built_packages(target: &Path) -> Vec<(Package, Vec<PathBuf>)> {
    let Some(lock) = target
        .parent()
        .and_then(|d| fs::read_to_string(d.join(CARGO_LOCK)).ok())
        .and_then(|c| toml::from_str::<LockFile>(&c).ok())
    else {
        return Vec::new();
    };
    let units = built_units(target);
    lock.package
        .iter()
        .filter_map(|lp| {
            let mut files: Vec<PathBuf> = units
                .iter()
                .filter(|u| unit_has(u, lp))
                .flat_map(|u| u.outputs.iter().cloned())
                .collect();
            if files.is_empty() {
                return None;
            }
            files.sort();
            files.dedup();
            Some((package(&lp.name, &lp.version, lp.source.as_deref()), files))
        })
        .collect()
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

/// Each installed crate in an install record, with the file names of its binaries (which include any platform suffix such as `.exe`).
fn install_bins(
    file_name: &str,
    content: &str,
) -> ResultDynError<Vec<(Package, Vec<String>)>> {
    let names = |v: Option<&serde_json::Value>| -> Vec<String> {
        v.and_then(|b| b.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|n| n.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(match file_name {
        CRATES2_JSON => {
            let c: Crates2 = serde_json::from_str(content)?;
            c.installs
                .iter()
                .filter_map(|(k, v)| Some((parse_install_key(k)?, names(v.get("bins")))))
                .collect()
        }
        CRATES_TOML => {
            let c: CratesToml = toml::from_str(content)?;
            c.v1.iter()
                .filter_map(|(k, v)| {
                    let bins = v
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|n| n.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    Some((parse_install_key(k)?, bins))
                })
                .collect()
        }
        _ => Vec::new(),
    })
}

//------------------------------------------------------------------------------
pub(crate) struct CargoScanner;

impl EcosystemScanner for CargoScanner {
    fn primary_site_kinds(&self) -> &'static [SiteKind] {
        // the crates built in a project's target directory, and `cargo install` records
        &[SiteKind::Installed]
    }

    /// Downloaded crate sources and toolchains (hundreds of `Cargo.lock` files, and other ecosystems' files in crate test fixtures).
    fn cache_dirs(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Some(h) = cargo_home() {
            out.push(h.join("registry"));
            out.push(h.join("git"));
        }
        out.extend(rustup_home().map(Path::to_path_buf));
        out
    }

    /// The Cargo home is found without walking, so that a non-default `$CARGO_HOME` is included.
    fn default_origins(&self) -> Vec<Origin> {
        cargo_home()
            .filter(|h| install_record(h).is_some())
            .map(|h| origin(h.to_path_buf()))
            .into_iter()
            .collect()
    }

    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect {
        let has = |name: &str| items.iter().any(|i| i.file_name() == Some(name));
        // Cargo and rustup homes contain hundreds of `Cargo.lock` files from downloaded crate sources and toolchains; these are not projects. Claim a Cargo home's installs (if any) and do not descend. Homes are recognized by layout as well as by path, so that other users' homes (with `--all-users`) are also handled.
        if is_cargo_home(dir, items) {
            let origins = if install_record(dir).is_some() {
                vec![origin(dir.to_path_buf())]
            } else {
                Vec::new()
            };
            return Detect {
                origins,
                prune: true,
                ..Default::default()
            };
        }
        if is_rustup_home(dir, items) {
            return Detect {
                prune: true,
                ..Default::default()
            };
        }
        let origins = if has(CARGO_LOCK) {
            vec![origin(dir.to_path_buf())]
        } else {
            Vec::new()
        };
        // build output is large and contains no projects; vendored crates (`cargo vendor`) ship their own lockfiles
        let mut skip_children = Vec::new();
        if has(CARGO_TOML) {
            skip_children.push("target");
            if is_cargo_vendor(&dir.join(VENDOR)) {
                skip_children.push(VENDOR);
            }
        }
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
            let target = dir.join(TARGET);
            if !target_deps_dirs(&target).is_empty() {
                sites.push(Site::new(
                    Ecosystem::Cargo,
                    SiteKind::Installed,
                    PathShared::from(target),
                ));
            }
            sites.push(Site::new(
                Ecosystem::Cargo,
                SiteKind::Lockfile,
                PathShared::from(lock),
            ));
        }
        sites
    }

    fn site_to_packages(&self, site: &Site) -> Vec<Package> {
        if site.as_path().is_dir() {
            return built_packages(site.as_path())
                .into_iter()
                .map(|(p, _)| p)
                .collect();
        }
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

    /// For an install record, the binaries of each installed crate in the install root's `bin` directory; for a target directory, the files each built crate's units wrote to `deps`; `Cargo.lock` sites have none.
    fn site_to_artifacts(
        &self,
        site: &Site,
        wanted: &HashSet<&Package>,
    ) -> Vec<(Package, Artifacts)> {
        let path = site.as_path();
        if path.is_dir() {
            return built_packages(path)
                .into_iter()
                .filter(|(p, _)| wanted.contains(p))
                .map(|(p, files)| {
                    let files = files
                        .into_iter()
                        .map(|f| {
                            let exists = f.exists();
                            (f, exists)
                        })
                        .collect();
                    (
                        p,
                        Artifacts {
                            files,
                            dirs: Vec::new(),
                        },
                    )
                })
                .collect();
        }
        let (Some(file_name), Some(root)) =
            (path.file_name().and_then(|n| n.to_str()), path.parent())
        else {
            return Vec::new();
        };
        let Ok(content) = fs::read_to_string(path) else {
            return Vec::new();
        };
        let bin = root.join("bin");
        install_bins(file_name, &content)
            .unwrap_or_default()
            .into_iter()
            .filter(|(p, _)| wanted.contains(p))
            .map(|(p, bins)| {
                let files = bins
                    .iter()
                    .map(|b| {
                        let fp = bin.join(b);
                        let exists = fp.exists();
                        (fp, exists)
                    })
                    .collect();
                (
                    p,
                    Artifacts {
                        files,
                        dirs: Vec::new(),
                    },
                )
            })
            .collect()
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

        // `cargo vendor` output is skipped; a `vendor` directory of something else is not
        let crate_dir = root.join("vendor").join("serde");
        fs::create_dir_all(&crate_dir).unwrap();
        fs::write(crate_dir.join(CARGO_LOCK), LOCK_V4).unwrap();
        fs::write(crate_dir.join(".cargo-checksum.json"), "{}").unwrap();
        let d = CargoScanner.detect(root, &items_of(root));
        assert_eq!(d.skip_children, vec!["target", "vendor"]);
        let other = root.join("other");
        fs::create_dir_all(other.join("vendor").join("x")).unwrap();
        fs::write(other.join(CARGO_TOML), "").unwrap();
        let d = CargoScanner.detect(&other, &items_of(&other));
        assert_eq!(d.skip_children, vec!["target"]);
    }

    #[test]
    fn test_detect_other_users_homes_a() {
        // another user's Cargo and rustup homes (as with `--all-users`) are recognized by layout
        let dir = tempdir().unwrap();
        let home = dir.path().join("other");
        let cargo = home.join(".cargo");
        let src = cargo.join("registry/src/index.crates.io-x/base64-0.22.1");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join(CARGO_LOCK), LOCK_V4).unwrap();
        let d = CargoScanner.detect(&cargo, &items_of(&cargo));
        assert!(d.prune);
        assert!(d.origins.is_empty());

        // with an install record, the home is also an origin
        fs::write(cargo.join(CRATES2_JSON), CRATES2).unwrap();
        let d = CargoScanner.detect(&cargo, &items_of(&cargo));
        assert!(d.prune);
        assert_eq!(d.origins.len(), 1);
        assert_eq!(d.origins[0].as_path(), cargo);

        let rustup = home.join(".rustup");
        let tc = rustup.join("toolchains/stable/lib/rustlib/src/rust");
        fs::create_dir_all(&tc).unwrap();
        fs::write(tc.join(CARGO_LOCK), LOCK_V4).unwrap();
        let d = CargoScanner.detect(&rustup, &items_of(&rustup));
        assert!(d.prune);

        // a project's `.cargo` config directory is not a Cargo home
        let proj = home.join("proj");
        fs::create_dir_all(proj.join(".cargo")).unwrap();
        fs::write(proj.join(".cargo/config.toml"), "").unwrap();
        let d =
            CargoScanner.detect(&proj.join(".cargo"), &items_of(&proj.join(".cargo")));
        assert!(!d.prune);
    }

    #[test]
    fn test_discover_other_users_homes_a() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("other");
        let src = home.join(".cargo/registry/src/index.crates.io-x/base64-0.22.1");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join(CARGO_LOCK), LOCK_V4).unwrap();
        fs::write(home.join(".cargo").join(CRATES2_JSON), CRATES2).unwrap();
        let git = home.join(".cargo/git/checkouts/x-1/abc");
        fs::create_dir_all(&git).unwrap();
        fs::write(git.join(CARGO_LOCK), LOCK_V4).unwrap();
        let tc = home.join(".rustup/toolchains/stable/lib/rustlib/src/rust");
        fs::create_dir_all(&tc).unwrap();
        fs::write(tc.join(CARGO_LOCK), LOCK_V4).unwrap();
        let proj = home.join("proj");
        fs::create_dir_all(&proj).unwrap();
        fs::write(proj.join(CARGO_LOCK), LOCK_V4).unwrap();

        let scanners: Vec<&dyn EcosystemScanner> = vec![&CargoScanner];
        let mut found: Vec<PathBuf> =
            crate::walk::discover(&scanners, false, &[dir.path().to_path_buf()])
                .into_iter()
                .map(|o| o.as_path().to_path_buf())
                .collect();
        found.sort();
        assert_eq!(found, vec![home.join(".cargo"), proj]);
    }

    /// Write a dep-info file for a unit with outputs `outs` (relative to `deps`) and the given sources.
    fn write_dep_info(deps: &Path, stem: &str, outs: &[&str], srcs: &[PathBuf]) {
        fs::create_dir_all(deps).unwrap();
        let srcs: Vec<String> = srcs
            .iter()
            .map(|p| p.to_string_lossy().replace(' ', "\\ "))
            .collect();
        let mut content = String::new();
        for o in outs.iter().chain([&format!("{stem}.d").as_str()]) {
            content.push_str(&format!(
                "{}: {}\n\n",
                deps.join(o).display(),
                srcs.join(" ")
            ));
        }
        for s in &srcs {
            content.push_str(&format!("{s}:\n"));
        }
        content.push_str("\n# env-dep:CARGO_PKG_NAME=x\n");
        fs::write(deps.join(format!("{stem}.d")), content).unwrap();
        for o in outs {
            fs::write(deps.join(o), "").unwrap();
        }
    }

    #[test]
    fn test_split_make_paths_a() {
        assert_eq!(
            split_make_paths("/a/b\\ c.rs  /d.rs"),
            vec![PathBuf::from("/a/b c.rs"), PathBuf::from("/d.rs")]
        );
        let rules =
            dep_info_rules("/t/deps/x.d: /s/a.rs /s/b.rs\n\n/s/a.rs:\n# env-dep:X=1\n");
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].1.len(), 2);
        assert!(rules[1].1.is_empty());
    }

    #[test]
    fn test_built_packages_a() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("proj");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(CARGO_LOCK), LOCK_V4).unwrap();
        // sources in a Cargo home with a space in its path
        let reg = dir
            .path()
            .join("my home/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f");
        let deps = root.join("target/release/deps");

        // built: serde 1.0.200 (an rlib) and serde_json (a proc-macro .so, under a cross-build triple)
        write_dep_info(
            &deps,
            "serde-1",
            &["libserde-1.rlib", "libserde-1.rmeta"],
            &[reg.join("serde-1.0.200/src/lib.rs")],
        );
        let cross = root.join("target/x86_64-unknown-linux-gnu/debug/deps");
        write_dep_info(
            &cross,
            "serde_json-2",
            &["libserde_json-2.so"],
            &[reg.join("serde_json-1.0.117/src/lib.rs")],
        );
        // stale: serde 1.0.190 is not in the lockfile
        write_dep_info(
            &deps,
            "serde-3",
            &["libserde-3.rlib"],
            &[reg.join("serde-1.0.190/src/lib.rs")],
        );
        // only checked: `internal` has only `.rmeta`
        write_dep_info(
            &deps,
            "internal-4",
            &["libinternal-4.rmeta"],
            &[reg.join("internal-3.1.4/src/lib.rs")],
        );
        // the project's own crate (an executable) reads no dependency sources
        write_dep_info(&deps, "myapp-5", &["myapp-5"], &[root.join("src/main.rs")]);
        // a git dependency in a workspace checkout, named by its Cargo.toml
        let checkout = dir
            .path()
            .join("my home/.cargo/git/checkouts/utest-a1b2c3/41301d9");
        let krate = checkout.join("crates/utest");
        fs::create_dir_all(krate.join("src")).unwrap();
        fs::write(
            checkout.join(CARGO_TOML),
            "[workspace]\nmembers = [\"crates/*\"]\n",
        )
        .unwrap();
        fs::write(
            krate.join(CARGO_TOML),
            "[package]\nname = \"utest\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        write_dep_info(
            &deps,
            "utest-6",
            &["libutest-6.rlib"],
            &[krate.join("src/lib.rs")],
        );

        let target = root.join(TARGET);
        let sites = CargoScanner.origin_to_sites(
            &origin(root.clone()),
            &ScanConfig::new(false, false),
            FlagLog(false),
        );
        assert_eq!(
            sites[0],
            Site::new(
                Ecosystem::Cargo,
                SiteKind::Installed,
                PathShared::from(target.clone())
            )
        );
        assert_eq!(sites[1].kind, SiteKind::Lockfile);

        let site = &sites[0];
        let mut packages = CargoScanner.site_to_packages(site);
        packages.sort();
        let names: Vec<String> = packages.iter().map(|p| p.to_string()).collect();
        assert_eq!(
            names,
            vec!["serde-1.0.200", "serde_json-1.0.117", "utest-0.1.0"]
        );
        let utest = packages.iter().find(|p| p.name == "utest").unwrap();
        let vcs = utest
            .direct_url
            .as_ref()
            .unwrap()
            .vcs_info
            .as_ref()
            .unwrap();
        assert_eq!(vcs.commit_id, "41301d9d61f2e0ad2b6f2a3f3d2c0b1e3c7e0d8a");

        // artifacts are the files the units wrote to `deps`
        let serde = packages.iter().find(|p| p.name == "serde").unwrap();
        let wanted: HashSet<&Package> = [serde].into_iter().collect();
        let arts = CargoScanner.site_to_artifacts(site, &wanted);
        assert_eq!(arts.len(), 1);
        let files: Vec<PathBuf> =
            arts[0].1.files.iter().map(|(f, _)| f.clone()).collect();
        assert_eq!(
            files,
            vec![
                deps.join("libserde-1.rlib"),
                deps.join("libserde-1.rmeta"),
                deps.join("serde-1.d")
            ]
        );
        assert!(arts[0].1.files.iter().all(|(_, exists)| *exists));
    }

    #[test]
    fn test_built_packages_b() {
        // a project that was never built has no target site; one only checked has a site without packages
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join(CARGO_LOCK), LOCK_V4).unwrap();
        let config = ScanConfig::new(false, false);
        let sites =
            CargoScanner.origin_to_sites(&origin(root.clone()), &config, FlagLog(false));
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, SiteKind::Lockfile);

        let reg = dir.path().join(".cargo/registry/src/index.crates.io-x");
        write_dep_info(
            &root.join("target/debug/deps"),
            "serde-1",
            &["libserde-1.rmeta"],
            &[reg.join("serde-1.0.200/src/lib.rs")],
        );
        let sites =
            CargoScanner.origin_to_sites(&origin(root.clone()), &config, FlagLog(false));
        assert_eq!(sites[0].kind, SiteKind::Installed);
        assert!(CargoScanner.site_to_packages(&sites[0]).is_empty());
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
