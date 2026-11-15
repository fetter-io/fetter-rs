//! Package registries for non-Python ecosystems: list the published versions of a package (for `lookup-name` / `lookup-bound`). Python uses PyPI via `pypi_project.rs`.
//!
//! - npm: `https://registry.npmjs.org/<name>` with the abbreviated metadata format.
//! - Cargo: the crates.io sparse index (`https://index.crates.io/…`), the endpoint cargo itself uses; yanked versions are excluded. (The crates.io web API requires a User-Agent and is rate limited.)
//! - GitHub Actions: tags from `git ls-remote --tags --refs https://github.com/<owner>/<repo>` (no token or API rate limit; requires `git`).
//! - VS Code: Open VSX (`https://open-vsx.org/api/<publisher>/<name>`).
//!
//! Each registry also gives the package's canonical name, which matters where OSV names are case-sensitive (VS Code).

use std::fs;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::eco::go::proxy_escape;
use crate::eco::go::semver_of as go_semver;
use crate::eco::php::ComposerVersion;
use crate::eco::ruby::GemVersion;
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
        Ecosystem::Gha | Ecosystem::PreCommit | Ecosystem::Swift => {
            // tags: v-prefixed or bare semver-like; a floating tag (`v4`) orders with `v4.0.0`, ties by name
            let mut v: Vec<(SemverLike, String)> = versions
                .into_iter()
                .filter_map(|s| SemverLike::parse(&s).map(|p| (p, s)))
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            v.into_iter().map(|(_, s)| s).collect()
        }
        Ecosystem::Go => {
            let mut v: Vec<(semver::Version, String)> = versions
                .into_iter()
                .filter_map(|s| go_semver(&s).map(|p| (p, s)))
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v.into_iter().map(|(_, s)| s).collect()
        }
        Ecosystem::Ruby => {
            let mut v: Vec<(GemVersion, String)> = versions
                .into_iter()
                .filter_map(|s| GemVersion::parse(&s).map(|p| (p, s)))
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v.into_iter().map(|(_, s)| s).collect()
        }
        Ecosystem::Php => {
            let mut v: Vec<(ComposerVersion, String)> = versions
                .into_iter()
                .filter_map(|s| ComposerVersion::parse(&s).map(|p| (p, s)))
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v.into_iter().map(|(_, s)| s).collect()
        }
        Ecosystem::Python => versions,
    }
}

/// Versions from the rubygems.org versions API (`[{"number": "1.2.3", "platform": "ruby", ...}, ...]`), one per number.
fn parse_rubygems(name: &str, body: &str) -> ResultDynError<RegistryVersions> {
    let v: serde_json::Value = serde_json::from_str(body)?;
    let mut versions: Vec<String> = v
        .as_array()
        .ok_or_else(|| format!("Unexpected rubygems.org response for {name}"))?
        .iter()
        .filter_map(|e| e.get("number").and_then(|n| n.as_str()).map(str::to_string))
        .collect();
    versions.sort();
    versions.dedup();
    Ok(RegistryVersions {
        name: name.to_string(),
        versions,
    })
}

