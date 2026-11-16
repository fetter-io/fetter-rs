//! Client-side OSV version matching, for ecosystems where the OSV API does not match versions server-side (see `Ecosystem::osv_server_side_versions`). Currently GitHub Actions: its advisories carry only `ECOSYSTEM` ranges, which OSV does not evaluate for that ecosystem, so a versioned query always returns nothing.
//!
//! Packages are queried by name only (returning all advisories with their `affected` data), and each package's ref is evaluated locally. A ref is interpreted by convention (see `RefKind`).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::ecosystem::Ecosystem;
use crate::package::Package;
use crate::ureq_client::UreqClient;
use crate::util::hash_string;
use crate::util::logger;
use crate::util::path_within_duration;
use crate::util::CacheConfig;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::DURATION_0;

const OSV_QUERY_URL: &str = "https://api.osv.dev/v1/query";
/// Upper bound on requests per package while following pagination.
const OSV_MAX_PAGES: usize = 100;

//------------------------------------------------------------------------------
/// A `MAJOR[.MINOR[.PATCH]][-PRE]` version with an optional `v` prefix; missing components are 0. A pre-release orders before its release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SemverLike {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Option<String>,
}

impl SemverLike {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('v').unwrap_or(s);
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p.to_string())),
            None => (s, None),
        };
        // build metadata is ignored for ordering
        let core = core.split('+').next()?;
        let mut parts = core.split('.');
        let mut next = || -> Option<Option<u64>> {
            match parts.next() {
                None => Some(None),
                Some(p) => p.parse::<u64>().ok().map(Some),
            }
        };
        let major = next()??;
        let minor = next()?.unwrap_or(0);
        let patch = next()?.unwrap_or(0);
        if parts.next().is_some() {
            return None;
        }
        Some(SemverLike {
            major,
            minor,
            patch,
            pre,
        })
    }

    fn new(major: u64, minor: u64, patch: u64) -> Self {
        SemverLike {
            major,
            minor,
            patch,
            pre: None,
        }
    }
}

impl Ord for SemverLike {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for SemverLike {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

//------------------------------------------------------------------------------
/// How a package's version (a GitHub Actions ref) is interpreted.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RefKind {
    /// `@v4.1.0`, or a SHA pin whose comment gives a full version: exactly this version.
    Exact(SemverLike),
    /// `@v4` or `@v4.1`: by convention a moving tag at the newest release in that line, `[low, high)`.
    Floating { low: SemverLike, high: SemverLike },
    /// `@main`, `@master`, `@release/v1`: by convention at or past the newest release.
    Branch,
    /// A SHA without a version comment (or with only a major/minor comment): the version cannot be determined without resolving the commit.
    Unresolvable,
}

pub(crate) fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

impl RefKind {
    pub(crate) fn classify(version: &str) -> Self {
        let v = version.trim();
        if v.is_empty() || is_sha(v) {
            return RefKind::Unresolvable;
        }
        let core = v.strip_prefix('v').unwrap_or(v);
        let parts: Vec<&str> = core.split('.').collect();
        let numeric = |p: &&str| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit());
        if parts.iter().all(numeric) {
            let n: Vec<u64> = parts.iter().filter_map(|p| p.parse().ok()).collect();
            return match n.as_slice() {
                [major] => RefKind::Floating {
                    low: SemverLike::new(*major, 0, 0),
                    high: SemverLike::new(major + 1, 0, 0),
                },
                [major, minor] => RefKind::Floating {
                    low: SemverLike::new(*major, *minor, 0),
                    high: SemverLike::new(*major, minor + 1, 0),
                },
                _ => SemverLike::parse(v).map_or(RefKind::Branch, RefKind::Exact),
            };
        }
        match SemverLike::parse(v) {
            Some(s) => RefKind::Exact(s),
            None => RefKind::Branch,
        }
    }
}

/// The version a commit is released as, from the tags that point to it: the highest full version (`v4.1.1`) if any, else the most specific floating tag (`v4.1` over `v4`). None if no tag names a version.
pub(crate) fn version_from_tags(tags: &[String]) -> Option<String> {
    let exact = tags
        .iter()
        .filter_map(|t| match RefKind::classify(t) {
            RefKind::Exact(v) => Some((v, t)),
            _ => None,
        })
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(a.1)));
    if let Some((_, t)) = exact {
        return Some(t.clone());
    }
    tags.iter()
        .filter(|t| matches!(RefKind::classify(t), RefKind::Floating { .. }))
        .max_by_key(|t| (t.matches('.').count(), SemverLike::parse(t)))
        .cloned()
}

