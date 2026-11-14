//! Package registries for non-Python ecosystems: list the published versions of a package (for `lookup-name` / `lookup-bound`). Python uses PyPI via `pypi_project.rs`.
//!
//! - npm: `https://registry.npmjs.org/<name>` with the abbreviated metadata format.
//! - Cargo: the crates.io sparse index (`https://index.crates.io/…`), the endpoint cargo itself uses; yanked versions are excluded. (The crates.io web API requires a User-Agent and is rate limited.)
//! - GitHub Actions: tags from `git ls-remote --tags --refs https://github.com/<owner>/<repo>` (no token or API rate limit; requires `git`).
//! - VS Code: Open VSX (`https://open-vsx.org/api/<publisher>/<name>`).
//!
//! Each registry also gives the package's canonical name, which matters where OSV names are case-sensitive (VS Code).

use std::fs;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::ecosystem::Ecosystem;
use crate::osv_match::SemverLike;
use crate::ureq_client::UreqClient;
use crate::util::hash_string;
use crate::util::logger;
use crate::util::path_within_duration;
use crate::util::CacheConfig;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::DURATION_0;

const NPM_ACCEPT: &str = "application/vnd.npm.install-v1+json";

/// A registry reported that a package does not exist (e.g. HTTP 404, or no such GitHub repository). Distinct from other failures (network, unparseable responses): a requirement for a package that is not found (e.g. an internal package on a private index) is reported as unresolved, while other failures are errors.
#[derive(Debug)]
pub(crate) struct NotFound(pub(crate) String);

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for NotFound {}

/// True if an error is a `NotFound`.
pub(crate) fn is_not_found(e: &(dyn std::error::Error + 'static)) -> bool {
    e.downcast_ref::<NotFound>().is_some()
}

/// Classify an HTTP error from a registry: 404 or 410 is `NotFound`; anything else is a failure.
pub(crate) fn http_error(
    registry: &str,
    name: &str,
    e: ureq::Error,
) -> Box<dyn std::error::Error> {
    match e {
        ureq::Error::StatusCode(404 | 410) => {
            Box::new(NotFound(format!("{name} not found on {registry}")))
        }
        e => format!("Cannot query {registry} for {name}: {e}").into(),
    }
}

/// The published versions of a package, sorted ascending by the ecosystem's version order (versions that do not parse are excluded).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RegistryVersions {
    /// The canonical package name, as the registry spells it.
    pub(crate) name: String,
    pub(crate) versions: Vec<String>,
}

//------------------------------------------------------------------------------
/// Sort versions ascending by the ecosystem's order, dropping those that do not parse.
fn sort_versions(eco: Ecosystem, versions: Vec<String>) -> Vec<String> {
    match eco {
        Ecosystem::Npm | Ecosystem::VsCode => {
            let mut v: Vec<(nodejs_semver::Version, String)> = versions
                .into_iter()
                .filter_map(|s| nodejs_semver::Version::parse(&s).ok().map(|p| (p, s)))
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v.into_iter().map(|(_, s)| s).collect()
        }
        Ecosystem::Cargo => {
            let mut v: Vec<(semver::Version, String)> = versions
                .into_iter()
                .filter_map(|s| semver::Version::parse(&s).ok().map(|p| (p, s)))
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v.into_iter().map(|(_, s)| s).collect()
        }
        Ecosystem::Gha => {
            // tags: v-prefixed or bare semver-like; a floating tag (`v4`) orders with `v4.0.0`, ties by name
            let mut v: Vec<(SemverLike, String)> = versions
                .into_iter()
                .filter_map(|s| SemverLike::parse(&s).map(|p| (p, s)))
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            v.into_iter().map(|(_, s)| s).collect()
        }
        Ecosystem::Python => versions,
    }
}

/// Path of a package in the crates.io sparse index.
fn crates_index_path(name: &str) -> String {
    let n = name.to_lowercase();
    match n.len() {
        1 => format!("1/{n}"),
        2 => format!("2/{n}"),
        3 => format!("3/{}/{n}", &n[..1]),
        _ => format!("{}/{}/{n}", &n[..2], &n[2..4]),
    }
}

