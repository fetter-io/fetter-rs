//! pre-commit scanner.
//!
//! An origin is a directory with a `.pre-commit-config.yaml`; its site is that file (`SiteKind::Manifest`). Each remote hook repository (`repo:` with a `rev:`) is a package named by its URL without scheme or `.git` (`github.com/psf/black`), with the rev as its version. `local` and `meta` repositories run no third-party code from a repository and are excluded. A SHA rev with a full version comment (`rev: <sha>  # frozen: v1.2.3`, as written by `pre-commit autoupdate --freeze`) takes the comment's version, with the SHA kept in `direct_url`, as for GitHub Actions pins.
//!
//! Hooks' pinned `additional_dependencies` (in any repository, `local` included) are packages of their own ecosystems (Python, npm, Go, Ruby): the config is also a `Manifest` site of each such ecosystem, read by `dependency_site_packages` (see `pinned_dependency`).
//!
//! OSV has no pre-commit ecosystem. When auditing, a hook repository on GitHub is mapped to the packages it installs (`map_hooks`): its `.pre-commit-hooks.yaml` at the rev gives the hooks' languages, and the language's manifest gives the package (`black` on PyPI at the rev) or, for wrapper repositories (`ruff-pre-commit`, `mirrors-mypy`; a placeholder version `0.0.0`), the tools they pin (`ruff==0.1.13`). Those packages are audited in their own ecosystems.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use rayon::prelude::*;
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::dep_spec::DepSpec;
use crate::eco::Detect;
use crate::eco::DirItem;
use crate::eco::EcosystemScanner;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::origin_site::SiteKind;
use crate::osv_match::is_sha;
use crate::osv_match::RefKind;
use crate::package::Package;
use crate::package_durl::DirectURL;
use crate::package_durl::VcsInfo;
use crate::path_shared::PathShared;
use crate::ureq_client::UreqClient;
use crate::util::hash_string;
use crate::util::logger;
use crate::util::path_normalize;
use crate::util::path_within_duration;
use crate::util::CacheConfig;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::ScanConfig;
use crate::util::DURATION_0;
use crate::version_spec::VersionSpec;

const CONFIG: &str = ".pre-commit-config.yaml";
const HOOKS_FILE: &str = ".pre-commit-hooks.yaml";
const GITHUB: &str = "github.com/";

//------------------------------------------------------------------------------
// config parsing

/// A repository URL as a package name: without scheme, credentials, `.git` or a trailing `/`; `git@host:path` is `host/path`.
pub(crate) fn repo_name(url: &str) -> Option<String> {
    let u = url.trim();
    let rest = if let Some(r) = u.split_once("://").map(|(_, r)| r) {
        r.to_string()
    } else {
        u.strip_prefix("git@")?.replacen(':', "/", 1)
    };
    let rest = rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(&rest);
    let name = rest.trim_end_matches('/').trim_end_matches(".git");
    (name.contains('/') && !name.is_empty()).then(|| name.to_string())
}

/// The URL to fetch from for a package name (`https://<name>`).
pub(crate) fn repo_url(name: &str) -> String {
    format!("https://{name}")
}

/// The comment of a `rev:` line, if any (`rev: <sha>  # frozen: v1.2.3` gives `v1.2.3`). YAML parsing drops comments, so they are read from the raw text.
fn rev_comments(content: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in content.lines() {
        let t = line.trim_start().trim_start_matches("- ").trim_start();
        let Some(rest) = t.strip_prefix("rev:") else {
            continue;
        };
        let Some((value, comment)) = rest.split_once(" #") else {
            continue;
        };
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        let comment = comment.trim();
        let comment = comment.strip_prefix("frozen:").unwrap_or(comment).trim();
        if let Some(word) = comment.split_whitespace().next() {
            out.entry(value.to_string()).or_insert(word.to_string());
        }
    }
    out
}

/// The hook repositories of a `.pre-commit-config.yaml`.
fn packages_from_config(content: &str) -> Vec<Package> {
    let Ok(doc) = serde_saphyr::from_str::<JsonValue>(content) else {
        return Vec::new();
    };
    let comments = rev_comments(content);
    let mut out = HashSet::new();
    for r in doc
        .get("repos")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(url), Some(rev)) = (
            r.get("repo").and_then(JsonValue::as_str),
            r.get("rev").map(|v| match v {
                JsonValue::String(s) => s.clone(),
                other => other.to_string(),
            }),
        ) else {
            continue;
        };
        let Some(name) = repo_name(url) else {
            // `local`, `meta`, or a local path
            continue;
        };
        let rev = rev.trim().to_string();
        if rev.is_empty() {
            continue;
        }
        let package = if is_sha(&rev) {
            let hint = comments
                .get(&rev)
                .filter(|c| matches!(RefKind::classify(c), RefKind::Exact(_)))
                .cloned();
            let direct_url = DirectURL {
                url: repo_url(&name),
                vcs_info: Some(VcsInfo {
                    commit_id: rev.clone(),
                    vcs: "git".to_string(),
                    revision: hint.clone(),
                }),
            };
            let version = hint.unwrap_or(rev);
            Package::new(
                Ecosystem::PreCommit,
                &name,
                VersionSpec::new(&version),
                Some(direct_url),
            )
        } else {
            Package::new(Ecosystem::PreCommit, &name, VersionSpec::new(&rev), None)
        };
        out.insert(package);
    }
    out.into_iter().collect()
}