/// A SHA pin without a version comment, resolved to the tag that points to its commit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedPin {
    pub name: String,
    pub commit: String,
    pub version: String,
}

/// A SHA pin whose version comment does not match its commit: the tag the comment names points to another commit, or does not exist. The comment may be stale (the pin was updated without it) or written to mislead; the SHA is what runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PinMismatch {
    pub name: String,
    pub commit: String,
    /// The version the comment names.
    pub comment: String,
    /// The version the commit is tagged as, if any; the pin is audited as this version, or is unaudited if none.
    pub actual: Option<String>,
    /// True if the tag the comment names exists (on another commit); false if no such tag exists.
    pub tag_exists: bool,
}

/// The outcome of checking GitHub Actions SHA pins against their repositories' tags.
#[derive(Debug, Default)]
pub(crate) struct PinCheck {
    /// The packages, with resolved and corrected pins updated.
    pub(crate) packages: Vec<Package>,
    pub(crate) resolved: Vec<ResolvedPin>,
    pub(crate) mismatches: Vec<PinMismatch>,
}

/// True if `tag` names the same version as a comment, ignoring a leading `v` (`# 4.1.1` names `v4.1.1`).
fn tag_matches(tag: &str, comment: &str) -> bool {
    tag == comment
        || tag.strip_prefix('v').unwrap_or(tag)
            == comment.strip_prefix('v').unwrap_or(comment)
}

/// Check GitHub Actions SHA pins against the tags of their repositories, listed with `git ls-remote` (see `registry::tag_commits`; once per repository, in parallel):
/// - A pin without a version comment (its version is the SHA) is resolved to the most specific tag on its commit (`version_from_tags`) and takes that version, as a pin with that comment would.
/// - A pin with a version comment is checked: if the tag the comment names is not on the pinned commit, it is a mismatch, and the pin takes the version its commit is actually tagged as, or (with no version tag) the SHA, which leaves it unaudited.
///
/// The SHA stays in `direct_url`, with `vcs_info.revision` set to the version used. A repository that cannot be listed leaves its pins unchanged (unresolved pins stay unaudited; commented pins are audited by the comment).
pub(crate) fn check_sha_pins(
    client: &Arc<dyn UreqClient>,
    packages: &[Package],
    cache_config: &CacheConfig,
    log: FlagLog,
) -> PinCheck {
    // the pinned commit of a GHA or pre-commit package, if it is a SHA pin
    let pinned = |p: &Package| -> Option<String> {
        if !matches!(
            p.ecosystem,
            Ecosystem::Gha | Ecosystem::PreCommit | Ecosystem::Swift
        ) {
            return None;
        }
        let v = p.version.to_string();
        if is_sha(&v) {
            return Some(v.to_lowercase());
        }
        // a versioned Swift pin was resolved by SwiftPM; only revision pins are checked
        if p.ecosystem == Ecosystem::Swift {
            return None;
        }
        p.direct_url
            .as_ref()?
            .vcs_info
            .as_ref()
            .map(|c| c.commit_id.to_lowercase())
            .filter(|c| is_sha(c))
    };
    let url_of = |p: &Package| -> String {
        match p.ecosystem {
            Ecosystem::PreCommit | Ecosystem::Swift => {
                crate::eco::precommit::repo_url(&p.name)
            }
            _ => format!("https://github.com/{}", p.name),
        }
        .to_lowercase()
    };
    let mut repos: Vec<String> = packages
        .iter()
        .filter(|p| pinned(p).is_some())
        .map(url_of)
        .collect();
    repos.sort();
    repos.dedup();
    let commits: HashMap<String, HashMap<String, Vec<String>>> = repos
        .into_par_iter()
        .filter_map(|repo| {
            match crate::registry::tag_commits(client, &repo, cache_config, log) {
                Ok(m) => Some((repo, m)),
                Err(e) => {
                    logger!(
                        log,
                        module_path!(),
                        "Cannot check SHA pins of {}: {}",
                        repo,
                        e
                    );
                    None
                }
            }
        })
        .collect();
    let mut check = PinCheck::default();
    let with_version = |p: &Package, version: &str| -> Package {
        let mut direct_url = p.direct_url.clone();
        if let Some(vcs) = direct_url.as_mut().and_then(|d| d.vcs_info.as_mut()) {
            vcs.revision = (!is_sha(version)).then(|| version.to_string());
        }
        Package::new(
            p.ecosystem,
            &p.name,
            crate::version_spec::VersionSpec::new(version),
            direct_url,
        )
    };
    for p in packages {
        let (Some(sha), Some(tags_by_commit)) = (pinned(p), commits.get(&url_of(p)))
        else {
            check.packages.push(p.clone());
            continue;
        };
        let tags_here = tags_by_commit.get(&sha).map(Vec::as_slice).unwrap_or(&[]);
        let actual = version_from_tags(tags_here);
        let version = p.version.to_string();
        if is_sha(&version) {
            // no version comment
            match actual {
                Some(v) => {
                    check.resolved.push(ResolvedPin {
                        name: p.name.clone(),
                        commit: sha.clone(),
                        version: v.clone(),
                    });
                    check.packages.push(with_version(p, &v));
                }
                None => check.packages.push(p.clone()),
            }
            continue;
        }
        if tags_here.iter().any(|t| tag_matches(t, &version)) {
            check.packages.push(p.clone());
            continue;
        }
        let tag_exists = tags_by_commit
            .values()
            .flatten()
            .any(|t| tag_matches(t, &version));
        check.mismatches.push(PinMismatch {
            name: p.name.clone(),
            commit: sha.clone(),
            comment: version.clone(),
            actual: actual.clone(),
            tag_exists,
        });
        check
            .packages
            .push(with_version(p, actual.as_deref().unwrap_or(&sha)));
    }
    check
        .resolved
        .sort_by(|a, b| (&a.name, &a.commit).cmp(&(&b.name, &b.commit)));
    check.resolved.dedup();
    check.mismatches.sort_by(|a, b| {
        (&a.name, &a.commit, &a.comment).cmp(&(&b.name, &b.commit, &b.comment))
    });
    check.mismatches.dedup();
    check
}