fn parse_npm(name: &str, body: &str) -> ResultDynError<RegistryVersions> {
    let v: JsonValue = serde_json::from_str(body)
        .map_err(|e| format!("Failed to parse npm response for {name}: {e}"))?;
    let versions = v
        .get("versions")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| format!("No versions for {name} in npm response"))?
        .keys()
        .cloned()
        .collect();
    Ok(RegistryVersions {
        name: v
            .get("name")
            .and_then(JsonValue::as_str)
            .unwrap_or(name)
            .to_string(),
        versions,
    })
}

/// The sparse index has one JSON object per line, per version; yanked versions are excluded.
fn parse_crates_index(name: &str, body: &str) -> ResultDynError<RegistryVersions> {
    let mut canonical = name.to_string();
    let mut versions = Vec::new();
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        let e: JsonValue = serde_json::from_str(line)
            .map_err(|e| format!("Failed to parse crates.io index for {name}: {e}"))?;
        if let Some(n) = e.get("name").and_then(JsonValue::as_str) {
            canonical = n.to_string();
        }
        if e.get("yanked").and_then(JsonValue::as_bool) == Some(true) {
            continue;
        }
        if let Some(v) = e.get("vers").and_then(JsonValue::as_str) {
            versions.push(v.to_string());
        }
    }
    Ok(RegistryVersions {
        name: canonical,
        versions,
    })
}

/// Open VSX lists versions as keys of `allVersions` (plus aliases such as `latest`, which do not parse as versions and are dropped when sorting).
fn parse_open_vsx(name: &str, body: &str) -> ResultDynError<RegistryVersions> {
    let v: JsonValue = serde_json::from_str(body)
        .map_err(|e| format!("Failed to parse Open VSX response for {name}: {e}"))?;
    if let Some(err) = v.get("error").and_then(JsonValue::as_str) {
        return Err(Box::new(NotFound(format!(
            "{name} not found on Open VSX: {err}"
        ))));
    }
    let canonical = match (
        v.get("namespace").and_then(JsonValue::as_str),
        v.get("name").and_then(JsonValue::as_str),
    ) {
        (Some(ns), Some(n)) => format!("{ns}.{n}"),
        _ => name.to_string(),
    };
    let versions = v
        .get("allVersions")
        .and_then(JsonValue::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    Ok(RegistryVersions {
        name: canonical,
        versions,
    })
}

/// Tag names from `git ls-remote --tags --refs` output (`<sha>\trefs/tags/<tag>` per line).
pub(crate) fn parse_ls_remote_tags(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|l| l.split_once('\t').map(|(_, r)| r))
        .filter_map(|r| r.strip_prefix("refs/tags/"))
        .map(str::to_string)
        .collect()
}