/// The ecosystems whose packages a hook's `additional_dependencies` can name.
const DEPENDENCY_ECOSYSTEMS: [Ecosystem; 4] = [
    Ecosystem::Python,
    Ecosystem::Npm,
    Ecosystem::Go,
    Ecosystem::Ruby,
];

/// A Go `go install` target (`github.com/owner/repo/cmd/x@v1.2.3`) as a module package. On GitHub, GitLab and Bitbucket the module is taken as the first three path segments (and a following major-version segment, `/v2`), the repository root; elsewhere the path is kept as written.
fn pinned_go(spec: &str) -> Option<Package> {
    let (path, version) = spec.rsplit_once('@')?;
    crate::eco::go::semver_of(version)?;
    let segs: Vec<&str> = path.split('/').collect();
    let module = if matches!(segs[0], "github.com" | "gitlab.com" | "bitbucket.org")
        && segs.len() >= 3
    {
        let mut n = 3;
        if segs.get(3).is_some_and(|s| {
            s.strip_prefix('v')
                .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
        }) {
            n = 4;
        }
        segs[..n].join("/")
    } else {
        path.to_string()
    };
    Some(Package::new(
        Ecosystem::Go,
        &module,
        VersionSpec::new(version),
        None,
    ))
}

/// A RubyGems `name:version` dependency (as `gem install` takes it) pinned to one version.
fn pinned_gem(spec: &str) -> Option<Package> {
    let (name, version) = spec.split_once(':')?;
    let ok = !name.is_empty()
        && version.starts_with(|c: char| c.is_ascii_digit())
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.');
    ok.then(|| Package::new(Ecosystem::Ruby, name, VersionSpec::new(version), None))
}

/// The package an `additional_dependencies` entry pins, given the hook's language if the config states it (always for `local` repositories). Without a language, the form decides: a PEP 508 `==` is Python; `name@version` is npm, or Go if the name is a module path (a first segment with a `.`). Unpinned entries (`types-requests`, `foo>=1`) are not reported, as the version installed is not known; nor are Rust dependencies, whose versions are requirements.
fn pinned_dependency(language: Option<&str>, spec: &str) -> Option<Package> {
    let spec = spec.trim();
    let go_like = |s: &str| {
        !s.starts_with('@')
            && s.split('/').next().is_some_and(|h| h.contains('.'))
            && s.contains('/')
    };
    match language {
        Some("python") => pinned_python(spec),
        Some("node") => pinned_npm_spec(spec),
        Some("golang") => pinned_go(spec),
        Some("ruby") => pinned_gem(spec),
        Some(_) => None,
        None if spec.contains("==") => pinned_python(spec),
        None if spec.contains(char::is_whitespace) => None,
        None if go_like(spec) => pinned_go(spec),
        None => pinned_npm_spec(spec),
    }
}

/// The pinned `additional_dependencies` of a `.pre-commit-config.yaml`'s hooks (in every repository, `local` included: they are installed into the hook's environment), per `pinned_dependency`.
fn dependencies_from_config(content: &str) -> Vec<Package> {
    let Ok(doc) = serde_saphyr::from_str::<JsonValue>(content) else {
        return Vec::new();
    };
    let mut out = HashSet::new();
    for h in doc
        .get("repos")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| r.get("hooks").and_then(JsonValue::as_array))
        .flatten()
    {
        let language = h.get("language").and_then(JsonValue::as_str);
        for d in h
            .get("additional_dependencies")
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
            .filter_map(JsonValue::as_str)
        {
            out.extend(pinned_dependency(language, d));
        }
    }
    let mut out: Vec<Package> = out.into_iter().collect();
    out.sort();
    out
}

/// True if a site is a `.pre-commit-config.yaml` read for another ecosystem's packages (its hooks' `additional_dependencies`).
pub(crate) fn is_dependency_site(site: &Site) -> bool {
    site.ecosystem != Ecosystem::PreCommit
        && site.kind == SiteKind::Manifest
        && site.as_path().file_name().is_some_and(|n| n == CONFIG)
}

/// The packages of a dependency site (`is_dependency_site`): the config's pinned `additional_dependencies` in the site's ecosystem.
pub(crate) fn dependency_site_packages(site: &Site) -> Vec<Package> {
    fs::read_to_string(site.as_path())
        .map(|c| dependencies_from_config(&c))
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.ecosystem == site.ecosystem)
        .collect()
}

//------------------------------------------------------------------------------
// mapping hooks to the packages they install

/// A hook repository and the packages it installs, audited in their own ecosystems.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MappedHook {
    pub hook: Package,
    pub packages: Vec<Package>,
}

/// The outcome of mapping hook repositories: the packages with mapped hooks replaced by what they install.
#[derive(Debug, Default)]
pub(crate) struct HookMapping {
    pub(crate) packages: Vec<Package>,
    pub(crate) mapped: Vec<MappedHook>,
}

/// Fetch a file of a GitHub repository at a rev from raw.githubusercontent.com, using a time-limited cache. `Ok(None)` if the file does not exist.
fn fetch_file(
    client: &Arc<dyn UreqClient>,
    owner_repo: &str,
    rev: &str,
    file: &str,
    cache_config: &CacheConfig,
    log: FlagLog,
) -> ResultDynError<Option<String>> {
    let url = format!("https://raw.githubusercontent.com/{owner_repo}/{rev}/{file}");
    let cache_fp = cache_config
        .directory
        .join(format!("precommit_file_{}", hash_string(&url)))
        .with_extension("txt");
    if cache_config.duration != DURATION_0
        && path_within_duration(&cache_fp, cache_config.duration)
    {
        if let Ok(c) = fs::read_to_string(&cache_fp) {
            logger!(log, module_path!(), "Loaded file cache: {:?}", cache_fp);
            return Ok(Some(c));
        }
    }
    match client.get(&url) {
        Ok(body) => {
            if cache_config.duration != DURATION_0 {
                let _ = fs::write(&cache_fp, &body);
            }
            Ok(Some(body))
        }
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(e) => Err(format!("Cannot fetch {url}: {e}").into()),
    }
}