/// True if a package of a client-side-matched ecosystem can be evaluated.
pub(crate) fn is_client_side_queryable(package: &Package) -> bool {
    RefKind::classify(&package.version.to_string()) != RefKind::Unresolvable
}

//------------------------------------------------------------------------------
// OSV records (the subset needed for matching)

#[derive(Debug, Clone, Deserialize, Serialize)]
struct OSVPackageId {
    name: String,
    ecosystem: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct OSVRange {
    #[serde(rename = "type")]
    range_type: String,
    #[serde(default)]
    events: Vec<HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct OSVAffected {
    package: Option<OSVPackageId>,
    #[serde(default)]
    ranges: Vec<OSVRange>,
    #[serde(default)]
    versions: Vec<String>,
    database_specific: Option<JsonValue>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct OSVVulnRecord {
    id: String,
    #[serde(default)]
    affected: Vec<OSVAffected>,
}

#[derive(Debug, Deserialize)]
struct OSVQueryResponse {
    #[serde(default)]
    vulns: Vec<OSVVulnRecord>,
    next_page_token: Option<String>,
}

//------------------------------------------------------------------------------
// range evaluation

#[derive(Debug, Clone, PartialEq)]
enum Upper {
    Unbounded,
    Exclusive(SemverLike),
    Inclusive(SemverLike),
}

/// A contiguous affected interval: `[low, upper)` or `[low, upper]`.
#[derive(Debug, Clone, PartialEq)]
struct Interval {
    low: SemverLike,
    upper: Upper,
}

impl Interval {
    fn contains(&self, v: &SemverLike) -> bool {
        *v >= self.low
            && match &self.upper {
                Upper::Unbounded => true,
                Upper::Exclusive(u) => v < u,
                Upper::Inclusive(u) => v <= u,
            }
    }
}

/// Parse GHSA's `last_known_affected_version_range` (e.g. `< 3.0.0`, `<= 45.0.7`), used to bound an otherwise open range.
fn last_known_bound(affected: &OSVAffected) -> Option<Upper> {
    let s = affected
        .database_specific
        .as_ref()?
        .get("last_known_affected_version_range")?
        .as_str()?
        .trim();
    if let Some(v) = s.strip_prefix("<=") {
        SemverLike::parse(v).map(Upper::Inclusive)
    } else if let Some(v) = s.strip_prefix('<') {
        SemverLike::parse(v).map(Upper::Exclusive)
    } else {
        None
    }
}

/// Convert the `ECOSYSTEM` and `SEMVER` ranges of an affected entry into intervals. `GIT` ranges (bounded by commits) are not evaluated. An interval left open after its last event is bounded by `last_known_affected_version_range` if present.
fn intervals(affected: &OSVAffected) -> Vec<Interval> {
    let mut out = Vec::new();
    for range in &affected.ranges {
        if range.range_type != "ECOSYSTEM" && range.range_type != "SEMVER" {
            continue;
        }
        let mut low: Option<SemverLike> = None;
        for event in &range.events {
            if let Some(v) = event.get("introduced") {
                low = SemverLike::parse(v);
            } else if let Some(v) = event.get("fixed").or_else(|| event.get("limit")) {
                if let (Some(l), Some(u)) = (low.take(), SemverLike::parse(v)) {
                    out.push(Interval {
                        low: l,
                        upper: Upper::Exclusive(u),
                    });
                }
            } else if let Some(v) = event.get("last_affected") {
                if let (Some(l), Some(u)) = (low.take(), SemverLike::parse(v)) {
                    out.push(Interval {
                        low: l,
                        upper: Upper::Inclusive(u),
                    });
                }
            }
        }
        if let Some(l) = low {
            out.push(Interval {
                low: l,
                upper: last_known_bound(affected).unwrap_or(Upper::Unbounded),
            });
        }
    }
    out
}

/// Is a ref affected by this affected entry?
/// - Exact: within an interval, or listed in `versions`.
/// - Floating `[low, high)`: the newest release in the line is affected, i.e. an interval overlaps the line and is not fixed within it. A `last_affected` bound within the line counts as unfixed.
/// - Branch: an interval with no fix at all.
fn ref_affected(kind: &RefKind, affected: &OSVAffected) -> bool {
    let ivs = intervals(affected);
    match kind {
        RefKind::Exact(v) => {
            ivs.iter().any(|i| i.contains(v))
                || affected
                    .versions
                    .iter()
                    .any(|s| SemverLike::parse(s).as_ref() == Some(v))
        }
        RefKind::Floating { low, high } => ivs.iter().any(|i| {
            i.low < *high
                && match &i.upper {
                    Upper::Unbounded => true,
                    Upper::Exclusive(u) => u >= high,
                    Upper::Inclusive(u) => u >= low,
                }
        }),
        RefKind::Branch => ivs.iter().any(|i| i.upper == Upper::Unbounded),
        RefKind::Unresolvable => false,
    }
}

/// Return the IDs of advisories affecting a package (matching on the package name, case-insensitively, and ecosystem).
fn affecting_ids(package: &Package, vulns: &[OSVVulnRecord]) -> Vec<String> {
    let kind = RefKind::classify(&package.version.to_string());
    let osv_eco = package.ecosystem.osv_name();
    vulns
        .iter()
        .filter(|v| {
            v.affected.iter().any(|a| {
                a.package.as_ref().is_some_and(|p| {
                    p.ecosystem == osv_eco && p.name.eq_ignore_ascii_case(&package.name)
                }) && ref_affected(&kind, a)
            })
        })
        .map(|v| v.id.clone())
        .collect()
}

//------------------------------------------------------------------------------
// querying

/// Fetch all advisories for a package name (no version), following pagination. Any failure is an error.
fn query_osv_package(
    client: &Arc<dyn UreqClient>,
    name: &str,
    ecosystem: Ecosystem,
) -> Result<Vec<OSVVulnRecord>, String> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..OSV_MAX_PAGES {
        let mut body = serde_json::json!({
            "package": {"name": name, "ecosystem": ecosystem.osv_name()}
        });
        if let Some(t) = &token {
            body["page_token"] = JsonValue::String(t.clone());
        }
        let text = client
            .post(OSV_QUERY_URL, &body.to_string())
            .map_err(|e| format!("OSV query failed for {name}: {e}"))?;
        let res: OSVQueryResponse = serde_json::from_str(&text)
            .map_err(|e| format!("OSV response could not be parsed for {name}: {e}"))?;
        out.extend(res.vulns);
        match res.next_page_token.filter(|t| !t.is_empty()) {
            Some(t) => token = Some(t),
            None => return Ok(out),
        }
    }
    Err(format!(
        "OSV pagination exceeded {OSV_MAX_PAGES} pages for {name}"
    ))
}

/// Fetch advisories for a package name, using a time-limited cache. Failures are not cached.
fn query_osv_package_cached(
    client: &Arc<dyn UreqClient>,
    name: &str,
    ecosystem: Ecosystem,
    cache_config: &CacheConfig,
    log: FlagLog,
) -> Result<Vec<OSVVulnRecord>, String> {
    if cache_config.duration == DURATION_0 {
        return query_osv_package(client, name, ecosystem);
    }
    let key = hash_string(&format!("{}\n{}", ecosystem.osv_name(), name));
    let cache_fp = cache_config
        .directory
        .join(format!("osv_package_{key}"))
        .with_extension("json");
    if path_within_duration(&cache_fp, cache_config.duration) {
        if let Ok(content) = fs::read_to_string(&cache_fp) {
            if let Ok(vulns) = serde_json::from_str::<Vec<OSVVulnRecord>>(&content) {
                logger!(
                    log,
                    module_path!(),
                    "Loaded OSV package cache: {:?}",
                    cache_fp
                );
                return Ok(vulns);
            }
        }
    }
    let vulns = query_osv_package(client, name, ecosystem)?;
    if let Ok(json) = serde_json::to_string(&vulns) {
        let _ = fs::write(&cache_fp, json);
    }
    Ok(vulns)
}

/// For packages of client-side-matched ecosystems, return vulnerability IDs per package (None if none), aligned with `packages`. Each distinct package name is queried once. All packages must satisfy `is_client_side_queryable`.
pub(crate) fn query_osv_client_side(
    client: Arc<dyn UreqClient>,
    packages: &[Package],
    cache_config: &CacheConfig,
    log: FlagLog,
) -> ResultDynError<Vec<Option<Vec<String>>>> {
    let mut names: Vec<(String, Ecosystem)> = packages
        .iter()
        .map(|p| (p.name.to_lowercase(), p.ecosystem))
        .collect();
    names.sort();
    names.dedup();
    let fetched: Result<HashMap<(String, Ecosystem), Vec<OSVVulnRecord>>, String> = names
        .into_par_iter()
        .map(|(name, eco)| {
            query_osv_package_cached(&client, &name, eco, cache_config, log)
                .map(|v| ((name, eco), v))
        })
        .collect();
    let fetched = fetched?;
    Ok(packages
        .iter()
        .map(|p| {
            let vulns = &fetched[&(p.name.to_lowercase(), p.ecosystem)];
            let ids = affecting_ids(p, vulns);
            if ids.is_empty() {
                None
            } else {
                Some(ids)
            }
        })
        .collect())
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ureq_client::UreqClientMock;
    use crate::util::path_cache;
    use crate::version_spec::VersionSpec;

    fn sv(s: &str) -> SemverLike {
        SemverLike::parse(s).unwrap()
    }

    fn affected(json: &str) -> OSVAffected {
        serde_json::from_str(json).unwrap()
    }

    fn gha(name: &str, version: &str) -> Package {
        Package::new(Ecosystem::Gha, name, VersionSpec::new(version), None)
    }

    #[test]
    fn test_semver_like_a() {
        assert_eq!(sv("v4"), SemverLike::new(4, 0, 0));
        assert_eq!(sv("4.1"), SemverLike::new(4, 1, 0));
        assert_eq!(sv("46.0.1"), SemverLike::new(46, 0, 1));
        assert!(sv("1.0.0-rc.1") < sv("1.0.0"));
        assert!(sv("3.28.2") < sv("3.28.3"));
        assert!(sv("41") > sv("35.0.0"));
        assert!(SemverLike::parse("main").is_none());
        assert!(SemverLike::parse("1.2.3.4").is_none());
        assert!(SemverLike::parse("").is_none());
    }

    #[test]
    fn test_classify_a() {
        assert_eq!(RefKind::classify("v4.1.0"), RefKind::Exact(sv("4.1.0")));
        assert_eq!(RefKind::classify("4.1.0"), RefKind::Exact(sv("4.1.0")));
        assert_eq!(
            RefKind::classify("v4"),
            RefKind::Floating {
                low: sv("4.0.0"),
                high: sv("5.0.0")
            }
        );
        assert_eq!(
            RefKind::classify("v4.1"),
            RefKind::Floating {
                low: sv("4.1.0"),
                high: sv("4.2.0")
            }
        );
        assert_eq!(RefKind::classify("main"), RefKind::Branch);
        assert_eq!(RefKind::classify("release/v1"), RefKind::Branch);
        assert_eq!(
            RefKind::classify("b4ffde65f46336ab88eb53be808477a3936bae11"),
            RefKind::Unresolvable
        );
        assert_eq!(RefKind::classify(""), RefKind::Unresolvable);
    }

    // GHSA-mrrh-fwg8-r2c3 (tj-actions/changed-files): fixed in 46.0.1
    const TJ: &str = r#"{"package": {"name": "tj-actions/changed-files", "ecosystem": "GitHub Actions"},
        "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "0"}, {"fixed": "46.0.1"}]}],
        "database_specific": {"last_known_affected_version_range": "<= 45.0.7"}}"#;

    // GHSA-vqf5-2xx6-9wfm (github/codeql-action): fixed in 3.28.3 for v3; v2 unfixed but bounded below 3.0.0
    const CODEQL_V3: &str = r#"{"package": {"name": "github/codeql-action", "ecosystem": "GitHub Actions"},
        "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "3.26.11"}, {"fixed": "3.28.3"}]}],
        "database_specific": {"last_known_affected_version_range": "<= 3.28.2"}}"#;
    const CODEQL_V2: &str = r#"{"package": {"name": "github/codeql-action", "ecosystem": "GitHub Actions"},
        "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "2.26.11"}]}],
        "database_specific": {"last_known_affected_version_range": "< 3.0.0"}}"#;

    #[test]
    fn test_exact_a() {
        let tj = affected(TJ);
        assert!(ref_affected(&RefKind::classify("v35.0.0"), &tj));
        assert!(ref_affected(&RefKind::classify("v46.0.0"), &tj));
        assert!(!ref_affected(&RefKind::classify("v46.0.1"), &tj));
        assert!(!ref_affected(&RefKind::classify("v47.0.0"), &tj));
    }

    #[test]
    fn test_floating_a() {
        let tj = affected(TJ);
        // the fix is in a later major: the newest v35 and v45 are affected
        assert!(ref_affected(&RefKind::classify("v35"), &tj));
        assert!(ref_affected(&RefKind::classify("v45"), &tj));
        // the fix is within v46: the newest v46 is fixed
        assert!(!ref_affected(&RefKind::classify("v46"), &tj));
        assert!(!ref_affected(&RefKind::classify("v47"), &tj));
        // minor lines
        assert!(!ref_affected(&RefKind::classify("v46.0"), &tj));
        let codeql = affected(CODEQL_V3);
        assert!(!ref_affected(&RefKind::classify("v3"), &codeql));
        assert!(ref_affected(&RefKind::classify("v3.27"), &codeql));
        assert!(!ref_affected(&RefKind::classify("v3.28"), &codeql));
    }

    #[test]
    fn test_open_range_bounded_a() {
        // the open v2 range is bounded by last_known_affected_version_range: v3 is not affected by it
        let v2 = affected(CODEQL_V2);
        assert!(ref_affected(&RefKind::classify("v2.26.11"), &v2));
        assert!(ref_affected(&RefKind::classify("v2"), &v2));
        assert!(!ref_affected(&RefKind::classify("v3.29.0"), &v2));
        assert!(!ref_affected(&RefKind::classify("v3"), &v2));
        // a branch is affected only by an unbounded (unfixed) range
        assert!(!ref_affected(&RefKind::Branch, &v2));
    }

    #[test]
    fn test_branch_a() {
        let open = affected(
            r#"{"package": {"name": "o/r", "ecosystem": "GitHub Actions"},
            "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "1.0.0"}]}]}"#,
        );
        assert!(ref_affected(&RefKind::Branch, &open));
        assert!(ref_affected(&RefKind::classify("v7"), &open));
        assert!(!ref_affected(&RefKind::Branch, &affected(TJ)));
    }

    #[test]
    fn test_last_affected_and_versions_a() {
        let a = affected(
            r#"{"package": {"name": "o/r", "ecosystem": "GitHub Actions"},
            "ranges": [{"type": "ECOSYSTEM", "events": [{"introduced": "2.0.0"}, {"last_affected": "2.3.0"}]},
                       {"type": "GIT", "events": [{"introduced": "abc"}, {"fixed": "def"}]}],
            "versions": ["1.5.0"]}"#,
        );
        assert!(ref_affected(&RefKind::classify("v2.3.0"), &a));
        assert!(!ref_affected(&RefKind::classify("v2.3.1"), &a));
        // listed version
        assert!(ref_affected(&RefKind::classify("v1.5.0"), &a));
        // floating: no fix, last affected within the line
        assert!(ref_affected(&RefKind::classify("v2"), &a));
        assert!(!ref_affected(&RefKind::classify("v3"), &a));
        assert!(!ref_affected(&RefKind::Unresolvable, &a));
    }

    #[test]
    fn test_affecting_ids_a() {
        let vulns: Vec<OSVVulnRecord> = serde_json::from_str(&format!(
            r#"[{{"id": "GHSA-mrrh", "affected": [{TJ}]}}, {{"id": "OTHER", "affected": [{{"package": {{"name": "x/y", "ecosystem": "GitHub Actions"}}, "ranges": [{{"type": "ECOSYSTEM", "events": [{{"introduced": "0"}}]}}]}}]}}]"#
        ))
        .unwrap();
        // name match is case-insensitive; other packages' entries are ignored
        assert_eq!(
            affecting_ids(&gha("TJ-Actions/changed-files", "v35"), &vulns),
            vec!["GHSA-mrrh"]
        );
        assert!(
            affecting_ids(&gha("tj-actions/changed-files", "v46"), &vulns).is_empty()
        );
    }

    #[test]
    fn test_query_client_side_a() {
        let mut mock = HashMap::new();
        mock.insert(
            "https://api.osv.dev/v1/query".to_string(),
            format!(r#"{{"vulns": [{{"id": "GHSA-mrrh", "affected": [{TJ}]}}]}}"#),
        );
        let client: Arc<dyn UreqClient> = Arc::new(UreqClientMock {
            mock_post: Some(mock),
            mock_get: None,
        });
        let packages = vec![
            gha("tj-actions/changed-files", "v35"),
            gha("tj-actions/changed-files", "v46.0.1"),
            gha("tj-actions/changed-files", "main"),
        ];
        let cache_config = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let r = query_osv_client_side(client, &packages, &cache_config, FlagLog(false))
            .unwrap();
        assert_eq!(r, vec![Some(vec!["GHSA-mrrh".to_string()]), None, None]);
    }

    #[test]
    fn test_version_from_tags_a() {
        let t = |v: &[&str]| {
            version_from_tags(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        // a full version over floating tags on the same commit
        assert_eq!(t(&["v1", "v1.12.0"]), Some("v1.12.0".to_string()));
        assert_eq!(
            t(&["v4", "v4.1", "v4.1.1", "latest"]),
            Some("v4.1.1".to_string())
        );
        // only floating tags: the most specific
        assert_eq!(t(&["v4", "v4.1"]), Some("v4.1".to_string()));
        assert_eq!(t(&["v14"]), Some("v14".to_string()));
        // several full versions: the highest
        assert_eq!(t(&["v2.0.0", "v2.0.1"]), Some("v2.0.1".to_string()));
        assert_eq!(t(&["latest", "nightly"]), None);
        assert_eq!(t(&[]), None);
    }

    /// A GitHub Actions SHA pin (no version comment), as `eco::gha` reads it.
    fn pin(name: &str, sha: &str) -> Package {
        Package::new(
            Ecosystem::Gha,
            name,
            VersionSpec::new(sha),
            Some(crate::package_durl::DirectURL {
                url: format!("https://github.com/{name}"),
                vcs_info: Some(crate::package_durl::VcsInfo {
                    commit_id: sha.to_string(),
                    vcs: "git".to_string(),
                    revision: None,
                }),
            }),
        )
    }

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const SHA_C: &str = "cccccccccccccccccccccccccccccccccccccccc";

    #[test]
    fn test_resolve_sha_pins_a() {
        let mut get = HashMap::new();
        // SHA_A: an annotated v35.0.0 (the peeled line names the commit) and a lightweight v35; SHA_B: not tagged
        get.insert(
            "git+https://github.com/tj-actions/changed-files".to_string(),
            format!(
                "1111111111111111111111111111111111111111\trefs/tags/v35.0.0\n{SHA_A}\trefs/tags/v35.0.0^{{}}\n{SHA_A}\trefs/tags/v35\n{SHA_C}\trefs/tags/v46.0.1\n"
            ),
        );
        let client: Arc<dyn UreqClient> = Arc::new(UreqClientMock {
            mock_post: None,
            mock_get: Some(get),
        });
        let packages = vec![
            pin("tj-actions/changed-files", SHA_A),
            pin("tj-actions/changed-files", SHA_B),
            // a repository that cannot be listed
            pin("acme/missing", SHA_A),
            // a Swift revision pin on a tagged commit
            Package::new(
                Ecosystem::Swift,
                "github.com/tj-actions/changed-files",
                VersionSpec::new(SHA_A),
                None,
            ),
            gha("actions/checkout", "v4"),
        ];
        let cache_config = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let check = check_sha_pins(&client, &packages, &cache_config, FlagLog(false));
        let (out, resolved) = (check.packages, check.resolved);
        assert!(check.mismatches.is_empty());
        assert_eq!(out[0].version.to_string(), "v35.0.0");
        let vcs = out[0]
            .direct_url
            .as_ref()
            .unwrap()
            .vcs_info
            .as_ref()
            .unwrap();
        assert_eq!(vcs.commit_id, SHA_A);
        assert_eq!(vcs.revision.as_deref(), Some("v35.0.0"));
        // unresolved pins and other packages are unchanged
        assert_eq!(out[1], packages[1]);
        assert_eq!(out[2], packages[2]);
        assert_eq!(out[3].version.to_string(), "v35.0.0");
        assert_eq!(out[3].ecosystem, Ecosystem::Swift);
        assert_eq!(out[4], packages[4]);
        let pin_of = |name: &str| ResolvedPin {
            name: name.to_string(),
            commit: SHA_A.to_string(),
            version: "v35.0.0".to_string(),
        };
        assert_eq!(
            resolved,
            vec![
                pin_of("github.com/tj-actions/changed-files"),
                pin_of("tj-actions/changed-files")
            ]
        );
        // the resolved pin is audited as v35.0.0
        assert!(matches!(
            RefKind::classify(&out[0].version.to_string()),
            RefKind::Exact(_)
        ));
        assert_eq!(
            RefKind::classify(&out[1].version.to_string()),
            RefKind::Unresolvable
        );
    }

    /// A SHA pin with a version comment, as `eco::gha` reads it: the comment is the version.
    fn commented_pin(name: &str, sha: &str, comment: &str) -> Package {
        let mut p = pin(name, sha);
        if let Some(v) = p.direct_url.as_mut().and_then(|d| d.vcs_info.as_mut()) {
            v.revision = Some(comment.to_string());
        }
        Package::new(
            Ecosystem::Gha,
            name,
            VersionSpec::new(comment),
            p.direct_url,
        )
    }

    #[test]
    fn test_check_sha_pins_mismatch_a() {
        let mut get = HashMap::new();
        get.insert(
            "git+https://github.com/tj-actions/changed-files".to_string(),
            format!("{SHA_A}\trefs/tags/v35.0.0\n{SHA_C}\trefs/tags/v46.0.1\n"),
        );
        let client: Arc<dyn UreqClient> = Arc::new(UreqClientMock {
            mock_post: None,
            mock_get: Some(get),
        });
        let name = "tj-actions/changed-files";
        let packages = vec![
            // consistent, ignoring the `v`
            commented_pin(name, SHA_A, "35.0.0"),
            // the comment's tag is on another commit; the commit is really v46.0.1
            commented_pin(name, SHA_C, "v35.0.0"),
            // no such tag, and the commit is untagged
            commented_pin(name, SHA_B, "v99.0.0"),
            // a repository that cannot be listed: audited by the comment, unchecked
            commented_pin("acme/missing", SHA_A, "v1.0.0"),
        ];
        let cache_config = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let check = check_sha_pins(&client, &packages, &cache_config, FlagLog(false));
        assert!(check.resolved.is_empty());
        assert_eq!(check.packages[0], packages[0]);
        assert_eq!(check.packages[3], packages[3]);
        // audited as what the commit actually is
        assert_eq!(check.packages[1].version.to_string(), "v46.0.1");
        let vcs = check.packages[1]
            .direct_url
            .as_ref()
            .unwrap()
            .vcs_info
            .as_ref()
            .unwrap();
        assert_eq!(vcs.revision.as_deref(), Some("v46.0.1"));
        // an untagged commit: unaudited
        assert_eq!(check.packages[2].version.to_string(), SHA_B);
        assert_eq!(
            RefKind::classify(&check.packages[2].version.to_string()),
            RefKind::Unresolvable
        );
        assert_eq!(
            check.mismatches,
            vec![
                PinMismatch {
                    name: name.to_string(),
                    commit: SHA_B.to_string(),
                    comment: "v99.0.0".to_string(),
                    actual: None,
                    tag_exists: false,
                },
                PinMismatch {
                    name: name.to_string(),
                    commit: SHA_C.to_string(),
                    comment: "v35.0.0".to_string(),
                    actual: Some("v46.0.1".to_string()),
                    tag_exists: true,
                },
            ]
        );
    }

    #[test]
    fn test_query_client_side_errors_a() {
        let client: Arc<dyn UreqClient> = Arc::new(UreqClientMock {
            mock_post: None,
            mock_get: None,
        });
        let cache_config = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let packages = vec![gha("o/r", "v1")];
        assert!(
            query_osv_client_side(client, &packages, &cache_config, FlagLog(false))
                .is_err()
        );
    }

    #[test]
    #[ignore] // hits the live OSV API
    fn test_query_client_side_live() {
        let client: Arc<dyn UreqClient> = Arc::new(crate::ureq_client::UreqClientLive);
        let cache_config = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let packages = vec![
            gha("tj-actions/changed-files", "v35.0.0"),
            gha("tj-actions/changed-files", "v46"),
            gha("actions/download-artifact", "v4.0.0"),
            gha("actions/download-artifact", "v4"),
            gha("github/codeql-action", "v3"),
        ];
        let r = query_osv_client_side(client, &packages, &cache_config, FlagLog(false))
            .unwrap();
        assert!(r[0]
            .as_ref()
            .unwrap()
            .contains(&"GHSA-mrrh-fwg8-r2c3".to_string()));
        assert_eq!(r[1], None);
        assert!(r[2]
            .as_ref()
            .unwrap()
            .contains(&"GHSA-cxww-7g56-2vh6".to_string()));
        assert_eq!(r[3], None);
        assert_eq!(r[4], None);
    }
}