/// List the tags of a GitHub repository. Prompts are disabled: GitHub asks for credentials for a repository that does not exist (or is private), which would otherwise hang.
fn git_ls_remote_tags(repo: &str) -> ResultDynError<Vec<String>> {
    let url = format!("https://github.com/{repo}");
    let output = Command::new("git")
        .args(["ls-remote", "--tags", "--refs", &url])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            format!("Failed to run git (required to list GitHub Actions tags): {e}")
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = format!("Cannot list tags for {url}: {}", stderr.trim());
        // GitHub asks for credentials for a repository that does not exist (or is private)
        return Err(
            if stderr.contains("Repository not found")
                || stderr.contains("could not read Username")
            {
                Box::new(NotFound(msg))
            } else {
                msg.into()
            },
        );
    }
    Ok(parse_ls_remote_tags(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// Query the registry for an ecosystem (not Python).
fn query(
    client: &Arc<dyn UreqClient>,
    eco: Ecosystem,
    name: &str,
) -> ResultDynError<RegistryVersions> {
    match eco {
        Ecosystem::Npm => {
            let url = format!("https://registry.npmjs.org/{}", name.replace('/', "%2f"));
            let body = client
                .get_with_accept(&url, NPM_ACCEPT)
                .map_err(|e| http_error("the npm registry", name, e))?;
            parse_npm(name, &body)
        }
        Ecosystem::Cargo => {
            let url = format!("https://index.crates.io/{}", crates_index_path(name));
            let body = client
                .get(&url)
                .map_err(|e| http_error("crates.io", name, e))?;
            parse_crates_index(name, &body)
        }
        Ecosystem::VsCode => {
            let (publisher, ext) = name.split_once('.').ok_or_else(|| {
                format!("A VS Code extension is publisher.name, not {name:?}")
            })?;
            let url = format!("https://open-vsx.org/api/{publisher}/{ext}");
            let body = client
                .get(&url)
                .map_err(|e| http_error("Open VSX", name, e))?;
            parse_open_vsx(name, &body)
        }
        Ecosystem::Gha => Ok(RegistryVersions {
            name: name.to_string(),
            versions: git_ls_remote_tags(name)?,
        }),
        Ecosystem::Python => Err("Python versions come from PyPI (pypi_project)".into()),
    }
}

/// List the published versions of a package, sorted ascending, using a time-limited cache. Failures are errors and are not cached.
pub(crate) fn list_versions(
    client: &Arc<dyn UreqClient>,
    eco: Ecosystem,
    name: &str,
    cache_config: &CacheConfig,
    log: FlagLog,
) -> ResultDynError<RegistryVersions> {
    let cache_fp = cache_config
        .directory
        .join(format!(
            "registry_{}_{}",
            eco.cli_name(),
            hash_string(&eco.name_to_key(name))
        ))
        .with_extension("json");
    if cache_config.duration != DURATION_0
        && path_within_duration(&cache_fp, cache_config.duration)
    {
        if let Some(rv) = fs::read_to_string(&cache_fp)
            .ok()
            .and_then(|c| serde_json::from_str::<RegistryVersions>(&c).ok())
        {
            logger!(log, module_path!(), "Loaded registry cache: {:?}", cache_fp);
            return Ok(rv);
        }
    }
    let mut rv = query(client, eco, name)?;
    rv.versions = sort_versions(eco, rv.versions);
    if cache_config.duration != DURATION_0 {
        if let Ok(json) = serde_json::to_string(&rv) {
            let _ = fs::write(&cache_fp, json);
        }
    }
    Ok(rv)
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ureq_client::UreqClientMock;
    use crate::util::path_cache;
    use std::collections::HashMap;

    #[test]
    fn test_crates_index_path_a() {
        assert_eq!(crates_index_path("a"), "1/a");
        assert_eq!(crates_index_path("ab"), "2/ab");
        assert_eq!(crates_index_path("abc"), "3/a/abc");
        assert_eq!(crates_index_path("serde"), "se/rd/serde");
        assert_eq!(crates_index_path("Inflector"), "in/fl/inflector");
    }

    #[test]
    fn test_sort_versions_a() {
        let s = |e: Ecosystem, v: &[&str]| {
            sort_versions(e, v.iter().map(|s| s.to_string()).collect())
        };
        assert_eq!(
            s(
                Ecosystem::Npm,
                &["1.10.0", "1.2.0", "2.0.0-rc.1", "2.0.0", "latest"]
            ),
            vec!["1.2.0", "1.10.0", "2.0.0-rc.1", "2.0.0"]
        );
        assert_eq!(
            s(Ecosystem::Cargo, &["0.10.0", "0.9.1"]),
            vec!["0.9.1", "0.10.0"]
        );
        assert_eq!(
            s(
                Ecosystem::Gha,
                &["v4.1.0", "v10", "v4", "v4.0.0", "release-x"]
            ),
            vec!["v4", "v4.0.0", "v4.1.0", "v10"]
        );
    }

    #[test]
    fn test_parsers_a() {
        let npm = parse_npm("@babel/core", r#"{"name": "@babel/core", "dist-tags": {"latest": "7.24.0"}, "versions": {"7.23.0": {}, "7.24.0": {}}}"#).unwrap();
        assert_eq!(npm.versions.len(), 2);
        assert!(parse_npm("x", "{}").is_err());

        let idx = "{\"name\":\"Inflector\",\"vers\":\"0.10.0\",\"yanked\":false}\n{\"name\":\"Inflector\",\"vers\":\"0.11.0\",\"yanked\":true}\n{\"name\":\"Inflector\",\"vers\":\"0.11.4\",\"yanked\":false}\n";
        let c = parse_crates_index("inflector", idx).unwrap();
        // canonical name; yanked excluded
        assert_eq!(c.name, "Inflector");
        assert_eq!(c.versions, vec!["0.10.0", "0.11.4"]);

        let ov = parse_open_vsx("codeinklingon.git-worktree-menu", r#"{"namespace": "CodeInKlingon", "name": "git-worktree-menu", "allVersions": {"latest": "u", "1.0.9": "u", "1.0.8": "u"}}"#).unwrap();
        // canonical (case-preserving) name
        assert_eq!(ov.name, "CodeInKlingon.git-worktree-menu");
        assert_eq!(
            sort_versions(Ecosystem::VsCode, ov.versions),
            vec!["1.0.8", "1.0.9"]
        );
        assert!(
            parse_open_vsx("a.b", r#"{"error": "Extension not found: a.b"}"#).is_err()
        );

        let tags = parse_ls_remote_tags("af51\trefs/tags/v1.0.0\n5440\trefs/tags/v1\n");
        assert_eq!(tags, vec!["v1.0.0", "v1"]);
    }

    #[test]
    fn test_list_versions_mock_a() {
        let mut m = HashMap::new();
        m.insert(
            "https://registry.npmjs.org/lodash".to_string(),
            r#"{"name": "lodash", "versions": {"4.17.21": {}, "4.17.15": {}, "4.17.9": {}}}"#.to_string(),
        );
        m.insert(
            "https://index.crates.io/se/rd/serde".to_string(),
            "{\"name\":\"serde\",\"vers\":\"1.0.200\",\"yanked\":false}\n".to_string(),
        );
        let client: Arc<dyn UreqClient> = Arc::new(UreqClientMock {
            mock_post: None,
            mock_get: Some(m),
        });
        let cc = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let npm = list_versions(&client, Ecosystem::Npm, "lodash", &cc, FlagLog(false))
            .unwrap();
        assert_eq!(npm.versions, vec!["4.17.9", "4.17.15", "4.17.21"]);
        let c = list_versions(&client, Ecosystem::Cargo, "serde", &cc, FlagLog(false))
            .unwrap();
        assert_eq!(c.versions, vec!["1.0.200"]);
        // a missing package is NotFound; a malformed name is another error
        let e = list_versions(&client, Ecosystem::VsCode, "x.y", &cc, FlagLog(false))
            .unwrap_err();
        assert!(is_not_found(e.as_ref()), "{e}");
        let e = list_versions(&client, Ecosystem::VsCode, "nodot", &cc, FlagLog(false))
            .unwrap_err();
        assert!(!is_not_found(e.as_ref()), "{e}");
    }

    #[test]
    #[ignore] // hits live registries and runs git
    fn test_list_versions_live() {
        let client: Arc<dyn UreqClient> = Arc::new(crate::ureq_client::UreqClientLive);
        let cc = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let lv = |e, n| list_versions(&client, e, n, &cc, FlagLog(false)).unwrap();
        assert!(lv(Ecosystem::Npm, "@babel/core")
            .versions
            .contains(&"7.24.0".to_string()));
        assert!(lv(Ecosystem::Cargo, "serde")
            .versions
            .contains(&"1.0.200".to_string()));
        assert!(lv(Ecosystem::Gha, "actions/checkout")
            .versions
            .contains(&"v4.1.0".to_string()));
        assert_eq!(
            lv(Ecosystem::VsCode, "codeinklingon.git-worktree-menu").name,
            "CodeInKlingon.git-worktree-menu"
        );
        let e = list_versions(
            &client,
            Ecosystem::Gha,
            "fetter-io/no-such-repo-xyz",
            &cc,
            FlagLog(false),
        )
        .unwrap_err();
        assert!(is_not_found(e.as_ref()), "{e}");
        let e = list_versions(
            &client,
            Ecosystem::Npm,
            "no-such-pkg-zzzz-fetter",
            &cc,
            FlagLog(false),
        )
        .unwrap_err();
        assert!(is_not_found(e.as_ref()), "{e}");
    }
}
