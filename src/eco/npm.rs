//! npm (JavaScript/TypeScript) scanner.
//!
//! Origins are (1) project directories with a `package.json` and either a `node_modules` directory or a lockfile, and (2) install directories: a `node_modules` directory whose parent has no `package.json` (global installs, nvm, volta). Sites follow the "installed where it exists, else lockfile" rule: an origin's `node_modules` (`SiteKind::Installed`) if present, otherwise its lockfile (`SiteKind::Lockfile`).
//!
//! Directories inside editor `extensions` directories (VS Code and its forks) are not npm origins; extensions are handled by the VS Code scanner.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value as JsonValue;

use crate::eco::Detect;
use crate::eco::DirItem;
use crate::eco::EcosystemScanner;
use crate::eco::EDITOR_DIRS;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::origin_site::SiteKind;
use crate::package::Package;
use crate::package_durl::DirectURL;
use crate::package_durl::VcsInfo;
use crate::path_shared::PathShared;
use crate::util::path_normalize;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::ScanConfig;
use crate::version_spec::VersionSpec;

const PACKAGE_JSON: &str = "package.json";
const NODE_MODULES: &str = "node_modules";
const SHRINKWRAP: &str = "npm-shrinkwrap.json";
const PACKAGE_LOCK: &str = "package-lock.json";
const PNPM_LOCK: &str = "pnpm-lock.yaml";
const YARN_LOCK: &str = "yarn.lock";
/// Bun's text lockfile (default since Bun 1.2). The older binary `bun.lockb` is not supported.
const BUN_LOCK: &str = "bun.lock";
/// Children of a project directory never walked: installed packages (read by the scanner from the origin) and framework build output/caches, which contain `package.json` and `node_modules` stubs but are not projects.
const SKIP_CHILDREN: [&str; 4] = [NODE_MODULES, ".next", ".nuxt", ".svelte-kit"];
/// Lockfiles in order of precedence; `npm-shrinkwrap.json` takes precedence over `package-lock.json` in npm.
const LOCKFILES: [&str; 5] = [SHRINKWRAP, PACKAGE_LOCK, PNPM_LOCK, YARN_LOCK, BUN_LOCK];

/// Registries whose tarball URLs denote the public npm registry; packages from these have no `direct_url`.
const NPM_REGISTRIES: [&str; 2] = [
    "https://registry.npmjs.org/",
    "https://registry.yarnpkg.com/",
];

//------------------------------------------------------------------------------
/// Return true if `path` is at or below an editor `extensions` directory (e.g. `~/.vscode/extensions/...`).
fn in_editor_extensions(path: &Path) -> bool {
    let names: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    names
        .windows(2)
        .any(|w| EDITOR_DIRS.contains(&w[0]) && w[1] == "extensions")
}

fn find_lockfile(dir: &Path) -> Option<PathBuf> {
    LOCKFILES.iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

fn has_node_modules(dir: &Path) -> bool {
    dir.join(NODE_MODULES).is_dir()
}

/// A project directory is an origin if it has something resolved to read.
fn is_project_origin(dir: &Path) -> bool {
    dir.join(PACKAGE_JSON).is_file()
        && (has_node_modules(dir) || find_lockfile(dir).is_some())
}

/// A `node_modules` directory without a sibling `package.json` is an install directory (e.g. a global `lib/node_modules`).
fn is_install_dir(dir: &Path) -> bool {
    dir.file_name().is_some_and(|n| n == NODE_MODULES)
        && dir.is_dir()
        && !dir.parent().is_some_and(|p| p.join(PACKAGE_JSON).is_file())
}

/// Global install directories found without walking. Locations under home (nvm, volta) are found by the walk.
fn global_install_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(prefix) = env::var_os("NPM_CONFIG_PREFIX").filter(|p| !p.is_empty()) {
        let prefix = PathBuf::from(prefix);
        dirs.push(prefix.join("lib").join(NODE_MODULES));
        dirs.push(prefix.join(NODE_MODULES)); // Windows layout
    }
    if env::consts::OS == "windows" {
        if let Some(appdata) = env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm").join(NODE_MODULES));
        }
    } else {
        dirs.push(PathBuf::from("/usr/local/lib/node_modules"));
        dirs.push(PathBuf::from("/usr/lib/node_modules"));
        if env::consts::OS == "macos" {
            dirs.push(PathBuf::from("/opt/homebrew/lib/node_modules"));
        }
    }
    dirs.into_iter().filter(|d| is_install_dir(d)).collect()
}

fn origin(dir: PathBuf) -> Origin {
    Origin::new(Ecosystem::Npm, PathShared::from(dir))
}

//------------------------------------------------------------------------------
/// Provenance of a resolved package. Returns None if the package is local (`file:`, `link:`) and should be excluded; Some(None) for the public npm registry; Some(Some(_)) otherwise.
fn resolved_to_provenance(resolved: Option<&str>) -> Option<Option<DirectURL>> {
    let Some(r) = resolved.filter(|r| !r.is_empty()) else {
        return Some(None);
    };
    if r.starts_with("file:") || r.starts_with("link:") {
        return None;
    }
    if NPM_REGISTRIES.iter().any(|reg| r.starts_with(reg)) {
        return Some(None);
    }
    let url = r.strip_prefix("git+").unwrap_or(r);
    let is_git =
        r.starts_with("git+") || r.starts_with("git:") || r.starts_with("github:");
    let (url, commit) = match url.split_once('#') {
        Some((u, c)) if is_git => (u, Some(c)),
        _ => (url, None),
    };
    Some(Some(DirectURL {
        url: url.to_string(),
        vcs_info: commit.map(|c| VcsInfo {
            commit_id: c.to_string(),
            vcs: "git".to_string(),
            revision: None,
        }),
    }))
}

fn package(name: &str, version: &str, direct_url: Option<DirectURL>) -> Package {
    Package::new(Ecosystem::Npm, name, VersionSpec::new(version), direct_url)
}

/// Split `name@version` where `name` may be scoped (`@scope/name@1.0.0`).
fn split_name_version(s: &str) -> Option<(&str, &str)> {
    let at = s[1..].find('@')? + 1;
    Some((&s[..at], &s[at + 1..]))
}

//------------------------------------------------------------------------------
// node_modules