/// Versions from a Packagist `p2` metadata file (`{"packages": {"vendor/name": [{"version": "1.2.3", ...}, ...]}}`; minified entries omit keys unchanged from the previous entry, but always carry `version`).
fn parse_packagist(name: &str, body: &str) -> ResultDynError<RegistryVersions> {
    let v: serde_json::Value = serde_json::from_str(body)?;
    let entries = v
        .get("packages")
        .and_then(|p| p.get(name))
        .and_then(|e| e.as_array())
        .ok_or_else(|| format!("Unexpected Packagist response for {name}"))?;
    let mut versions: Vec<String> = entries
        .iter()
        .filter_map(|e| {
            e.get("version")
                .and_then(|n| n.as_str())
                .map(str::to_string)
        })
        .collect();
    versions.sort();
    versions.dedup();
    Ok(RegistryVersions {
        name: name.to_string(),
        versions,
    })
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

/// Classify a `git ls-remote` failure: GitHub asks for credentials for a repository that does not exist (or is private), so either message means not found.
fn ls_remote_error(url: &str, stderr: &str) -> Box<dyn std::error::Error> {
    let msg = format!("Cannot list tags for {url}: {stderr}");
    if stderr.contains("Repository not found")
        || stderr.contains("could not read Username")
    {
        Box::new(NotFound(msg))
    } else {
        msg.into()
    }
}

/// List the tags of a git repository.
fn git_ls_remote_tags(
    client: &Arc<dyn UreqClient>,
    url: &str,
) -> ResultDynError<Vec<String>> {
    let out = client
        .git_ls_remote_tags(url, true)
        .map_err(|e| ls_remote_error(url, &e))?;
    Ok(parse_ls_remote_tags(&out))
}

/// Map each commit to the tags that point to it, from `git ls-remote --tags` output with peeled lines: a lightweight tag's line names its commit; an annotated tag's line names the tag object, and its `^{}` line names the commit. Tags are sorted.
pub(crate) fn parse_tag_commits(
    output: &str,
) -> std::collections::HashMap<String, Vec<String>> {
    let mut out: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for line in output.lines() {
        let Some((sha, r)) = line.split_once('\t') else {
            continue;
        };
        let Some(tag) = r.strip_prefix("refs/tags/") else {
            continue;
        };
        let tag = tag.strip_suffix("^{}").unwrap_or(tag);
        let tags = out.entry(sha.to_lowercase()).or_default();
        if !tags.iter().any(|t| t == tag) {
            tags.push(tag.to_string());
        }
    }
    for tags in out.values_mut() {
        tags.sort();
    }
    out
}

/// The tags of a git repository (a GitHub Actions or pre-commit hook repository) by commit (see `parse_tag_commits`), using the registry's time-limited cache.
pub(crate) fn tag_commits(
    client: &Arc<dyn UreqClient>,
    url: &str,
    cache_config: &CacheConfig,
    log: FlagLog,
) -> ResultDynError<std::collections::HashMap<String, Vec<String>>> {
    let cache_fp = cache_config
        .directory
        .join(format!(
            "registry_gha_commits_{}",
            hash_string(&url.to_lowercase())
        ))
        .with_extension("json");
    if cache_config.duration != DURATION_0
        && path_within_duration(&cache_fp, cache_config.duration)
    {
        if let Some(m) = fs::read_to_string(&cache_fp)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
        {
            logger!(
                log,
                module_path!(),
                "Loaded tag commit cache: {:?}",
                cache_fp
            );
            return Ok(m);
        }
    }
    let out = client
        .git_ls_remote_tags(url, false)
        .map_err(|e| ls_remote_error(url, &e))?;
    let m = parse_tag_commits(&out);
    if cache_config.duration != DURATION_0 {
        if let Ok(json) = serde_json::to_string(&m) {
            let _ = fs::write(&cache_fp, json);
        }
    }
    Ok(m)
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
            versions: git_ls_remote_tags(client, &format!("https://github.com/{name}"))?,
        }),
        // a SwiftPM package's versions are its repository's semver tags
        Ecosystem::Swift => Ok(RegistryVersions {
            name: name.to_string(),
            versions: git_ls_remote_tags(client, &crate::eco::precommit::repo_url(name))?
                .into_iter()
                .map(|t| t.strip_prefix('v').map(str::to_string).unwrap_or(t))
                .collect(),
        }),
        Ecosystem::PreCommit => Ok(RegistryVersions {
            name: name.to_string(),
            versions: git_ls_remote_tags(client, &crate::eco::precommit::repo_url(name))?,
        }),
        Ecosystem::Go => {
            // tagged versions, one per line; pseudo-versions are not listed
            let url = format!("https://proxy.golang.org/{}/@v/list", proxy_escape(name));
            let body = client
                .get(&url)
                .map_err(|e| http_error("the Go module proxy", name, e))?;
            Ok(RegistryVersions {
                name: name.to_string(),
                versions: body
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        }
        Ecosystem::Ruby => {
            // one entry per version and platform; names are case-sensitive
            let url = format!("https://rubygems.org/api/v1/versions/{name}.json");
            let body = client
                .get(&url)
                .map_err(|e| http_error("rubygems.org", name, e))?;
            parse_rubygems(name, &body)
        }
        Ecosystem::Php => {
            // tagged releases; branch versions are in a separate `~dev` file, not read
            let key = name.to_lowercase();
            let url = format!("https://repo.packagist.org/p2/{key}.json");
            let body = client
                .get(&url)
                .map_err(|e| http_error("Packagist", name, e))?;
            parse_packagist(&key, &body)
        }
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
        assert_eq!(
            s(
                Ecosystem::Go,
                &[
                    "v1.10.0",
                    "v1.2.0",
                    "v2.0.0+incompatible",
                    "v1.2.0-rc.1",
                    "1.0.0"
                ]
            ),
            vec!["v1.2.0-rc.1", "v1.2.0", "v1.10.0", "v2.0.0+incompatible"]
        );
        assert_eq!(
            s(
                Ecosystem::Ruby,
                &["7.10.0", "7.1.3.4", "8.0.0.rc1", "8.0.0", "x"]
            ),
            vec!["7.1.3.4", "7.10.0", "8.0.0.rc1", "8.0.0"]
        );
        assert_eq!(
            s(
                Ecosystem::Php,
                &["v1.10.0", "1.2.0", "1.2.0-RC1", "dev-main", "v2.0.0"]
            ),
            vec!["1.2.0-RC1", "1.2.0", "v1.10.0", "v2.0.0"]
        );
        assert_eq!(
            s(
                Ecosystem::Ruby,
                &["7.10.0", "7.1.3.4", "8.0.0.rc1", "8.0.0", "x"]
            ),
            vec!["7.1.3.4", "7.10.0", "8.0.0.rc1", "8.0.0"]
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

        // annotated tags: the peeled `^{}` line names the commit
        let m = parse_tag_commits(
            "1111\trefs/tags/v2.0.0\nAAAA\trefs/tags/v2.0.0^{}\naaaa\trefs/tags/v2\nbbbb\trefs/heads/main\n",
        );
        assert_eq!(m["aaaa"], vec!["v2", "v2.0.0"]);
        assert_eq!(m["1111"], vec!["v2.0.0"]);
        assert!(!m.contains_key("bbbb"));
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
        m.insert(
            "git+https://github.com/apple/swift-nio".to_string(),
            "aaaa\trefs/tags/2.10.0\nbbbb\trefs/tags/v2.0.0\ncccc\trefs/tags/nightly\n"
                .to_string(),
        );
        m.insert(
            "https://repo.packagist.org/p2/guzzlehttp/guzzle.json".to_string(),
            r#"{"minified": "composer/2.0", "packages": {"guzzlehttp/guzzle": [{"version": "7.9.2", "require": {}}, {"version": "7.4.0"}, {"version": "v6.5.8"}]}}"#.to_string(),
        );
        m.insert(
            "https://rubygems.org/api/v1/versions/nokogiri.json".to_string(),
            r#"[{"number": "1.16.5", "platform": "ruby"}, {"number": "1.16.5", "platform": "arm64-darwin"}, {"number": "1.15.6", "platform": "ruby"}]"#.to_string(),
        );
        m.insert(
            "https://proxy.golang.org/github.com/!burnt!sushi/toml/@v/list".to_string(),
            "v1.3.0\nv0.3.0\nv1.6.0\n".to_string(),
        );
        let client: Arc<dyn UreqClient> = Arc::new(UreqClientMock {
            mock_post: None,
            mock_get: Some(m),
        });
        let cc = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let go = list_versions(
            &client,
            Ecosystem::Go,
            "github.com/BurntSushi/toml",
            &cc,
            FlagLog(false),
        )
        .unwrap();
        assert_eq!(go.name, "github.com/BurntSushi/toml");
        assert_eq!(go.versions, vec!["v0.3.0", "v1.3.0", "v1.6.0"]);
        // Swift: semver tags of the repository, without a `v`
        let sw = list_versions(
            &client,
            Ecosystem::Swift,
            "github.com/apple/swift-nio",
            &cc,
            FlagLog(false),
        )
        .unwrap();
        assert_eq!(sw.versions, vec!["2.0.0", "2.10.0"]);
        // names are case-insensitive on Packagist; the canonical name is lowercase
        let php = list_versions(
            &client,
            Ecosystem::Php,
            "GuzzleHttp/Guzzle",
            &cc,
            FlagLog(false),
        )
        .unwrap();
        assert_eq!(php.name, "guzzlehttp/guzzle");
        assert_eq!(php.versions, vec!["v6.5.8", "7.4.0", "7.9.2"]);
        let e = list_versions(&client, Ecosystem::Php, "acme/none", &cc, FlagLog(false))
            .unwrap_err();
        assert!(is_not_found(e.as_ref()), "{e}");
        let rb = list_versions(&client, Ecosystem::Ruby, "nokogiri", &cc, FlagLog(false))
            .unwrap();
        // one entry per version, not per platform
        assert_eq!(rb.versions, vec!["1.15.6", "1.16.5"]);
        let e = list_versions(&client, Ecosystem::Ruby, "Nokogiri", &cc, FlagLog(false))
            .unwrap_err();
        assert!(is_not_found(e.as_ref()), "{e}");
        let e = list_versions(
            &client,
            Ecosystem::Go,
            "example.com/none",
            &cc,
            FlagLog(false),
        )
        .unwrap_err();
        assert!(is_not_found(e.as_ref()), "{e}");
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
        assert!(lv(Ecosystem::Go, "github.com/BurntSushi/toml")
            .versions
            .contains(&"v1.3.0".to_string()));
        assert!(lv(Ecosystem::Ruby, "nokogiri")
            .versions
            .contains(&"1.16.5".to_string()));
        assert!(lv(Ecosystem::Php, "guzzlehttp/guzzle")
            .versions
            .contains(&"7.4.0".to_string()));
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