/// The hooks' languages, and their `additional_dependencies` by language, from a `.pre-commit-hooks.yaml`.
fn hook_languages(content: &str) -> (Vec<String>, Vec<(String, String)>) {
    let Ok(doc) = serde_saphyr::from_str::<JsonValue>(content) else {
        return (Vec::new(), Vec::new());
    };
    let mut langs = Vec::new();
    let mut deps = Vec::new();
    for h in doc.as_array().into_iter().flatten() {
        let Some(lang) = h.get("language").and_then(JsonValue::as_str) else {
            continue;
        };
        langs.push(lang.to_string());
        for d in h
            .get("additional_dependencies")
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
            .filter_map(JsonValue::as_str)
        {
            deps.push((lang.to_string(), d.to_string()));
        }
    }
    langs.sort();
    langs.dedup();
    deps.sort();
    deps.dedup();
    (langs, deps)
}

/// The version of a repository's own package at a rev: the manifest's static version, unless absent; else the rev without its `v`. `None` for a placeholder (`0.0.0`), whose repository only wraps what it pins.
fn own_version(static_version: Option<&str>, rev: &str) -> Option<String> {
    match static_version {
        Some("0.0.0") => None,
        Some(v) if !v.is_empty() => Some(v.to_string()),
        _ => Some(rev.strip_prefix('v').unwrap_or(rev).to_string()),
    }
}

/// A Python requirement pinned to one version (`ruff==0.1.13`) as a PyPI package.
fn pinned_python(spec: &str) -> Option<Package> {
    let ds = DepSpec::from_string(spec).ok()?;
    let v = ds.get_exact()?;
    Some(Package::new(Ecosystem::Python, &ds.name, v, None))
}

/// An npm dependency pinned to one version (`prettier@3.1.0`, or a name and an exact version) as an npm package.
fn pinned_npm(name: &str, version: &str) -> Option<Package> {
    let v = version.trim().trim_start_matches('=');
    nodejs_semver::Version::parse(v)
        .ok()
        .map(|_| Package::new(Ecosystem::Npm, name, VersionSpec::new(v), None))
}

fn pinned_npm_spec(spec: &str) -> Option<Package> {
    // a scoped name starts with `@`
    let at = spec[1..].find('@')? + 1;
    pinned_npm(&spec[..at], &spec[at + 1..])
}