/// Read the package at `dir` (its `package.json` name and version), then any nested `node_modules`.
fn read_package_dir(dir: &Path, out: &mut Vec<Package>) {
    if let Ok(content) = fs::read_to_string(dir.join(PACKAGE_JSON)) {
        if let Ok(pj) = serde_json::from_str::<JsonValue>(&content) {
            if let (Some(name), Some(version)) = (
                pj.get("name").and_then(JsonValue::as_str),
                pj.get("version").and_then(JsonValue::as_str),
            ) {
                out.push(package(name, version, None));
            }
        }
    }
    let nested = dir.join(NODE_MODULES);
    if nested.is_dir() {
        walk_node_modules(&nested, out);
    }
}

/// Real (non-symlink) child directories.
fn child_dirs(dir: &Path) -> Vec<PathBuf> {
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Collect all packages installed in a `node_modules` directory, including scoped and nested packages and pnpm's `.pnpm` store. Symlinks are never followed: they are workspace links, `npm link`s, or pnpm's links into its store (which is read directly).
fn walk_node_modules(nm: &Path, out: &mut Vec<Package>) {
    for child in child_dirs(nm) {
        let Some(name) = child.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name == ".pnpm" {
            // .pnpm/<name>@<version>/node_modules/<name> is the package; its dependencies there are symlinks
            for entry in child_dirs(&child) {
                let inner = entry.join(NODE_MODULES);
                if inner.is_dir() {
                    walk_node_modules(&inner, out);
                }
            }
        } else if name.starts_with('.') {
            // .bin, .cache, .vite, ...
            continue;
        } else if name.starts_with('@') {
            for scoped in child_dirs(&child) {
                read_package_dir(&scoped, out);
            }
        } else {
            read_package_dir(&child, out);
        }
    }
}

//------------------------------------------------------------------------------
// package-lock.json / npm-shrinkwrap.json

/// Read packages from `package-lock.json` (or `npm-shrinkwrap.json`). Versions 2 and 3 use the flat `packages` map; version 1 uses the nested `dependencies` tree. The root project, workspace members, and links are excluded.
fn packages_from_package_lock(content: &str) -> ResultDynError<Vec<Package>> {
    let v: JsonValue = serde_json::from_str(content)?;
    let mut out = Vec::new();
    if let Some(pkgs) = v.get("packages").and_then(JsonValue::as_object) {
        for (key, entry) in pkgs {
            // only installed dependencies: keys under node_modules/ (the root is "", members are e.g. "packages/a")
            let Some(idx) = key.rfind("node_modules/") else {
                continue;
            };
            if entry.get("link").and_then(JsonValue::as_bool) == Some(true) {
                continue;
            }
            let name = entry
                .get("name")
                .and_then(JsonValue::as_str)
                .unwrap_or(&key[idx + "node_modules/".len()..]);
            let Some(version) = entry.get("version").and_then(JsonValue::as_str) else {
                continue;
            };
            let resolved = entry.get("resolved").and_then(JsonValue::as_str);
            if let Some(durl) = resolved_to_provenance(resolved) {
                out.push(package(name, version, durl));
            }
        }
    } else if let Some(deps) = v.get("dependencies").and_then(JsonValue::as_object) {
        packages_from_lock_v1(deps, &mut out);
    }
    Ok(out)
}

fn packages_from_lock_v1(
    deps: &serde_json::Map<String, JsonValue>,
    out: &mut Vec<Package>,
) {
    for (name, entry) in deps {
        if let Some(version) = entry.get("version").and_then(JsonValue::as_str) {
            if version.starts_with("file:") || version.starts_with("link:") {
                // local
            } else if version.contains(':') {
                // a URL in place of a version (git, tarball): no registry version to audit
                if let Some(durl) = resolved_to_provenance(Some(version)) {
                    out.push(package(name, "", durl));
                }
            } else {
                let resolved = entry.get("resolved").and_then(JsonValue::as_str);
                if let Some(durl) = resolved_to_provenance(resolved) {
                    out.push(package(name, version, durl));
                }
            }
        }
        if let Some(nested) = entry.get("dependencies").and_then(JsonValue::as_object) {
            packages_from_lock_v1(nested, out);
        }
    }
}

//------------------------------------------------------------------------------
// pnpm-lock.yaml

/// Parse a pnpm `packages` key into name and version. Version 5 keys are `/name/1.0.0_peer@x`; version 6 keys are `/name@1.0.0(peer@x)`; version 9 keys are `name@1.0.0` (peer suffixes appear only under `snapshots`).
fn parse_pnpm_key(key: &str, major: u32) -> Option<(String, String)> {
    let key = key.strip_prefix('/').unwrap_or(key);
    if major >= 6 {
        let key = key.split('(').next()?;
        let (name, version) = split_name_version(key)?;
        Some((name.to_string(), version.to_string()))
    } else {
        let (name, version) = key.rsplit_once('/')?;
        let version = version.split('_').next()?;
        Some((name.to_string(), version.to_string()))
    }
}

fn lockfile_major(v: &JsonValue) -> u32 {
    let raw = match v.get("lockfileVersion") {
        Some(JsonValue::String(s)) => s.clone(),
        Some(JsonValue::Number(n)) => n.to_string(),
        _ => return 0,
    };
    raw.split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .unwrap_or(0)
}

/// Read packages from `pnpm-lock.yaml` (versions 5, 6, and 9). Workspace projects (`importers`) are not under `packages` and so are excluded. Entries with explicit `name`/`version` fields (non-registry packages in older formats) use those.
fn packages_from_pnpm_lock(content: &str) -> ResultDynError<Vec<Package>> {
    let v: JsonValue = serde_saphyr::from_str(content)?;
    let major = lockfile_major(&v);
    let mut out = Vec::new();
    let Some(pkgs) = v.get("packages").and_then(JsonValue::as_object) else {
        return Ok(out);
    };
    for (key, entry) in pkgs {
        let parsed = parse_pnpm_key(key, major);
        let name = entry
            .get("name")
            .and_then(JsonValue::as_str)
            .map(str::to_string)
            .or_else(|| parsed.as_ref().map(|(n, _)| n.clone()));
        let version = entry
            .get("version")
            .and_then(JsonValue::as_str)
            .map(str::to_string)
            .or_else(|| parsed.as_ref().map(|(_, v)| v.clone()));
        let (Some(name), Some(version)) = (name, version) else {
            continue;
        };
        if version.starts_with(|c: char| c.is_ascii_digit()) {
            let tarball = entry
                .get("resolution")
                .and_then(|r| r.get("tarball"))
                .and_then(JsonValue::as_str);
            if let Some(durl) = resolved_to_provenance(tarball) {
                out.push(package(&name, &version, durl));
            }
        } else if version.contains("://") {
            // a URL in place of a version (git, tarball)
            if let Some(durl) = resolved_to_provenance(Some(&version)) {
                out.push(package(&name, "", durl));
            }
        }
        // else file:, link:, workspace: (local)
    }
    Ok(out)
}

//------------------------------------------------------------------------------
// yarn.lock

/// Read packages from `yarn.lock`: yarn berry (v2+, YAML with `__metadata`) or yarn classic (v1, a custom format).
fn packages_from_yarn_lock(content: &str) -> ResultDynError<Vec<Package>> {
    if content.contains("__metadata:") {
        packages_from_yarn_berry(content)
    } else {
        Ok(packages_from_yarn_classic(content))
    }
}

/// Yarn berry entries carry a `resolution` such as `lodash@npm:4.17.21`; the protocol determines provenance. Workspace, link, portal, and file resolutions are local and excluded.
fn packages_from_yarn_berry(content: &str) -> ResultDynError<Vec<Package>> {
    let v: JsonValue = serde_saphyr::from_str(content)?;
    let mut out = Vec::new();
    let Some(entries) = v.as_object() else {
        return Ok(out);
    };
    for (key, entry) in entries {
        if key == "__metadata" {
            continue;
        }
        let Some(resolution) = entry.get("resolution").and_then(JsonValue::as_str) else {
            continue;
        };
        let Some((name, rest)) = split_name_version(resolution) else {
            continue;
        };
        let version = entry.get("version").and_then(JsonValue::as_str);
        let Some((protocol, reference)) = rest.split_once(':') else {
            continue;
        };
        match protocol {
            "npm" => out.push(package(name, version.unwrap_or(reference), None)),
            // a patched registry package
            "patch" => {
                if let Some(version) = version {
                    out.push(package(name, version, None));
                }
            }
            "workspace" | "link" | "portal" | "file" => {}
            _ => {
                if let Some(Some(durl)) = resolved_to_provenance(Some(rest)) {
                    out.push(package(name, version.unwrap_or(""), Some(durl)));
                }
            }
        }
    }
    Ok(out)
}

/// Yarn classic blocks start with an unindented header of comma-separated descriptors (`"lodash@^4.17.0", lodash@^4.17.21:`), followed by indented `version` and `resolved` fields.
fn packages_from_yarn_classic(content: &str) -> Vec<Package> {
    struct Block {
        name: Option<String>,
        version: Option<String>,
        resolved: Option<String>,
        local: bool,
    }
    fn emit(block: Option<Block>, out: &mut Vec<Package>) {
        if let Some(Block {
            name: Some(name),
            version: Some(version),
            resolved,
            local: false,
        }) = block
        {
            if let Some(durl) = resolved_to_provenance(resolved.as_deref()) {
                out.push(package(&name, &version, durl));
            }
        }
    }
    let unquote = |s: &str| s.trim().trim_matches('"').to_string();

    let mut out = Vec::new();
    let mut block: Option<Block> = None;
    for line in content.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        if !line.starts_with(' ') {
            emit(block.take(), &mut out);
            let header = line.trim_end().trim_end_matches(':');
            let first = unquote(header.split(", ").next().unwrap_or(""));
            let local = ["@file:", "@link:", "@portal:", "@workspace:"]
                .iter()
                .any(|p| first.contains(p));
            block = Some(Block {
                name: split_name_version(&first).map(|(n, _)| n.to_string()),
                version: None,
                resolved: None,
                local,
            });
        } else if let Some(b) = block.as_mut() {
            let field = line.trim();
            if let Some(v) = field.strip_prefix("version ") {
                b.version = Some(unquote(v));
            } else if let Some(r) = field.strip_prefix("resolved ") {
                b.resolved = Some(unquote(r));
            }
        }
    }
    emit(block.take(), &mut out);
    out
}

//------------------------------------------------------------------------------
// bun.lock

/// Remove commas that directly precede a closing `}` or `]` (ignoring whitespace), outside of strings. `bun.lock` is JSON with trailing commas, which `serde_json` rejects.
fn strip_trailing_commas(content: &str) -> String {
    let chars: Vec<char> = content.chars().collect();
    let mut out = String::with_capacity(content.len());
    let mut in_string = false;
    let mut escaped = false;
    for (i, &c) in chars.iter().enumerate() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            ',' => {
                let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
                if !matches!(next, Some('}') | Some(']')) {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Read packages from `bun.lock`. Each `packages` entry is an array whose first element is `name@spec`; keys are resolution paths (e.g. `parent/child` for a nested duplicate) and are not used for names. A registry package is `["name@1.0.0", registry, info, integrity]`, where an empty registry is the default npm registry. Workspace, `file:`, `link:` and `root:` entries are local and excluded; git, GitHub and tarball specs carry provenance without a version (→ unaudited).
fn packages_from_bun_lock(content: &str) -> ResultDynError<Vec<Package>> {
    let v: JsonValue = serde_json::from_str(&strip_trailing_commas(content))?;
    let mut out = Vec::new();
    let Some(pkgs) = v.get("packages").and_then(JsonValue::as_object) else {
        return Ok(out);
    };
    for entry in pkgs.values() {
        let Some(arr) = entry.as_array() else {
            continue;
        };
        let Some((name, spec)) = arr
            .first()
            .and_then(JsonValue::as_str)
            .and_then(split_name_version)
        else {
            continue;
        };
        if spec.starts_with(|c: char| c.is_ascii_digit()) {
            let registry = arr
                .get(1)
                .and_then(JsonValue::as_str)
                .filter(|r| !r.is_empty());
            let durl = match registry {
                Some(url)
                    if !NPM_REGISTRIES
                        .iter()
                        .any(|reg| url.starts_with(reg.trim_end_matches('/'))) =>
                {
                    Some(DirectURL {
                        url: url.to_string(),
                        vcs_info: None,
                    })
                }
                _ => None,
            };
            out.push(package(name, spec, durl));
        } else if ["workspace:", "file:", "link:", "root:"]
            .iter()
            .any(|p| spec.starts_with(p))
        {
            // local
        } else if let Some(Some(durl)) = resolved_to_provenance(Some(spec)) {
            out.push(package(name, "", Some(durl)));
        }
    }
    Ok(out)
}

//------------------------------------------------------------------------------
pub(crate) struct NpmScanner;

impl EcosystemScanner for NpmScanner {
    fn default_origins(&self) -> Vec<Origin> {
        global_install_dirs().into_iter().map(origin).collect()
    }

    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect {
        if dir.file_name().is_some_and(|n| n == NODE_MODULES) {
            // A node_modules reached by the walk has no package.json beside it (projects skip their node_modules), so it is an install directory. Its contents are read by the scanner, never walked.
            let origins = if !in_editor_extensions(dir) && is_install_dir(dir) {
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
        let has = |name: &str| items.iter().any(|i| i.file_name() == Some(name));
        if !has(PACKAGE_JSON) {
            return Detect::default();
        }
        let is_origin = !in_editor_extensions(dir)
            && (items
                .iter()
                .any(|i| i.file_name() == Some(NODE_MODULES) && i.file_type.is_dir())
                || LOCKFILES.iter().any(|l| has(l)));
        Detect {
            origins: if is_origin {
                vec![origin(dir.to_path_buf())]
            } else {
                Vec::new()
            },
            prune: false,
            skip_children: SKIP_CHILDREN.to_vec(),
        }
    }

    /// Accept a `package.json` or lockfile, a `node_modules` directory, or a directory containing a `package.json`.
    fn accepts(&self, path: &Path) -> bool {
        let Ok(p) = path_normalize(path, false) else {
            return false;
        };
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if p.is_file() {
            return name == PACKAGE_JSON || LOCKFILES.contains(&name);
        }
        p.is_dir() && (name == NODE_MODULES || p.join(PACKAGE_JSON).is_file())
    }

    /// A workspace member without its own `node_modules` or lockfile resolves to the nearest ancestor project that has one.
    fn normalize_origin(&self, path: &Path) -> ResultDynError<Origin> {
        let p = path_normalize(path, true)?;
        let dir = if p.is_file() {
            p.parent()
                .ok_or("Cannot get parent directory")?
                .to_path_buf()
        } else {
            p
        };
        if dir.file_name().is_some_and(|n| n == NODE_MODULES) {
            return Ok(origin(dir));
        }
        dir.ancestors()
            .find(|d| is_project_origin(d))
            .map(|d| origin(d.to_path_buf()))
            .ok_or_else(|| {
                format!("No node_modules or lockfile found for {dir:?} or its parents; run `npm install`")
                    .into()
            })
    }

    /// Installed (`node_modules`) if present, else the first lockfile found.
    fn origin_to_sites(
        &self,
        origin: &Origin,
        _config: &ScanConfig,
        _log: FlagLog,
    ) -> Vec<Site> {
        let dir = origin.as_path();
        let site =
            |kind, path: PathBuf| Site::new(Ecosystem::Npm, kind, PathShared::from(path));
        if dir.file_name().is_some_and(|n| n == NODE_MODULES) {
            return vec![site(SiteKind::Installed, dir.to_path_buf())];
        }
        if has_node_modules(dir) {
            return vec![site(SiteKind::Installed, dir.join(NODE_MODULES))];
        }
        find_lockfile(dir)
            .map(|l| site(SiteKind::Lockfile, l))
            .into_iter()
            .collect()
    }

    fn site_to_packages(&self, site: &Site) -> Vec<Package> {
        let path = site.as_path();
        let packages = match site.kind {
            SiteKind::Installed => {
                let mut out = Vec::new();
                walk_node_modules(path, &mut out);
                out
            }
            _ => {
                let Ok(content) = fs::read_to_string(path) else {
                    return Vec::new();
                };
                let parsed = match path.file_name().and_then(|n| n.to_str()) {
                    Some(SHRINKWRAP) | Some(PACKAGE_LOCK) => {
                        packages_from_package_lock(&content)
                    }
                    Some(PNPM_LOCK) => packages_from_pnpm_lock(&content),
                    Some(YARN_LOCK) => packages_from_yarn_lock(&content),
                    Some(BUN_LOCK) => packages_from_bun_lock(&content),
                    _ => Ok(Vec::new()),
                };
                // a malformed file yields no packages rather than failing the whole scan
                parsed.unwrap_or_default()
            }
        };
        // the same name and version can be installed or locked in many places within one site
        let unique: HashSet<Package> = packages.into_iter().collect();
        unique.into_iter().collect()
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn names(packages: &[Package]) -> Vec<String> {
        let mut v: Vec<String> = packages.iter().map(|p| p.to_string()).collect();
        v.sort();
        v.dedup();
        v
    }

    fn get<'a>(packages: &'a [Package], name: &str) -> &'a Package {
        packages.iter().find(|p| p.name == name).unwrap()
    }

    //--------------------------------------------------------------------------
    // package-lock

    const PACKAGE_LOCK_V3: &str = r#"{
  "name": "myapp",
  "version": "1.0.0",
  "lockfileVersion": 3,
  "requires": true,
  "packages": {
    "": {"name": "myapp", "version": "1.0.0", "workspaces": ["packages/*"]},
    "node_modules/lodash": {"version": "4.17.15", "resolved": "https://registry.npmjs.org/lodash/-/lodash-4.17.15.tgz"},
    "node_modules/@babel/core": {"version": "7.24.0", "resolved": "https://registry.npmjs.org/@babel/core/-/core-7.24.0.tgz"},
    "node_modules/debug": {"version": "4.3.4"},
    "node_modules/debug/node_modules/ms": {"version": "2.1.2"},
    "node_modules/ms": {"version": "2.1.3"},
    "node_modules/my-alias": {"name": "left-pad", "version": "1.3.0"},
    "node_modules/gitdep": {"version": "0.1.0", "resolved": "git+ssh://git@github.com/o/gitdep.git#0123abc"},
    "node_modules/private": {"version": "2.0.0", "resolved": "https://npm.example.com/private/-/private-2.0.0.tgz"},
    "node_modules/localtar": {"version": "1.0.0", "resolved": "file:../localtar-1.0.0.tgz"},
    "node_modules/member-a": {"resolved": "packages/a", "link": true},
    "packages/a": {"name": "member-a", "version": "0.0.1"}
  }
}"#;

    const PACKAGE_LOCK_V1: &str = r#"{
  "name": "old",
  "version": "1.0.0",
  "lockfileVersion": 1,
  "dependencies": {
    "lodash": {"version": "4.17.15", "resolved": "https://registry.npmjs.org/lodash/-/lodash-4.17.15.tgz"},
    "debug": {"version": "4.3.4", "dependencies": {"ms": {"version": "2.1.2"}}},
    "gitdep": {"version": "git+https://github.com/o/gitdep.git#0123abc"},
    "local": {"version": "file:../local"}
  }
}"#;

    #[test]
    fn test_package_lock_v3_a() {
        let packages = packages_from_package_lock(PACKAGE_LOCK_V3).unwrap();
        // root, workspace member, link, and file: tarball are excluded; nested versions are included
        assert_eq!(
            names(&packages),
            vec![
                "@babel/core-7.24.0",
                "debug-4.3.4",
                "gitdep-0.1.0",
                "left-pad-1.3.0",
                "lodash-4.17.15",
                "ms-2.1.2",
                "ms-2.1.3",
                "private-2.0.0",
            ]
        );
        assert!(packages.iter().all(|p| p.ecosystem == Ecosystem::Npm));
        assert_eq!(get(&packages, "lodash").direct_url, None);
        let git = get(&packages, "gitdep").direct_url.clone().unwrap();
        assert_eq!(git.url, "ssh://git@github.com/o/gitdep.git");
        assert_eq!(git.vcs_info.unwrap().commit_id, "0123abc");
        let private = get(&packages, "private").direct_url.clone().unwrap();
        assert!(private.url.starts_with("https://npm.example.com/"));
        assert!(private.vcs_info.is_none());
    }

    #[test]
    fn test_package_lock_v1_a() {
        let packages = packages_from_package_lock(PACKAGE_LOCK_V1).unwrap();
        assert_eq!(
            names(&packages),
            vec!["debug-4.3.4", "gitdep-", "lodash-4.17.15", "ms-2.1.2"]
        );
        // a git URL in place of a version: provenance kept, no version (so it is unaudited)
        let git = get(&packages, "gitdep");
        assert_eq!(git.version.to_string(), "");
        assert_eq!(
            git.direct_url.clone().unwrap().url,
            "https://github.com/o/gitdep.git"
        );
    }

    #[test]
    fn test_package_lock_invalid_a() {
        assert!(packages_from_package_lock("not json").is_err());
        assert!(packages_from_package_lock("{}").unwrap().is_empty());
    }

    //--------------------------------------------------------------------------
    // pnpm

    const PNPM_V9: &str = r#"lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .:
    dependencies:
      react-dom:
        specifier: ^18.2.0
        version: 18.2.0(react@18.2.0)

  packages/a:
    dependencies:
      lodash:
        specifier: 4.17.15
        version: 4.17.15

packages:

  '@types/node@20.11.5':
    resolution: {integrity: sha512-g557vgQjUUfN76MZAN/dt1z3dzcUsimuysco0KeluHgrPdJXkP/XdAURgyO2W9fZWHRtRBiVKzKn8vyOAwlG+w==}

  lodash@4.17.15:
    resolution: {integrity: sha512-8xOcRHvCjnocdS5cpwXQXVzmmh5e5+saE2QGoeQmbKmRS6J3VQppPOIt0MnmE+4xlZoumy0GPG0D0MVIQbNA1A==}

  react-dom@18.2.0:
    resolution: {integrity: sha512-6IMTriUmvsjHUjNtEDudZfuDQUoWXVxKHhlEGSk81n4YFS+r/Kl99wXiwlVXtPBtJenozv2P+hxDsw9eA7Xo6g==}
    peerDependencies:
      react: ^18.2.0

  react@18.2.0:
    resolution: {integrity: sha512-/3IjMdb2L9QbBdWiW5e3P2/npwMBaU9mHCSCUzNln0ZCYbcfTsGbTJrU/kGemdH2IWmB2ioZ+zkxtmq6g09fGQ==}

  private@2.0.0:
    resolution: {tarball: https://npm.example.com/private/-/private-2.0.0.tgz}

snapshots:

  react-dom@18.2.0(react@18.2.0):
    dependencies:
      react: 18.2.0
"#;

    const PNPM_V6: &str = r#"lockfileVersion: '6.0'

dependencies:
  react-dom:
    specifier: ^18.2.0
    version: 18.2.0(react@18.2.0)

packages:

  /@types/node@20.11.5:
    resolution: {integrity: sha512-x}
    dev: false

  /react-dom@18.2.0(react@18.2.0):
    resolution: {integrity: sha512-x}
    dev: false

  /react@18.2.0:
    resolution: {integrity: sha512-x}
    dev: false

  github.com/o/gitdep/0123abc:
    resolution: {tarball: https://codeload.github.com/o/gitdep/tar.gz/0123abc}
    name: gitdep
    version: 0.1.0
    dev: false
"#;

    const PNPM_V5: &str = r#"lockfileVersion: 5.4

specifiers:
  react-dom: ^18.2.0

packages:

  /@types/node/20.11.5:
    resolution: {integrity: sha512-x}
    dev: false

  /react-dom/18.2.0_react@18.2.0:
    resolution: {integrity: sha512-x}
    dev: false

  /react/18.2.0:
    resolution: {integrity: sha512-x}
    dev: false
"#;

    #[test]
    fn test_pnpm_v9_a() {
        let packages = packages_from_pnpm_lock(PNPM_V9).unwrap();
        // importers (workspace projects) are excluded; snapshots do not duplicate
        assert_eq!(
            names(&packages),
            vec![
                "@types/node-20.11.5",
                "lodash-4.17.15",
                "private-2.0.0",
                "react-18.2.0",
                "react-dom-18.2.0",
            ]
        );
        assert!(get(&packages, "private").direct_url.is_some());
        assert_eq!(get(&packages, "lodash").direct_url, None);
    }

    #[test]
    fn test_pnpm_v6_a() {
        let packages = packages_from_pnpm_lock(PNPM_V6).unwrap();
        assert_eq!(
            names(&packages),
            vec![
                "@types/node-20.11.5",
                "gitdep-0.1.0",
                "react-18.2.0",
                "react-dom-18.2.0"
            ]
        );
        let git = get(&packages, "gitdep").direct_url.clone().unwrap();
        assert!(git.url.starts_with("https://codeload.github.com/"));
    }

    #[test]
    fn test_pnpm_v5_a() {
        let packages = packages_from_pnpm_lock(PNPM_V5).unwrap();
        assert_eq!(
            names(&packages),
            vec!["@types/node-20.11.5", "react-18.2.0", "react-dom-18.2.0"]
        );
    }

    #[test]
    fn test_parse_pnpm_key_a() {
        let pk = |k: &str, m: u32| parse_pnpm_key(k, m).unwrap();
        assert_eq!(pk("lodash@4.17.21", 9), ("lodash".into(), "4.17.21".into()));
        assert_eq!(
            pk("@scope/n@1.0.0(peer@2.0.0)", 9),
            ("@scope/n".into(), "1.0.0".into())
        );
        assert_eq!(
            pk("/@scope/n@1.0.0", 6),
            ("@scope/n".into(), "1.0.0".into())
        );
        assert_eq!(
            pk("/@scope/n/1.0.0_peer@2", 5),
            ("@scope/n".into(), "1.0.0".into())
        );
    }

    //--------------------------------------------------------------------------
    // yarn

    const YARN_CLASSIC: &str = r#"# THIS IS AN AUTOGENERATED FILE. DO NOT EDIT THIS FILE DIRECTLY.
# yarn lockfile v1


"@babel/code-frame@^7.0.0", "@babel/code-frame@^7.22.13":
  version "7.23.5"
  resolved "https://registry.yarnpkg.com/@babel/code-frame/-/code-frame-7.23.5.tgz#9009b69a8c602293476ad598ff53e4562e15c244"
  integrity sha512-x
  dependencies:
    "@babel/highlight" "^7.23.4"

lodash@^4.17.15:
  version "4.17.15"
  resolved "https://registry.yarnpkg.com/lodash/-/lodash-4.17.15.tgz#b447f6670a0455bbfeedd11392eff330ea097548"
  integrity sha512-x

gitdep@o/gitdep#main:
  version "0.1.0"
  resolved "git+https://github.com/o/gitdep.git#0123abc"

local@file:../local:
  version "1.0.0"
"#;

    const YARN_BERRY: &str = r#"# This file is generated by running "yarn install" inside your project.

__metadata:
  version: 8
  cacheKey: 10c0

"@babel/code-frame@npm:^7.0.0, @babel/code-frame@npm:^7.22.13":
  version: 7.23.5
  resolution: "@babel/code-frame@npm:7.23.5"
  checksum: 10c0/x
  languageName: node
  linkType: hard

"lodash@npm:^4.17.15":
  version: 4.17.15
  resolution: "lodash@npm:4.17.15"
  languageName: node
  linkType: hard

"myapp@workspace:.":
  version: 0.0.0-use.local
  resolution: "myapp@workspace:."
  languageName: unknown
  linkType: soft

"resolve@patch:resolve@npm%3A^1.22.1#optional!builtin<compat/resolve>":
  version: 1.22.8
  resolution: "resolve@patch:resolve@npm%3A1.22.8#optional!builtin<compat/resolve>::version=1.22.8&hash=c3c19d"
  languageName: node
  linkType: hard

"gitdep@https://github.com/o/gitdep.git#commit=0123abc":
  version: 0.1.0
  resolution: "gitdep@https://github.com/o/gitdep.git#commit=0123abc"
  languageName: node
  linkType: hard
"#;

    #[test]
    fn test_yarn_classic_a() {
        let packages = packages_from_yarn_lock(YARN_CLASSIC).unwrap();
        // file: is local and excluded
        assert_eq!(
            names(&packages),
            vec!["@babel/code-frame-7.23.5", "gitdep-0.1.0", "lodash-4.17.15"]
        );
        // the yarnpkg registry is the public registry
        assert_eq!(get(&packages, "lodash").direct_url, None);
        let git = get(&packages, "gitdep").direct_url.clone().unwrap();
        assert_eq!(git.url, "https://github.com/o/gitdep.git");
        assert_eq!(git.vcs_info.unwrap().commit_id, "0123abc");
    }

    #[test]
    fn test_yarn_berry_a() {
        let packages = packages_from_yarn_lock(YARN_BERRY).unwrap();
        // the workspace is excluded; a patched registry package is kept
        assert_eq!(
            names(&packages),
            vec![
                "@babel/code-frame-7.23.5",
                "gitdep-0.1.0",
                "lodash-4.17.15",
                "resolve-1.22.8"
            ]
        );
        assert_eq!(get(&packages, "resolve").direct_url, None);
        assert!(get(&packages, "gitdep").direct_url.is_some());
    }

    //--------------------------------------------------------------------------
    // bun

    const BUN_LOCK_V1: &str = r#"{
  "lockfileVersion": 1,
  "workspaces": {
    "": {
      "name": "myapp",
      "dependencies": {
        "discord.js": "^14.14.0",
        "member-a": "workspace:*",
      },
    },
    "packages/a": {
      "name": "member-a",
      "dependencies": {
        "lodash": "4.17.15",
      },
    },
  },
  "packages": {
    "@discordjs/rest": ["@discordjs/rest@2.6.0", "", { "dependencies": { "@discordjs/collection": "^2.1.1" } }, "sha512-x"],

    "@discordjs/rest/@discordjs/collection": ["@discordjs/collection@2.1.1", "", {}, "sha512-y"],

    "@discordjs/collection": ["@discordjs/collection@1.5.3", "", {}, "sha512-z"],

    "discord.js": ["discord.js@14.14.1", "", { "dependencies": { "@discordjs/rest": "^2.6.0" } }, "sha512-w"],

    "lodash": ["lodash@4.17.15", "https://registry.npmjs.org/", {}, "sha512-v"],

    "private": ["private@2.0.0", "https://npm.example.com/", {}, "sha512-u"],

    "member-a": ["member-a@workspace:packages/a"],

    "local": ["local@file:../local", {}],

    "linked": ["linked@link:linked", {}],

    "gitdep": ["gitdep@github:o/gitdep#0123abc", {}, "o-gitdep-0123abc"],

    "tardep": ["tardep@https://example.com/tardep-1.0.0.tgz", {}],

    "odd,name": ["odd,name@1.0.0", "", { "description": "a string with ,} and ,] inside" }, "sha512-t"],
  }
}
"#;

    #[test]
    fn test_bun_lock_a() {
        let packages = packages_from_bun_lock(BUN_LOCK_V1).unwrap();
        // names come from the first element (not the key); workspace, file:, link: are excluded
        assert_eq!(
            names(&packages),
            vec![
                "@discordjs/collection-1.5.3",
                "@discordjs/collection-2.1.1",
                "@discordjs/rest-2.6.0",
                "discord.js-14.14.1",
                "gitdep-",
                "lodash-4.17.15",
                "odd,name-1.0.0",
                "private-2.0.0",
                "tardep-",
            ]
        );
        assert!(packages.iter().all(|p| p.ecosystem == Ecosystem::Npm));
        // default and explicit public registry: no provenance
        assert_eq!(get(&packages, "discord.js").direct_url, None);
        assert_eq!(get(&packages, "lodash").direct_url, None);
        // custom registry
        let private = get(&packages, "private").direct_url.clone().unwrap();
        assert_eq!(private.url, "https://npm.example.com/");
        // git: provenance kept, no version (so it is unaudited)
        let git = get(&packages, "gitdep");
        assert_eq!(git.version.to_string(), "");
        let vcs = git.direct_url.clone().unwrap().vcs_info.unwrap();
        assert_eq!(vcs.commit_id, "0123abc");
        assert!(get(&packages, "tardep").direct_url.is_some());
    }

    #[test]
    fn test_bun_lock_invalid_a() {
        assert!(packages_from_bun_lock("not json").is_err());
        assert!(packages_from_bun_lock("{}").unwrap().is_empty());
        assert!(packages_from_bun_lock(r#"{"packages": {}, }"#)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_strip_trailing_commas_a() {
        assert_eq!(
            strip_trailing_commas(r#"{"a": [1, 2, ], }"#),
            r#"{"a": [1, 2 ] }"#
        );
        // commas inside strings, including escaped quotes, are kept
        assert_eq!(
            strip_trailing_commas(r#"{"a": "x,}", "b": "\",]", }"#),
            r#"{"a": "x,}", "b": "\",]" }"#
        );
        assert_eq!(strip_trailing_commas("[1,\n  2,\n]"), "[1,\n  2\n]");
    }

    #[test]
    fn test_bun_lockfile_site_a() {
        // a bun-only project (no node_modules) is an origin with its bun.lock as the site
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_pkg(root, "myapp", "1.0.0");
        fs::write(root.join(BUN_LOCK), BUN_LOCK_V1).unwrap();
        let d = NpmScanner.detect(root, &items_of(root));
        assert_eq!(d.origins.len(), 1);
        let config = ScanConfig::new(false, false);
        let sites = NpmScanner.origin_to_sites(&d.origins[0], &config, FlagLog(false));
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, SiteKind::Lockfile);
        assert!(sites[0].as_path().ends_with(BUN_LOCK));
        assert_eq!(NpmScanner.site_to_packages(&sites[0]).len(), 9);
        assert!(NpmScanner.accepts(&root.join(BUN_LOCK)));
    }

    //--------------------------------------------------------------------------
    // node_modules

    fn write_pkg(dir: &Path, name: &str, version: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join(PACKAGE_JSON),
            format!(r#"{{"name": "{name}", "version": "{version}"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn test_walk_node_modules_npm_a() {
        let dir = tempdir().unwrap();
        let nm = dir.path().join(NODE_MODULES);
        write_pkg(&nm.join("lodash"), "lodash", "4.17.15");
        write_pkg(&nm.join("@babel/core"), "@babel/core", "7.24.0");
        write_pkg(&nm.join("debug"), "debug", "4.3.4");
        write_pkg(&nm.join("debug/node_modules/ms"), "ms", "2.1.2");
        write_pkg(&nm.join("ms"), "ms", "2.1.3");
        // not packages: .bin, a package.json-less dir, a nested non-root package.json
        fs::create_dir_all(nm.join(".bin")).unwrap();
        fs::create_dir_all(nm.join("broken")).unwrap();
        fs::create_dir_all(nm.join("lodash/lib")).unwrap();
        fs::write(nm.join("lodash/lib/package.json"), r#"{"type": "module"}"#).unwrap();
        let mut out = Vec::new();
        walk_node_modules(&nm, &mut out);
        assert_eq!(
            names(&out),
            vec![
                "@babel/core-7.24.0",
                "debug-4.3.4",
                "lodash-4.17.15",
                "ms-2.1.2",
                "ms-2.1.3"
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_walk_node_modules_pnpm_a() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let nm = dir.path().join(NODE_MODULES);
        let store = nm.join(".pnpm");
        // the store holds real package dirs; dependencies within it are symlinks
        write_pkg(
            &store.join("react@18.2.0/node_modules/react"),
            "react",
            "18.2.0",
        );
        write_pkg(
            &store.join("react-dom@18.2.0_react@18.2.0/node_modules/react-dom"),
            "react-dom",
            "18.2.0",
        );
        symlink(
            store.join("react@18.2.0/node_modules/react"),
            store.join("react-dom@18.2.0_react@18.2.0/node_modules/react"),
        )
        .unwrap();
        // top-level entries are symlinks into the store
        symlink(
            store.join("react@18.2.0/node_modules/react"),
            nm.join("react"),
        )
        .unwrap();
        // a workspace link
        let member = dir.path().join("packages/a");
        write_pkg(&member, "member-a", "0.0.1");
        symlink(&member, nm.join("member-a")).unwrap();

        let mut out = Vec::new();
        walk_node_modules(&nm, &mut out);
        // each store package once; links not followed
        assert_eq!(names(&out), vec!["react-18.2.0", "react-dom-18.2.0"]);
        assert_eq!(out.len(), 2);
    }

    //--------------------------------------------------------------------------
    // detection, origins, sites

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
    fn test_detect_project_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_pkg(root, "myapp", "1.0.0");
        // package.json alone is not an origin, but node_modules is still skipped
        let d = NpmScanner.detect(root, &items_of(root));
        assert!(d.origins.is_empty());
        assert!(d.skip_children.contains(&NODE_MODULES));
        assert!(d.skip_children.contains(&".next"));
        // with a lockfile
        fs::write(root.join(PACKAGE_LOCK), "{}").unwrap();
        let d = NpmScanner.detect(root, &items_of(root));
        assert_eq!(d.origins.len(), 1);
        assert!(!d.prune);
        // with node_modules only
        fs::remove_file(root.join(PACKAGE_LOCK)).unwrap();
        fs::create_dir(root.join(NODE_MODULES)).unwrap();
        let d = NpmScanner.detect(root, &items_of(root));
        assert_eq!(d.origins.len(), 1);
    }

    #[test]
    fn test_detect_install_dir_a() {
        // a node_modules without a sibling package.json is an install origin, and is never walked
        let dir = tempdir().unwrap();
        let nm = dir.path().join("lib").join(NODE_MODULES);
        write_pkg(&nm.join("npm"), "npm", "10.8.0");
        let d = NpmScanner.detect(&nm, &items_of(&nm));
        assert_eq!(d.origins.len(), 1);
        assert_eq!(d.origins[0].as_path(), nm);
        assert!(d.prune);
        // a project's node_modules (reached only via --root on it) is pruned but not claimed
        let proj = dir.path().join("proj");
        write_pkg(&proj, "proj", "1.0.0");
        fs::create_dir(proj.join(NODE_MODULES)).unwrap();
        let pnm = proj.join(NODE_MODULES);
        let d = NpmScanner.detect(&pnm, &items_of(&pnm));
        assert!(d.origins.is_empty());
        assert!(d.prune);
    }

    #[test]
    fn test_detect_editor_extensions_a() {
        let dir = tempdir().unwrap();
        let ext = dir.path().join(".vscode/extensions/ms-x.y-1.0.0");
        write_pkg(&ext, "y", "1.0.0");
        fs::create_dir(ext.join(NODE_MODULES)).unwrap();
        let d = NpmScanner.detect(&ext, &items_of(&ext));
        assert!(d.origins.is_empty());
        assert!(in_editor_extensions(&ext));
        assert!(in_editor_extensions(&dir.path().join(".cursor/extensions")));
        assert!(!in_editor_extensions(&dir.path().join(".vscode/settings")));
        assert!(!in_editor_extensions(&dir.path().join("src/extensions")));
    }

    #[test]
    fn test_origin_to_sites_precedence_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_pkg(root, "myapp", "1.0.0");
        fs::write(root.join(PACKAGE_LOCK), PACKAGE_LOCK_V3).unwrap();
        let o = origin(root.to_path_buf());
        let config = ScanConfig::new(false, false);
        // lockfile only
        let sites = NpmScanner.origin_to_sites(&o, &config, FlagLog(false));
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, SiteKind::Lockfile);
        assert_eq!(NpmScanner.site_to_packages(&sites[0]).len(), 8);
        // installed takes precedence over the lockfile
        write_pkg(&root.join("node_modules/lodash"), "lodash", "4.17.21");
        let sites = NpmScanner.origin_to_sites(&o, &config, FlagLog(false));
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, SiteKind::Installed);
        assert_eq!(
            names(&NpmScanner.site_to_packages(&sites[0])),
            vec!["lodash-4.17.21"]
        );
    }

    #[test]
    fn test_site_to_packages_dedup_a() {
        // the same package installed at several depths is one package for the site
        let dir = tempdir().unwrap();
        let nm = dir.path().join(NODE_MODULES);
        write_pkg(&nm.join("ms"), "ms", "2.1.3");
        write_pkg(&nm.join("a/node_modules/ms"), "ms", "2.1.3");
        write_pkg(&nm.join("a"), "a", "1.0.0");
        let site = Site::new(Ecosystem::Npm, SiteKind::Installed, PathShared::from(nm));
        assert_eq!(NpmScanner.site_to_packages(&site).len(), 2);
    }

    #[test]
    fn test_normalize_origin_a() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_pkg(root, "myapp", "1.0.0");
        fs::write(root.join(PNPM_LOCK), PNPM_V9).unwrap();
        let member = root.join("packages/a");
        write_pkg(&member, "member-a", "0.0.1");
        for p in [
            root.join(PACKAGE_JSON),
            root.join(PNPM_LOCK),
            root.to_path_buf(),
            member.clone(),
            member.join(PACKAGE_JSON),
        ] {
            assert!(NpmScanner.accepts(&p), "{p:?}");
            assert_eq!(
                NpmScanner.normalize_origin(&p).unwrap().as_path(),
                root,
                "{p:?}"
            );
        }
        assert!(!NpmScanner.accepts(&root.join("packages")));
    }

    #[test]
    fn test_normalize_origin_errors_a() {
        let dir = tempdir().unwrap();
        write_pkg(dir.path(), "x", "1.0.0");
        assert!(NpmScanner.accepts(dir.path()));
        let err = NpmScanner
            .normalize_origin(dir.path())
            .unwrap_err()
            .to_string();
        assert!(err.contains("npm install"), "{err}");
    }

    #[test]
    fn test_resolved_to_provenance_a() {
        assert_eq!(resolved_to_provenance(None), Some(None));
        assert_eq!(
            resolved_to_provenance(Some("https://registry.npmjs.org/a/-/a-1.0.0.tgz")),
            Some(None)
        );
        assert_eq!(resolved_to_provenance(Some("file:../x")), None);
        assert_eq!(resolved_to_provenance(Some("link:../x")), None);
        let d = resolved_to_provenance(Some("git+https://github.com/o/r.git#abc"))
            .unwrap()
            .unwrap();
        assert_eq!(d.url, "https://github.com/o/r.git");
        assert_eq!(d.vcs_info.unwrap().commit_id, "abc");
        // a tarball URL fragment is not a commit
        let d = resolved_to_provenance(Some("https://npm.example.com/a.tgz#sha1"))
            .unwrap()
            .unwrap();
        assert!(d.vcs_info.is_none());
    }
}