/// Python packages a hook repository installs: its own package (`pyproject.toml`, else `setup.cfg`, else `setup.py`), unless a placeholder, and its pinned dependencies.
fn python_packages(
    fetch: &dyn Fn(&str) -> ResultDynError<Option<String>>,
    rev: &str,
) -> ResultDynError<Vec<Package>> {
    let mut out = Vec::new();
    if let Some(c) = fetch("pyproject.toml")? {
        if let Ok(t) = toml::from_str::<toml::Value>(&c) {
            if let Some(project) = t.get("project") {
                if let Some(name) = project.get("name").and_then(|n| n.as_str()) {
                    let static_v = project.get("version").and_then(|v| v.as_str());
                    if let Some(v) = own_version(static_v, rev) {
                        out.push(Package::new(
                            Ecosystem::Python,
                            name,
                            VersionSpec::new(&v),
                            None,
                        ));
                    }
                    out.extend(
                        project
                            .get("dependencies")
                            .and_then(|d| d.as_array())
                            .into_iter()
                            .flatten()
                            .filter_map(|d| d.as_str())
                            .filter_map(pinned_python),
                    );
                    return Ok(out);
                }
            }
            // Poetry (before PEP 621 support): `[tool.poetry]`, where a bare version is exact
            if let Some(poetry) = t.get("tool").and_then(|t| t.get("poetry")) {
                if let Some(name) = poetry.get("name").and_then(|n| n.as_str()) {
                    let static_v = poetry.get("version").and_then(|v| v.as_str());
                    if let Some(v) = own_version(static_v, rev) {
                        out.push(Package::new(
                            Ecosystem::Python,
                            name,
                            VersionSpec::new(&v),
                            None,
                        ));
                    }
                    for (dep, spec) in poetry
                        .get("dependencies")
                        .and_then(|d| d.as_table())
                        .into_iter()
                        .flatten()
                    {
                        // optional dependencies are installed only with extras, which pre-commit does not request
                        if spec.get("optional").and_then(|o| o.as_bool()) == Some(true) {
                            continue;
                        }
                        let spec = spec
                            .as_str()
                            .or_else(|| spec.get("version").and_then(|v| v.as_str()));
                        if let Some(v) = spec.map(str::trim) {
                            let exact = v.strip_prefix("==").unwrap_or(v);
                            if dep != "python"
                                && !exact.is_empty()
                                && exact
                                    .chars()
                                    .all(|c| c.is_ascii_alphanumeric() || c == '.')
                            {
                                out.extend(pinned_python(&format!("{dep}=={exact}")));
                            }
                        }
                    }
                    return Ok(out);
                }
            }
        }
    }
    if let Some(c) = fetch("setup.cfg")? {
        let mut section = String::new();
        let mut name = None;
        let mut version = None;
        let mut in_requires = false;
        for line in c.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                section = t.to_string();
                in_requires = false;
                continue;
            }
            let continuation = line.starts_with([' ', '\t']) && !t.is_empty();
            if in_requires && continuation {
                out.extend(pinned_python(t));
                continue;
            }
            in_requires = false;
            if let Some((k, v)) = t.split_once('=') {
                let (k, v) = (k.trim(), v.trim());
                match (section.as_str(), k) {
                    ("[metadata]", "name") => name = Some(v.to_string()),
                    ("[metadata]", "version")
                        if !v.starts_with("attr:") && !v.starts_with("file:") =>
                    {
                        version = Some(v.to_string())
                    }
                    ("[options]", "install_requires") => {
                        in_requires = true;
                        out.extend(pinned_python(v));
                    }
                    _ => {}
                }
            }
        }
        if let Some(name) = name {
            if let Some(v) = own_version(version.as_deref(), rev) {
                out.insert(
                    0,
                    Package::new(Ecosystem::Python, &name, VersionSpec::new(&v), None),
                );
            }
            return Ok(out);
        }
    }
    if let Some(c) = fetch("setup.py")? {
        // literal arguments only: `name='x'`, `version='1.0'`, `install_requires=['a==1']`
        let literal = |key: &str| -> Option<String> {
            let i = c.find(&format!("{key}="))? + key.len() + 1;
            let rest = c[i..].trim_start();
            let q = rest.chars().next().filter(|q| *q == '\'' || *q == '"')?;
            let end = rest[1..].find(q)?;
            Some(rest[1..1 + end].to_string())
        };
        if let Some(name) = literal("name") {
            if let Some(v) = own_version(literal("version").as_deref(), rev) {
                out.push(Package::new(
                    Ecosystem::Python,
                    &name,
                    VersionSpec::new(&v),
                    None,
                ));
            }
            if let Some(i) = c.find("install_requires=") {
                if let Some(end) = c[i..].find(']') {
                    for part in c[i..i + end].split([',', '[']) {
                        let s = part.trim().trim_matches(|ch| ch == '\'' || ch == '"');
                        out.extend(pinned_python(s));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// npm packages a hook repository installs: its own package (`package.json`), unless a placeholder, and its exact dependencies.
fn node_packages(
    fetch: &dyn Fn(&str) -> ResultDynError<Option<String>>,
) -> ResultDynError<Vec<Package>> {
    let mut out = Vec::new();
    let Some(c) = fetch("package.json")? else {
        return Ok(out);
    };
    let Ok(pj) = serde_json::from_str::<JsonValue>(&c) else {
        return Ok(out);
    };
    if let (Some(name), Some(v)) = (
        pj.get("name").and_then(JsonValue::as_str),
        pj.get("version").and_then(JsonValue::as_str),
    ) {
        if v != "0.0.0" {
            out.extend(pinned_npm(name, v));
        }
    }
    for (name, v) in pj
        .get("dependencies")
        .and_then(JsonValue::as_object)
        .into_iter()
        .flatten()
    {
        if let Some(v) = v.as_str() {
            out.extend(pinned_npm(name, v));
        }
    }
    Ok(out)
}

/// The Go module a hook repository installs (`go install` of the repository at the rev, a module version), from `go.mod`.
fn go_packages(
    fetch: &dyn Fn(&str) -> ResultDynError<Option<String>>,
    rev: &str,
) -> ResultDynError<Vec<Package>> {
    let Some(c) = fetch("go.mod")? else {
        return Ok(Vec::new());
    };
    let module = c
        .lines()
        .find_map(|l| l.trim().strip_prefix("module "))
        .map(|m| m.trim().trim_matches('"').to_string());
    Ok(match module {
        Some(m) if crate::eco::go::semver_of(rev).is_some() => {
            vec![Package::new(Ecosystem::Go, &m, VersionSpec::new(rev), None)]
        }
        _ => Vec::new(),
    })
}

/// The crate a hook repository installs (`cargo install` of the repository), from `Cargo.toml`.
fn rust_packages(
    fetch: &dyn Fn(&str) -> ResultDynError<Option<String>>,
    rev: &str,
) -> ResultDynError<Vec<Package>> {
    let Some(c) = fetch("Cargo.toml")? else {
        return Ok(Vec::new());
    };
    let Ok(t) = toml::from_str::<toml::Value>(&c) else {
        return Ok(Vec::new());
    };
    let Some(name) = t
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
    else {
        return Ok(Vec::new());
    };
    let static_v = t
        .get("package")
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str());
    Ok(own_version(static_v, rev)
        .map(|v| Package::new(Ecosystem::Cargo, name, VersionSpec::new(&v), None))
        .into_iter()
        .collect())
}

/// The packages a hook repository installs, for each of its hooks' languages that has a registry (python, node, golang, rust), plus the hooks' pinned `additional_dependencies`. Other languages (docker, system, script, ...) install nothing auditable. Empty if the repository has no `.pre-commit-hooks.yaml`.
fn hook_packages(
    client: &Arc<dyn UreqClient>,
    hook: &Package,
    cache_config: &CacheConfig,
    log: FlagLog,
) -> ResultDynError<Vec<Package>> {
    let Some(owner_repo) = hook.name.strip_prefix(GITHUB) else {
        return Ok(Vec::new());
    };
    let version = hook.version.to_string();
    // fetch at the pinned commit if there is one
    let fetch_rev = hook
        .direct_url
        .as_ref()
        .and_then(|d| d.vcs_info.as_ref())
        .map(|v| v.commit_id.clone())
        .unwrap_or_else(|| version.clone());
    let fetch =
        |file: &str| fetch_file(client, owner_repo, &fetch_rev, file, cache_config, log);
    let Some(hooks_yaml) = fetch(HOOKS_FILE)? else {
        return Ok(Vec::new());
    };
    let (langs, extra) = hook_languages(&hooks_yaml);
    let mut out = Vec::new();
    for lang in &langs {
        match lang.as_str() {
            "python" => out.extend(python_packages(&fetch, &version)?),
            "node" => out.extend(node_packages(&fetch)?),
            "golang" => out.extend(go_packages(&fetch, &version)?),
            "rust" => out.extend(rust_packages(&fetch, &version)?),
            _ => {}
        }
    }
    for (lang, spec) in &extra {
        match lang.as_str() {
            "python" => out.extend(pinned_python(spec)),
            "node" => out.extend(pinned_npm_spec(spec)),
            _ => {}
        }
    }
    let unique: HashSet<Package> = out.into_iter().collect();
    let mut out: Vec<Package> = unique.into_iter().collect();
    out.sort();
    Ok(out)
}

/// Replace each pre-commit hook repository on GitHub with the packages it installs (see `hook_packages`), so they can be audited in their ecosystems; each repository and rev is fetched once, in parallel. A hook that installs nothing auditable (or cannot be fetched; logged) is kept, and remains unaudited.
pub(crate) fn map_hooks(
    client: &Arc<dyn UreqClient>,
    packages: &[Package],
    cache_config: &CacheConfig,
    log: FlagLog,
) -> HookMapping {
    let mut hooks: Vec<&Package> = packages
        .iter()
        .filter(|p| p.ecosystem == Ecosystem::PreCommit)
        .collect();
    hooks.sort();
    hooks.dedup();
    let found: HashMap<Package, Vec<Package>> = hooks
        .into_par_iter()
        .filter_map(|h| match hook_packages(client, h, cache_config, log) {
            Ok(p) if !p.is_empty() => Some((h.clone(), p)),
            Ok(_) => None,
            Err(e) => {
                logger!(
                    log,
                    module_path!(),
                    "Cannot map pre-commit hook {}: {}",
                    h,
                    e
                );
                None
            }
        })
        .collect();
    let mut mapping = HookMapping::default();
    let mut seen = HashSet::new();
    for p in packages {
        match found.get(p) {
            Some(installed) => {
                if seen.insert(p.clone()) {
                    mapping.mapped.push(MappedHook {
                        hook: p.clone(),
                        packages: installed.clone(),
                    });
                }
                mapping.packages.extend(installed.iter().cloned());
            }
            None => mapping.packages.push(p.clone()),
        }
    }
    // a package installed by several hooks, or also found elsewhere, is audited once
    let mut unique = HashSet::new();
    mapping.packages.retain(|p| unique.insert(p.clone()));
    mapping.mapped.sort_by(|a, b| a.hook.cmp(&b.hook));
    mapping
}

//------------------------------------------------------------------------------
fn origin(dir: PathBuf) -> Origin {
    Origin::new(Ecosystem::PreCommit, PathShared::from(dir))
}

pub(crate) struct PreCommitScanner;

impl EcosystemScanner for PreCommitScanner {
    fn primary_site_kinds(&self) -> &'static [SiteKind] {
        &[SiteKind::Manifest]
    }

    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect {
        Detect {
            origins: if items.iter().any(|i| i.file_name() == Some(CONFIG)) {
                vec![origin(dir.to_path_buf())]
            } else {
                Vec::new()
            },
            ..Default::default()
        }
    }

    fn accepts(&self, path: &Path) -> bool {
        let Ok(p) = path_normalize(path, false) else {
            return false;
        };
        if p.is_file() {
            return p.file_name().is_some_and(|n| n == CONFIG);
        }
        p.is_dir() && p.join(CONFIG).is_file()
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
        if dir.join(CONFIG).is_file() {
            Ok(origin(dir))
        } else {
            Err(format!("No {CONFIG} found in {dir:?}").into())
        }
    }

    /// The config file, as a pre-commit site for the hook repositories, and as a site of each enabled ecosystem its hooks' pinned `additional_dependencies` name (as VS Code extensions' bundled `node_modules` are npm sites).
    fn origin_to_sites(
        &self,
        origin: &Origin,
        config: &ScanConfig,
        _log: FlagLog,
    ) -> Vec<Site> {
        let fp = origin.as_path().join(CONFIG);
        let Ok(content) = fs::read_to_string(&fp) else {
            return Vec::new();
        };
        let path = PathShared::from(fp);
        let mut sites = vec![Site::new(
            Ecosystem::PreCommit,
            SiteKind::Manifest,
            path.clone(),
        )];
        let deps = dependencies_from_config(&content);
        for eco in DEPENDENCY_ECOSYSTEMS {
            if config.ecosystems.contains(&eco) && deps.iter().any(|p| p.ecosystem == eco)
            {
                sites.push(Site::new(eco, SiteKind::Manifest, path.clone()));
            }
        }
        sites
    }

    fn site_to_packages(&self, site: &Site) -> Vec<Package> {
        fs::read_to_string(site.as_path())
            .map(|c| packages_from_config(&c))
            .unwrap_or_default()
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ureq_client::UreqClientMock;
    use crate::util::path_cache;
    use tempfile::tempdir;

    const CONFIG_A: &str = r#"repos:
-   repo: https://github.com/astral-sh/ruff-pre-commit
    rev: v0.1.13
    hooks:
    -   id: ruff
-   repo: https://github.com/psf/black.git
    rev: "24.1.0"
    hooks:
    -   id: black
-   repo: https://github.com/pre-commit/pre-commit-hooks
    rev: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  # frozen: v4.5.0
    hooks:
    -   id: trailing-whitespace
-   repo: git@gitlab.com:acme/hooks.git
    rev: v1.0
    hooks:
    -   id: thing
-   repo: local
    hooks:
    -   id: mypy
        name: mypy
        entry: mypy
        language: system
-   repo: meta
    hooks:
    -   id: check-hooks-apply
"#;

    fn names(packages: &[Package]) -> Vec<String> {
        let mut out: Vec<String> = packages.iter().map(|p| p.to_string()).collect();
        out.sort();
        out
    }

    #[test]
    fn test_repo_name_a() {
        assert_eq!(
            repo_name("https://github.com/psf/black.git").as_deref(),
            Some("github.com/psf/black")
        );
        assert_eq!(
            repo_name("git@gitlab.com:acme/hooks.git").as_deref(),
            Some("gitlab.com/acme/hooks")
        );
        assert_eq!(
            repo_name("https://user@bitbucket.org/a/b/").as_deref(),
            Some("bitbucket.org/a/b")
        );
        assert_eq!(repo_name("local"), None);
        assert_eq!(repo_name("meta"), None);
        assert_eq!(repo_name("../relative/repo"), None);
    }

    #[test]
    fn test_packages_from_config_a() {
        let packages = packages_from_config(CONFIG_A);
        assert_eq!(
            names(&packages),
            vec![
                "github.com/astral-sh/ruff-pre-commit-v0.1.13",
                "github.com/pre-commit/pre-commit-hooks-v4.5.0",
                "github.com/psf/black-24.1.0",
                "gitlab.com/acme/hooks-v1.0",
            ]
        );
        let frozen = packages
            .iter()
            .find(|p| p.name.ends_with("pre-commit-hooks"))
            .unwrap();
        let vcs = frozen
            .direct_url
            .as_ref()
            .unwrap()
            .vcs_info
            .as_ref()
            .unwrap();
        assert_eq!(vcs.commit_id, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(vcs.revision.as_deref(), Some("v4.5.0"));
        assert!(packages_from_config("not: [valid").is_empty());
    }

    fn mock(files: &[(&str, &str)]) -> Arc<dyn UreqClient> {
        let mut get = HashMap::new();
        for (k, v) in files {
            get.insert(
                format!("https://raw.githubusercontent.com/{k}"),
                v.to_string(),
            );
        }
        Arc::new(UreqClientMock {
            mock_post: None,
            mock_get: Some(get),
        })
    }

    fn hook(name: &str, version: &str) -> Package {
        Package::new(Ecosystem::PreCommit, name, VersionSpec::new(version), None)
    }

    #[test]
    fn test_map_hooks_a() {
        let client = mock(&[
            // a wrapper: its package is a placeholder; it pins the tool
            ("astral-sh/ruff-pre-commit/v0.1.13/.pre-commit-hooks.yaml", "- id: ruff\n  language: python\n"),
            ("astral-sh/ruff-pre-commit/v0.1.13/pyproject.toml", "[project]\nname = \"ruff-pre-commit\"\nversion = \"0.0.0\"\ndependencies = [\"ruff==0.1.13\"]\n"),
            // poetry
            ("PyCQA/isort/5.12.0/.pre-commit-hooks.yaml", "- id: isort\n  language: python\n"),
            ("PyCQA/isort/5.12.0/pyproject.toml", "[tool.poetry]\nname = \"isort\"\nversion = \"5.12.0\"\n\n[tool.poetry.dependencies]\npython = \"^3.8.0\"\ncolorama = {version = \"0.4.6\", optional = true}\nother = \"^1.0\"\npinned = \"2.0.0\"\n"),
            // a tool that is its own hook, with a dynamic version
            ("pylint-dev/pylint/v3.0.3/.pre-commit-hooks.yaml", "- id: pylint\n  language: python\n"),
            ("pylint-dev/pylint/v3.0.3/pyproject.toml", "[project]\nname = \"pylint\"\ndynamic = [\"version\"]\n"),
            // setup.cfg with a static version
            ("pre-commit/pre-commit-hooks/v6.0.0/.pre-commit-hooks.yaml", "- id: x\n  language: python\n- id: y\n  language: fail\n"),
            ("pre-commit/pre-commit-hooks/v6.0.0/setup.cfg", "[metadata]\nname = pre_commit_hooks\nversion = 6.0.0\n\n[options]\ninstall_requires =\n    ruamel.yaml>=0.15\n    tomli==2.0.1\n"),
            // setup.py mirror
            ("pre-commit/mirrors-mypy/v1.8.0/.pre-commit-hooks.yaml", "- id: mypy\n  language: python\n"),
            ("pre-commit/mirrors-mypy/v1.8.0/setup.py", "setup(\n    name='pre_commit_placeholder_package',\n    version='0.0.0',\n    install_requires=['mypy==1.8.0'],\n)\n"),
            // node mirror: the tool is an additional dependency of the hook
            ("pre-commit/mirrors-prettier/v3.1.0/.pre-commit-hooks.yaml", "- id: prettier\n  language: node\n  additional_dependencies: ['prettier@3.1.0']\n"),
            ("pre-commit/mirrors-prettier/v3.1.0/package.json", "{\"name\": \"placeholder_package\", \"version\": \"0.0.0\"}"),
            // go, with docker and system variants
            ("gitleaks/gitleaks/v8.18.2/.pre-commit-hooks.yaml", "- id: gitleaks\n  language: golang\n- id: gitleaks-docker\n  language: docker_image\n"),
            ("gitleaks/gitleaks/v8.18.2/go.mod", "module github.com/zricethezav/gitleaks/v8\n\ngo 1.20\n"),
            // docker only: nothing auditable
            ("koalaman/shellcheck-precommit/v0.11.0/.pre-commit-hooks.yaml", "- id: shellcheck\n  language: docker_image\n"),
        ]);
        let packages = vec![
            hook("github.com/astral-sh/ruff-pre-commit", "v0.1.13"),
            hook("github.com/pylint-dev/pylint", "v3.0.3"),
            hook("github.com/PyCQA/isort", "5.12.0"),
            hook("github.com/pre-commit/pre-commit-hooks", "v6.0.0"),
            hook("github.com/pre-commit/mirrors-mypy", "v1.8.0"),
            hook("github.com/pre-commit/mirrors-prettier", "v3.1.0"),
            hook("github.com/gitleaks/gitleaks", "v8.18.2"),
            hook("github.com/koalaman/shellcheck-precommit", "v0.11.0"),
            // not on GitHub; and a repository without a hooks file
            hook("gitlab.com/acme/hooks", "v1.0"),
            hook("github.com/acme/missing", "v1"),
            Package::new(Ecosystem::Npm, "lodash", VersionSpec::new("4.17.21"), None),
        ];
        let cc = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let m = map_hooks(&client, &packages, &cc, FlagLog(false));
        let by_hook: HashMap<String, Vec<String>> = m
            .mapped
            .iter()
            .map(|h| {
                (
                    h.hook.name.clone(),
                    h.packages
                        .iter()
                        .map(|p| format!("{} {p}", p.ecosystem))
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            by_hook["github.com/astral-sh/ruff-pre-commit"],
            vec!["python ruff-0.1.13"]
        );
        assert_eq!(
            by_hook["github.com/pylint-dev/pylint"],
            vec!["python pylint-3.0.3"]
        );
        // the optional colorama is not installed; a range (`^1.0`) is not a pin
        assert_eq!(
            by_hook["github.com/PyCQA/isort"],
            vec!["python isort-5.12.0", "python pinned-2.0.0"]
        );
        assert_eq!(
            by_hook["github.com/pre-commit/pre-commit-hooks"],
            vec!["python pre_commit_hooks-6.0.0", "python tomli-2.0.1"]
        );
        assert_eq!(
            by_hook["github.com/pre-commit/mirrors-mypy"],
            vec!["python mypy-1.8.0"]
        );
        assert_eq!(
            by_hook["github.com/pre-commit/mirrors-prettier"],
            vec!["npm prettier-3.1.0"]
        );
        assert_eq!(
            by_hook["github.com/gitleaks/gitleaks"],
            vec!["go github.com/zricethezav/gitleaks/v8-v8.18.2"]
        );
        assert_eq!(m.mapped.len(), 7);
        // unmapped hooks are kept (and stay unaudited); other packages pass through
        let kept: Vec<String> = m
            .packages
            .iter()
            .filter(|p| {
                p.ecosystem == Ecosystem::PreCommit
                    || p.ecosystem == Ecosystem::Npm && p.name == "lodash"
            })
            .map(|p| p.to_string())
            .collect();
        assert_eq!(
            kept,
            vec![
                "github.com/koalaman/shellcheck-precommit-v0.11.0",
                "gitlab.com/acme/hooks-v1.0",
                "github.com/acme/missing-v1",
                "lodash-4.17.21",
            ]
        );
    }

    #[test]
    fn test_map_hooks_pinned_commit_a() {
        // a frozen rev is fetched at its commit
        let sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let client = mock(&[
            (
                &format!("psf/black/{sha}/.pre-commit-hooks.yaml"),
                "- id: black\n  language: python\n",
            ),
            (
                &format!("psf/black/{sha}/pyproject.toml"),
                "[project]\nname = \"black\"\ndynamic = [\"version\"]\n",
            ),
        ]);
        let packages = packages_from_config(&format!(
            "repos:\n- repo: https://github.com/psf/black\n  rev: {sha}  # frozen: 24.1.0\n  hooks:\n  - id: black\n"
        ));
        let cc = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let m = map_hooks(&client, &packages, &cc, FlagLog(false));
        assert_eq!(names(&m.packages), vec!["black-24.1.0"]);
        assert_eq!(m.packages[0].ecosystem, Ecosystem::Python);
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

    const CONFIG_DEPS: &str = r#"repos:
-   repo: https://github.com/pre-commit/mirrors-mypy
    rev: v1.8.0
    hooks:
    -   id: mypy
        additional_dependencies: [types-requests, "pydantic==2.5.3", "attrs>=23"]
-   repo: https://github.com/pre-commit/mirrors-eslint
    rev: v8.56.0
    hooks:
    -   id: eslint
        additional_dependencies:
        -   eslint@8.56.0
        -   "@typescript-eslint/parser@6.19.0"
        -   eslint-plugin-react@^7
-   repo: local
    hooks:
    -   id: golangci
        name: golangci
        entry: golangci-lint run
        language: golang
        additional_dependencies: [github.com/golangci/golangci-lint/cmd/golangci-lint@v1.55.2]
    -   id: rubocop
        name: rubocop
        entry: rubocop
        language: ruby
        additional_dependencies: ["rubocop:1.59.0", rubocop-rails]
    -   id: gofumpt
        name: gofumpt
        entry: gofumpt
        language: golang
        additional_dependencies: [mvdan.cc/gofumpt@v0.5.0, github.com/foo/bar/v2/cmd/x@v2.1.0]
    -   id: clippy
        name: clippy
        entry: cargo clippy
        language: rust
        additional_dependencies: ["cli:cargo-deny:0.14.3"]
"#;

    #[test]
    fn test_dependencies_from_config_a() {
        let deps = dependencies_from_config(CONFIG_DEPS);
        let got: Vec<String> = deps
            .iter()
            .map(|p| format!("{}:{}", p.ecosystem, p))
            .collect();
        assert_eq!(
            got,
            [
                "python:pydantic-2.5.3",
                "npm:@typescript-eslint/parser-6.19.0",
                "npm:eslint-8.56.0",
                "go:github.com/foo/bar/v2-v2.1.0",
                "go:github.com/golangci/golangci-lint-v1.55.2",
                "go:mvdan.cc/gofumpt-v0.5.0",
                "ruby:rubocop-1.59.0",
            ]
        );
        // the hook repositories themselves are unchanged
        assert_eq!(packages_from_config(CONFIG_DEPS).len(), 2);
    }

    #[test]
    fn test_pinned_dependency_a() {
        assert_eq!(
            pinned_dependency(None, "black==24.1.0").unwrap().ecosystem,
            Ecosystem::Python
        );
        assert!(pinned_dependency(None, "black").is_none());
        assert!(pinned_dependency(None, "foo @ https://x.org/foo.whl").is_none());
        assert_eq!(
            pinned_dependency(None, "prettier@3.1.0").unwrap().ecosystem,
            Ecosystem::Npm
        );
        assert!(pinned_dependency(None, "prettier@^3").is_none());
        assert_eq!(
            pinned_dependency(None, "github.com/a/b@v1.0.0")
                .unwrap()
                .ecosystem,
            Ecosystem::Go
        );
        assert!(pinned_dependency(None, "github.com/a/b@latest").is_none());
        assert!(pinned_dependency(Some("ruby"), "rubocop").is_none());
        assert!(pinned_dependency(Some("ruby"), "rubocop:~> 1.0").is_none());
        assert!(pinned_dependency(Some("rust"), "serde:1.0.0").is_none());
        assert!(pinned_dependency(Some("system"), "black==24.1.0").is_none());
        // an explicit language wins over the form
        assert!(pinned_dependency(Some("node"), "black==24.1.0").is_none());
    }

    #[test]
    fn test_dependency_sites_a() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(CONFIG), CONFIG_DEPS).unwrap();
        let o = PreCommitScanner.normalize_origin(dir.path()).unwrap();
        let sites = PreCommitScanner.origin_to_sites(
            &o,
            &ScanConfig::new(false, false),
            FlagLog(false),
        );
        let ecos: Vec<Ecosystem> = sites.iter().map(|s| s.ecosystem).collect();
        assert_eq!(
            ecos,
            [
                Ecosystem::PreCommit,
                Ecosystem::Python,
                Ecosystem::Npm,
                Ecosystem::Go,
                Ecosystem::Ruby
            ]
        );
        assert!(!is_dependency_site(&sites[0]));
        assert!(sites[1..].iter().all(is_dependency_site));
        assert_eq!(dependency_site_packages(&sites[2]).len(), 2);
        assert!(dependency_site_packages(&sites[2])
            .iter()
            .all(|p| p.ecosystem == Ecosystem::Npm));
        // only enabled ecosystems get a site
        let mut config = ScanConfig::new(false, false);
        config.ecosystems = vec![Ecosystem::PreCommit, Ecosystem::Npm];
        let sites = PreCommitScanner.origin_to_sites(&o, &config, FlagLog(false));
        assert_eq!(sites.len(), 2);
        // a config without additional dependencies has only its own site
        fs::write(dir.path().join(CONFIG), CONFIG_A).unwrap();
        let sites = PreCommitScanner.origin_to_sites(
            &o,
            &ScanConfig::new(false, false),
            FlagLog(false),
        );
        assert_eq!(sites.len(), 1);
    }

    #[test]
    fn test_scanner_a() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(CONFIG), CONFIG_A).unwrap();
        let d = PreCommitScanner.detect(dir.path(), &items_of(dir.path()));
        assert_eq!(d.origins.len(), 1);
        assert!(PreCommitScanner.accepts(&dir.path().join(CONFIG)));
        let o = PreCommitScanner.normalize_origin(dir.path()).unwrap();
        let sites = PreCommitScanner.origin_to_sites(
            &o,
            &ScanConfig::new(false, false),
            FlagLog(false),
        );
        assert_eq!(sites[0].kind, SiteKind::Manifest);
        assert_eq!(PreCommitScanner.site_to_packages(&sites[0]).len(), 4);
        let empty = tempdir().unwrap();
        assert!(PreCommitScanner.normalize_origin(empty.path()).is_err());
    }
}
