//! A cross-ecosystem allow list: the bound for `validate` / `purge-invalid`, and the output of `derive`.
//!
//! Read from a `fetter.toml` with one table per ecosystem, each in that ecosystem's native syntax:
//!
//! ```toml
//! [python]
//! requirements = ["requests>=2.31,<3", "numpy==2.1.*"]   # PEP 508 lines
//!
//! [npm]
//! "lodash" = "^4.17.21"                                  # npm range semantics
//!
//! [cargo]
//! "serde" = "1"                                          # cargo semantics: ^1
//!
//! [gha]
//! "actions/checkout" = ["v4", "v4.2.2"]                  # allowed refs (or "*")
//!
//! [vscode]
//! "ms-python.python" = "*"                               # npm range semantics
//! ```
//!
//! Any existing Python bound (requirements.txt, pyproject.toml, a lockfile, a URL, a git repo) is also accepted, as an allow list with only a Python section. An ecosystem without a section is not validated (unless `--strict`).

use std::borrow::Borrow;
use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;

use crate::dep_manifest::DepManifest;
use crate::eco::go::semver_of as go_semver;
use crate::eco::go::strip_version_v;
use crate::eco::php::is_branch_version;
use crate::eco::php::ComposerConstraint;
use crate::eco::php::ComposerVersion;
use crate::eco::ruby::GemRequirement;
use crate::eco::ruby::GemVersion;
use crate::ecosystem::Ecosystem;
use crate::osv_match::is_sha;
use crate::package::Package;
use crate::util::Anchor;
use crate::util::FlagLog;
use crate::util::ResultDynError;

const ANY: &str = "*";

//------------------------------------------------------------------------------
/// For `derive --anchor both`: the observed range from `lo` to `hi` (inclusive) in the ecosystem's syntax, or an exact version if they are the same.
fn both_spec(eco: Ecosystem, lo: &str, hi: &str) -> String {
    if lo == hi {
        return match eco {
            // a bare cargo version is a caret requirement
            Ecosystem::Cargo => format!("={lo}"),
            _ => lo.to_string(),
        };
    }
    match eco {
        // comma-separated constraints all hold
        Ecosystem::Cargo | Ecosystem::Ruby => format!(">={lo}, <={hi}"),
        // npm-style (npm, VS Code, Go, Swift) and composer: space-separated constraints all hold
        _ => format!(">={lo} <={hi}"),
    }
}

/// A version as ordered by its ecosystem, for `derive`. Only one variant occurs within an ecosystem.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum VersionKey {
    Semver(semver::Version),
    Gem(GemVersion),
    Composer(ComposerVersion),
}

/// The version's ordering key: semver for npm, Cargo and VS Code; semver without the `v` for Go; RubyGems order for Ruby.
fn version_key(eco: Ecosystem, v: &str) -> Option<VersionKey> {
    match eco {
        Ecosystem::Go => go_semver(v).map(VersionKey::Semver),
        Ecosystem::Ruby => GemVersion::parse(v).map(VersionKey::Gem),
        Ecosystem::Php => ComposerVersion::parse(v).map(VersionKey::Composer),
        _ => semver::Version::parse(v).ok().map(VersionKey::Semver),
    }
}

//------------------------------------------------------------------------------
#[derive(Debug, Clone)]
enum Requirement {
    /// npm and VS Code: npm range semantics (a bare `1.2.3` is exact); any of the ranges.
    NpmRange(Vec<nodejs_semver::Range>),
    /// Cargo: cargo semantics (a bare `1.2.3` is `^1.2.3`); any of the requirements.
    CargoReq(Vec<semver::VersionReq>),
    /// GitHub Actions: allowed refs, compared as written (or by pinned SHA); None is any.
    GhaRefs(Option<Vec<String>>),
    /// Go: npm range semantics over versions with the `v` removed (`">=v1.2.3 <v2"`); any of the ranges.
    GoRange(Vec<nodejs_semver::Range>),
    /// Ruby: RubyGems requirements (`"~> 7.1"`, `">= 1.0, < 2"`; a bare version is exact); any of them.
    RubyReq(Vec<GemRequirement>),
    /// PHP: composer constraints (`"^5.4"`, `"~1.2"`, `">=1.0 <2.0"`, `"dev-main"`); any of them.
    PhpReq(Vec<ComposerConstraint>),
    /// Swift: npm-style ranges, and revision SHAs (for branch and revision pins) matched exactly; any of them.
    SwiftReq(Vec<nodejs_semver::Range>, Vec<String>),
}

/// One requirement for one package of a non-Python ecosystem: one or more specs, satisfied if any is (in a `fetter.toml`, a string or an array of strings). Arrays let any ecosystem express alternatives, e.g. a pre-release outside a range (`[">=7.18.7", "8.0.0-rc.1"]`), which cargo syntax cannot express in one requirement.
#[derive(Debug, Clone)]
pub struct ReqEntry {
    pub(crate) ecosystem: Ecosystem,
    pub(crate) name: String,
    /// The specs as written (for display and output).
    pub(crate) specs: Vec<String>,
    req: Requirement,
    /// An optional entry allows matching packages but is not reported missing when none is found: one created only from a `pkg:github` purl, which names a repository used as a GitHub Action, a pre-commit hook, or neither.
    pub(crate) optional: bool,
}

impl PartialEq for ReqEntry {
    fn eq(&self, other: &Self) -> bool {
        (self.ecosystem, &self.name, &self.specs)
            == (other.ecosystem, &other.name, &other.specs)
    }
}

impl ReqEntry {
    pub(crate) fn new(
        ecosystem: Ecosystem,
        name: &str,
        specs: Vec<String>,
    ) -> ResultDynError<Self> {
        if specs.is_empty() {
            return Err(format!("Empty {ecosystem} requirement for {name}").into());
        }
        let err = |spec: &str, e: &dyn fmt::Display| -> Box<dyn std::error::Error> {
            format!("Invalid {ecosystem} requirement for {name}: {spec:?}: {e}").into()
        };
        let req = match ecosystem {
            Ecosystem::Npm | Ecosystem::VsCode => Requirement::NpmRange(
                specs
                    .iter()
                    .map(|s| nodejs_semver::Range::parse(s).map_err(|e| err(s, &e)))
                    .collect::<Result<_, _>>()?,
            ),
            Ecosystem::Cargo => Requirement::CargoReq(
                specs
                    .iter()
                    .map(|s| semver::VersionReq::parse(s).map_err(|e| err(s, &e)))
                    .collect::<Result<_, _>>()?,
            ),
            Ecosystem::Gha | Ecosystem::PreCommit => {
                Requirement::GhaRefs(if specs.iter().any(|s| s.trim() == ANY) {
                    None
                } else {
                    Some(specs.iter().map(|r| r.trim().to_string()).collect())
                })
            }
            Ecosystem::Go => Requirement::GoRange(
                specs
                    .iter()
                    .map(|s| {
                        nodejs_semver::Range::parse(strip_version_v(s))
                            .map_err(|e| err(s, &e))
                    })
                    .collect::<Result<_, _>>()?,
            ),
            Ecosystem::Ruby => Requirement::RubyReq(
                specs
                    .iter()
                    .map(|s| GemRequirement::parse(s).map_err(|e| err(s, &e)))
                    .collect::<Result<_, _>>()?,
            ),
            Ecosystem::Swift => {
                let (shas, ranges): (Vec<&String>, Vec<&String>) =
                    specs.iter().partition(|s| is_sha(s.trim()));
                Requirement::SwiftReq(
                    ranges
                        .iter()
                        .map(|s| nodejs_semver::Range::parse(s).map_err(|e| err(s, &e)))
                        .collect::<Result<_, _>>()?,
                    shas.iter().map(|s| s.trim().to_lowercase()).collect(),
                )
            }
            Ecosystem::Php => Requirement::PhpReq(
                specs
                    .iter()
                    .map(|s| ComposerConstraint::parse(s).map_err(|e| err(s, &e)))
                    .collect::<Result<_, _>>()?,
            ),
            Ecosystem::Python => {
                return Err("Python requirements are PEP 508 lines".into())
            }
        };
        Ok(ReqEntry {
            ecosystem,
            name: name.to_string(),
            specs,
            req,
            optional: false,
        })
    }

    /// Does the package satisfy this requirement (any of its specs)? A version that cannot be parsed in the ecosystem's syntax does not.
    pub(crate) fn matches(&self, package: &Package) -> bool {
        let version = package.version.to_string();
        match &self.req {
            Requirement::NpmRange(ranges) => nodejs_semver::Version::parse(&version)
                .is_ok_and(|v| ranges.iter().any(|r| v.satisfies(r))),
            Requirement::CargoReq(reqs) => semver::Version::parse(&version)
                .is_ok_and(|v| reqs.iter().any(|r| r.matches(&v))),
            Requirement::GoRange(ranges) => {
                version.starts_with('v')
                    && nodejs_semver::Version::parse(strip_version_v(&version))
                        .is_ok_and(|v| ranges.iter().any(|r| v.satisfies(r)))
            }
            Requirement::RubyReq(reqs) => GemVersion::parse(&version)
                .is_some_and(|v| reqs.iter().any(|r| r.matches(&v))),
            Requirement::PhpReq(cs) => cs.iter().any(|c| c.matches(&version)),
            Requirement::SwiftReq(ranges, shas) => {
                shas.contains(&version.to_lowercase())
                    || nodejs_semver::Version::parse(&version)
                        .is_ok_and(|v| ranges.iter().any(|r| v.satisfies(r)))
            }
            Requirement::GhaRefs(None) => true,
            Requirement::GhaRefs(Some(refs)) => {
                // a SHA pin matches by its comment version (the package version) or by the SHA itself
                let sha = package
                    .direct_url
                    .as_ref()
                    .and_then(|d| d.vcs_info.as_ref())
                    .map(|v| v.commit_id.as_str());
                refs.iter()
                    .any(|r| *r == version || Some(r.as_str()) == sha)
            }
        }
    }
}

impl fmt::Display for ReqEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.name, self.specs.join(" | "))
    }
}

/// The requirements of one non-Python ecosystem, keyed by normalized package key.
#[derive(Debug, Clone, Default)]
pub(crate) struct EcoSection {
    entries: BTreeMap<String, ReqEntry>,
}

impl EcoSection {
    pub(crate) fn get(&self, key: &str) -> Option<&ReqEntry> {
        self.entries.get(key)
    }

    /// Entries sorted by key.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &ReqEntry)> {
        self.entries.iter()
    }

    fn insert(&mut self, entry: ReqEntry) {
        self.entries
            .insert(entry.ecosystem.name_to_key(&entry.name), entry);
    }
}

//------------------------------------------------------------------------------
#[derive(Debug, Clone, Default)]
pub struct AllowList {
    pub(crate) python: Option<DepManifest>,
    pub(crate) sections: BTreeMap<Ecosystem, EcoSection>,
}

/// A `fetter.toml` has only ecosystem tables at the top level (a `pyproject.toml` does not).
/// The top-level key of a `fetter.toml` listing purls.
const PURLS: &str = "purls";

fn is_fetter_toml(table: &toml::Table) -> bool {
    !table.is_empty()
        && table.iter().all(|(k, v)| {
            (v.is_table() && Ecosystem::ALL.iter().any(|e| e.cli_name() == k))
                || (k == PURLS && v.is_array())
        })
}

/// Requirements from purls, by ecosystem: Python requirement lines, and `(ecosystem, name) -> specs` for the others. A purl with a version allows exactly that version; without, any. A `pkg:github` purl applies to both GitHub Actions and pre-commit. Purls of types fetter does not map are returned separately.
#[allow(clippy::type_complexity)]
fn purl_requirements(
    purls: &[String],
) -> ResultDynError<(
    Vec<String>,
    BTreeMap<(Ecosystem, String), (String, Vec<String>)>,
    Vec<String>,
)> {
    let mut python = Vec::new();
    let mut others: BTreeMap<(Ecosystem, String), (String, Vec<String>)> =
        BTreeMap::new();
    let mut unsupported = Vec::new();
    for s in purls {
        let purl = crate::purl::Purl::parse(s)
            .map_err(|e| format!("Invalid purl {s:?}: {e}"))?;
        let targets = crate::purl::purl_targets(&purl);
        if targets.is_empty() {
            unsupported.push(s.clone());
        }
        for (eco, name, version) in targets {
            let req = crate::purl::exact_requirement(eco, &name, version.as_deref());
            if eco == Ecosystem::Python {
                python.push(req);
            } else {
                let e = others
                    .entry((eco, eco.name_to_key(&name)))
                    .or_insert_with(|| (name.clone(), Vec::new()));
                if !e.1.contains(&req) {
                    e.1.push(req);
                }
            }
        }
    }
    Ok((python, others, unsupported))
}

impl AllowList {
    pub(crate) fn from_python(dm: DepManifest) -> Self {
        AllowList {
            python: Some(dm),
            sections: BTreeMap::new(),
        }
    }

    /// Parse a `fetter.toml`.
    pub(crate) fn from_fetter_toml(content: &str) -> ResultDynError<Self> {
        let table: toml::Table = toml::from_str(content)?;
        if !is_fetter_toml(&table) {
            return Err("Not a fetter.toml: expected only ecosystem tables ([python], [npm], [cargo], [gha], [vscode])".into());
        }
        let mut al = AllowList::default();
        let purls: Vec<String> = match table.get(PURLS) {
            Some(v) => v
                .as_array()
                .ok_or("purls must be an array of strings")?
                .iter()
                .map(|p| {
                    p.as_str()
                        .map(str::to_string)
                        .ok_or("purls must be strings")
                })
                .collect::<Result<_, _>>()?,
            None => Vec::new(),
        };
        let (purl_python, purl_others, unsupported) = purl_requirements(&purls)?;
        if !unsupported.is_empty() {
            eprintln!(
                "fetter: {} purl(s) of unsupported types ignored: {}",
                unsupported.len(),
                unsupported.join(", ")
            );
        }
        let mut python_reqs: Option<Vec<String>> =
            (!purl_python.is_empty()).then(|| purl_python.clone());
        for (key, value) in &table {
            if key == PURLS {
                continue;
            }
            let eco = *Ecosystem::ALL
                .iter()
                .find(|e| e.cli_name() == key)
                .ok_or("unknown ecosystem")?;
            let t = value.as_table().ok_or("expected a table")?;
            if eco == Ecosystem::Python {
                let reqs = match t.get("requirements") {
                    Some(v) => v
                        .as_array()
                        .ok_or("[python] requirements must be an array of strings")?
                        .iter()
                        .map(|r| {
                            r.as_str()
                                .map(str::to_string)
                                .ok_or("[python] requirements must be strings")
                        })
                        .collect::<Result<Vec<String>, _>>()?,
                    None => Vec::new(),
                };
                if let Some(k) = t.keys().find(|k| *k != "requirements") {
                    return Err(format!("Unknown key in [python]: {k}").into());
                }
                python_reqs.get_or_insert_with(Vec::new).extend(reqs);
                continue;
            }
            let mut section = EcoSection::default();
            for (name, v) in t {
                let specs = match v {
                    toml::Value::String(s) => vec![s.clone()],
                    // any of several specs (or, for GHA, allowed refs)
                    toml::Value::Array(a) => a
                        .iter()
                        .map(|r| {
                            r.as_str().map(str::to_string).ok_or_else(|| {
                                format!("[{eco}] requirements for {name} must be strings")
                            })
                        })
                        .collect::<Result<Vec<String>, _>>()?,
                    _ => {
                        return Err(format!(
                            "[{eco}] requirement for {name} must be a string or an array of strings"
                        )
                        .into())
                    }
                };
                section.insert(ReqEntry::new(eco, name, specs)?);
            }
            al.sections.insert(eco, section);
        }
        if let Some(reqs) = python_reqs {
            al.python = Some(DepManifest::try_from_iter(reqs)?);
        }
        // purls add alternatives to (or create) entries
        for ((eco, key), (name, specs)) in purl_others {
            let section = al.sections.entry(eco).or_default();
            // a `pkg:github` purl applies to both GitHub Actions and pre-commit; an entry it alone creates is optional
            let optional = matches!(eco, Ecosystem::Gha | Ecosystem::PreCommit)
                && section.get(&key).is_none_or(|e| e.optional);
            let mut all = section
                .get(&key)
                .map(|e| e.specs.clone())
                .unwrap_or_default();
            for s in specs {
                if !all.contains(&s) {
                    all.push(s);
                }
            }
            let name = section.get(&key).map(|e| e.name.clone()).unwrap_or(name);
            let mut entry = ReqEntry::new(eco, &name, all)?;
            entry.optional = optional;
            section.insert(entry);
        }
        Ok(al)
    }

    /// An allow list from purls (from an SBOM or a purl list); see `purl_requirements`.
    pub(crate) fn from_purls(purls: &[String]) -> ResultDynError<Self> {
        let mut t = toml::Table::new();
        t.insert(
            PURLS.to_string(),
            toml::Value::Array(
                purls
                    .iter()
                    .map(|p| toml::Value::String(p.clone()))
                    .collect(),
            ),
        );
        Self::from_fetter_toml(&toml::to_string(&t)?)
    }

    /// Load a bound: an SBOM (CycloneDX or SPDX 2.x JSON) or a list of purls (one per line), a `fetter.toml` (any `.toml` file with only ecosystem tables and `purls`), or any Python bound source.
    pub(crate) fn from_bound(
        bound: &Path,
        bound_options: Option<&Vec<String>>,
        log: FlagLog,
    ) -> ResultDynError<Self> {
        // an SBOM (CycloneDX or SPDX JSON) or a purl list
        if bound.is_file() {
            let is_json = bound.extension().is_some_and(|e| e == "json");
            if let Ok(content) = fs::read_to_string(bound) {
                if is_json {
                    if let Some((purls, without)) =
                        serde_json::from_str::<serde_json::Value>(&content)
                            .ok()
                            .and_then(|v| crate::purl::sbom_purls(&v))
                    {
                        if without > 0 {
                            eprintln!(
                                "fetter: {without} SBOM package(s) have no purl and are ignored"
                            );
                        }
                        return Self::from_purls(&purls);
                    }
                } else if crate::purl::is_purl_list(&content) {
                    let purls: Vec<String> = content
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty() && !l.starts_with('#'))
                        .map(str::to_string)
                        .collect();
                    return Self::from_purls(&purls);
                }
            }
        }
        let is_toml = bound.extension().is_some_and(|e| e == "toml")
            && bound.file_name().is_some_and(|n| n != "pyproject.toml");
        if is_toml && bound.is_file() {
            let content = fs::read_to_string(bound)
                .map_err(|e| format!("Cannot read {}: {e}", bound.display()))?;
            if toml::from_str::<toml::Table>(&content).is_ok_and(|t| is_fetter_toml(&t)) {
                return Self::from_fetter_toml(&content);
            }
        }
        Ok(Self::from_python(DepManifest::from_path_or_url(
            bound,
            bound_options,
            log,
        )?))
    }

    /// Does the allow list have a section for this ecosystem?
    #[cfg(test)]
    pub(crate) fn has_section(&self, eco: Ecosystem) -> bool {
        match eco {
            Ecosystem::Python => self.python.is_some(),
            _ => self.sections.contains_key(&eco),
        }
    }

    pub(crate) fn section(&self, eco: Ecosystem) -> Option<&EcoSection> {
        self.sections.get(&eco)
    }

    /// True if only Python requirements are present (so a requirements file can represent it).
    pub(crate) fn is_python_only(&self) -> bool {
        self.sections.values().all(|s| s.entries.is_empty())
    }

    /// Derive an allow list from observed packages. Python uses `DepManifest::from_packages`. For npm, Cargo, and VS Code, `Lower` gives `>=` the lowest observed version and `Upper` gives `<=` the highest. GHA lists every observed ref (the anchor does not apply). Versions that cannot be expressed in the ecosystem's syntax are skipped.
    pub(crate) fn from_packages<I>(packages: I, anchor: Anchor) -> ResultDynError<Self>
    where
        I: IntoIterator,
        I::Item: Borrow<Package>,
    {
        let mut python: Vec<Package> = Vec::new();
        let mut by_eco_key: BTreeMap<(Ecosystem, String), Vec<Package>> = BTreeMap::new();
        for p in packages {
            let p: &Package = p.borrow();
            if p.ecosystem == Ecosystem::Python {
                python.push(p.clone());
            } else {
                by_eco_key
                    .entry((p.ecosystem, p.key.clone()))
                    .or_default()
                    .push(p.clone());
            }
        }
        let mut al = AllowList {
            python: (!python.is_empty())
                .then(|| DepManifest::from_packages(&python, anchor))
                .transpose()?,
            sections: BTreeMap::new(),
        };
        for ((eco, _), mut pkgs) in by_eco_key {
            pkgs.sort();
            let name = pkgs[0].name.clone();
            let specs = if matches!(eco, Ecosystem::Gha | Ecosystem::PreCommit) {
                let mut refs: Vec<String> =
                    pkgs.iter().map(|p| p.version.to_string()).collect();
                refs.sort();
                refs.dedup();
                refs
            } else {
                // version order per ecosystem
                let mut versions: Vec<(String, VersionKey)> = pkgs
                    .iter()
                    .filter_map(|p| {
                        let v = p.version.to_string();
                        version_key(eco, &v).map(|k| (v, k))
                    })
                    .collect();
                versions.sort_by(|a, b| a.1.cmp(&b.1));
                versions.dedup_by(|a, b| a.1 == b.1);
                let base = match anchor {
                    Anchor::Lower => versions.first().map(|(v, _)| format!(">={v}")),
                    Anchor::Upper => versions.last().map(|(v, _)| format!("<={v}")),
                    Anchor::Both => versions
                        .first()
                        .zip(versions.last())
                        .map(|((lo, _), (hi, _))| both_spec(eco, lo, hi)),
                };
                // unordered versions are each allowed as an exact alternative: PHP branch versions (`dev-main`) and Swift revision pins (a SHA)
                let mut branches: Vec<String> = pkgs
                    .iter()
                    .map(|p| p.version.to_string())
                    .filter(|v| match eco {
                        Ecosystem::Php => is_branch_version(v),
                        Ecosystem::Swift => is_sha(v),
                        _ => false,
                    })
                    .collect();
                branches.sort();
                branches.dedup();
                let mut specs = match base {
                    Some(b) => vec![b],
                    None if !branches.is_empty() => Vec::new(),
                    None => continue,
                };
                // Observed versions the range excludes (pre-releases, which `>=`/`<=` ranges exclude in npm and cargo semantics, though not in RubyGems' or composer's) are added as exact alternatives, so every observed package satisfies the derived requirement.
                if let Some(entry) = (!specs.is_empty())
                    .then(|| ReqEntry::new(eco, &name, specs.clone()).ok())
                    .flatten()
                {
                    for p in &pkgs {
                        if !entry.matches(p)
                            && version_key(eco, &p.version.to_string()).is_some()
                        {
                            let v = p.version.to_string();
                            specs.push(if eco == Ecosystem::Cargo {
                                format!("={v}")
                            } else {
                                v
                            });
                        }
                    }
                }
                specs.extend(branches);
                specs.dedup();
                specs
            };
            if let Ok(entry) = ReqEntry::new(eco, &name, specs) {
                al.sections.entry(eco).or_default().insert(entry);
            }
        }
        Ok(al)
    }

    /// Write as a `fetter.toml`, with ecosystems in canonical order and entries sorted.
    pub(crate) fn to_toml(&self) -> String {
        let mut out = String::from("# via fetter\n");
        if let Some(dm) = &self.python {
            out.push_str("\n[python]\nrequirements = [\n");
            for record in dm.to_dep_manifest_report().records {
                out.push_str(&format!(
                    "    {},\n",
                    toml_string(&record.dep_spec.to_string())
                ));
            }
            out.push_str("]\n");
        }
        for eco in Ecosystem::ALL {
            let Some(section) = self.sections.get(&eco).filter(|s| !s.entries.is_empty())
            else {
                continue;
            };
            out.push_str(&format!("\n[{eco}]\n"));
            for entry in section.entries.values() {
                // a single spec is a string (except GHA refs, always an array unless any); several are an array
                let value = if entry.specs.len() == 1
                    && (!matches!(eco, Ecosystem::Gha | Ecosystem::PreCommit)
                        || entry.specs[0] == ANY)
                {
                    toml_string(&entry.specs[0])
                } else {
                    let items: Vec<String> =
                        entry.specs.iter().map(|s| toml_string(s)).collect();
                    format!("[{}]", items.join(", "))
                };
                out.push_str(&format!("{} = {value}\n", toml_string(&entry.name)));
            }
        }
        out
    }

    /// Counts of requirements per ecosystem.
    #[cfg(test)]
    pub(crate) fn counts(&self) -> HashMap<Ecosystem, usize> {
        let mut m = HashMap::new();
        if let Some(dm) = &self.python {
            m.insert(Ecosystem::Python, dm.iter_dep_specs().count());
        }
        for (e, s) in &self.sections {
            m.insert(*e, s.entries.len());
        }
        m
    }
}

/// A TOML basic string.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::package_durl::DirectURL;
    use crate::package_durl::VcsInfo;
    use crate::version_spec::VersionSpec;

    fn pkg(eco: Ecosystem, name: &str, version: &str) -> Package {
        Package::new(eco, name, VersionSpec::new(version), None)
    }

    const TOML: &str = r#"
[python]
requirements = ["requests>=2.31,<3", "numpy==2.1.*"]

[npm]
"lodash" = "^4.17.21"
"@babel/core" = ">=7.24.0"
"left-pad" = "1.3.0"

[cargo]
"serde" = "1"
"serde_json" = "=1.0.117"

[gha]
"actions/checkout" = ["v4", "v4.2.2"]
"codecov/codecov-action" = "*"

[vscode]
"ms-python.python" = "*"
"CodeInKlingon.git-worktree-menu" = "<1.0.9"
"#;

    #[test]
    fn test_from_fetter_toml_a() {
        let al = AllowList::from_fetter_toml(TOML).unwrap();
        assert!(al.has_section(Ecosystem::Python));
        assert!(al.has_section(Ecosystem::Gha));
        assert!(!al.is_python_only());
        assert_eq!(al.counts()[&Ecosystem::Python], 2);
        assert_eq!(al.counts()[&Ecosystem::Npm], 3);
        // keys are normalized per ecosystem
        assert!(al
            .section(Ecosystem::VsCode)
            .unwrap()
            .get("codeinklingon.git-worktree-menu")
            .is_some());
        assert!(al
            .section(Ecosystem::Cargo)
            .unwrap()
            .get("serde_json")
            .is_some());
    }

    #[test]
    fn test_npm_semantics_a() {
        let al = AllowList::from_fetter_toml(TOML).unwrap();
        let npm = al.section(Ecosystem::Npm).unwrap();
        let m = |key: &str, name: &str, v: &str| {
            npm.get(key).unwrap().matches(&pkg(Ecosystem::Npm, name, v))
        };
        assert!(m("lodash", "lodash", "4.17.21"));
        assert!(m("lodash", "lodash", "4.99.0"));
        assert!(!m("lodash", "lodash", "5.0.0"));
        assert!(!m("lodash", "lodash", "4.17.15"));
        // a bare version is exact in npm
        assert!(m("left-pad", "left-pad", "1.3.0"));
        assert!(!m("left-pad", "left-pad", "1.3.1"));
        assert!(m("@babel/core", "@babel/core", "7.25.1"));
        // an unparseable version does not match
        assert!(!m("lodash", "lodash", "not-a-version"));
    }

    #[test]
    fn test_cargo_semantics_a() {
        let al = AllowList::from_fetter_toml(TOML).unwrap();
        let cargo = al.section(Ecosystem::Cargo).unwrap();
        let m = |key: &str, v: &str| {
            cargo
                .get(key)
                .unwrap()
                .matches(&pkg(Ecosystem::Cargo, key, v))
        };
        // a bare version is caret in cargo
        assert!(m("serde", "1.0.200"));
        assert!(m("serde", "1.9.0"));
        assert!(!m("serde", "2.0.0"));
        assert!(m("serde_json", "1.0.117"));
        assert!(!m("serde_json", "1.0.118"));
    }

    #[test]
    fn test_gha_and_vscode_a() {
        let al = AllowList::from_fetter_toml(TOML).unwrap();
        let gha = al.section(Ecosystem::Gha).unwrap();
        let co = gha.get("actions/checkout").unwrap();
        assert!(co.matches(&pkg(Ecosystem::Gha, "actions/checkout", "v4")));
        assert!(co.matches(&pkg(Ecosystem::Gha, "actions/checkout", "v4.2.2")));
        assert!(!co.matches(&pkg(Ecosystem::Gha, "actions/checkout", "v3")));
        assert!(!co.matches(&pkg(Ecosystem::Gha, "actions/checkout", "main")));
        // a SHA pin matches by its comment version or by its SHA
        let sha = "11bd71901bbe5b1630ceea73d27597364c9af683";
        let pinned = Package::new(
            Ecosystem::Gha,
            "actions/checkout",
            VersionSpec::new(sha),
            Some(DirectURL {
                url: "https://github.com/actions/checkout".to_string(),
                vcs_info: Some(VcsInfo {
                    commit_id: sha.to_string(),
                    vcs: "git".to_string(),
                    revision: None,
                }),
            }),
        );
        assert!(!co.matches(&pinned));
        let al2 = AllowList::from_fetter_toml(&format!(
            "[gha]\n\"actions/checkout\" = [\"{sha}\"]\n"
        ))
        .unwrap();
        assert!(al2
            .section(Ecosystem::Gha)
            .unwrap()
            .get("actions/checkout")
            .unwrap()
            .matches(&pinned));
        assert!(gha.get("codecov/codecov-action").unwrap().matches(&pkg(
            Ecosystem::Gha,
            "codecov/codecov-action",
            "main"
        )));
        let vs = al.section(Ecosystem::VsCode).unwrap();
        assert!(vs.get("ms-python.python").unwrap().matches(&pkg(
            Ecosystem::VsCode,
            "ms-python.python",
            "2024.1.0"
        )));
        assert!(!vs
            .get("codeinklingon.git-worktree-menu")
            .unwrap()
            .matches(&pkg(
                Ecosystem::VsCode,
                "CodeInKlingon.git-worktree-menu",
                "1.0.9"
            )));
    }

    #[test]
    fn test_from_fetter_toml_errors_a() {
        assert!(AllowList::from_fetter_toml("[project]\nname = \"x\"\n").is_err());
        assert!(
            AllowList::from_fetter_toml("[npm]\n\"lodash\" = \"not a range!!\"\n")
                .is_err()
        );
        assert!(AllowList::from_fetter_toml("[cargo]\n\"serde\" = 1\n").is_err());
        assert!(AllowList::from_fetter_toml("[python]\nrequirements = \"x\"\n").is_err());
        assert!(AllowList::from_fetter_toml("[python]\nother = []\n").is_err());
        let err = AllowList::from_fetter_toml("[npm]\n\"lodash\" = \"^^^\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("lodash"), "{err}");
    }

    #[test]
    fn test_from_bound_a() {
        let dir = tempfile::tempdir().unwrap();
        let ft = dir.path().join("fetter.toml");
        fs::write(&ft, TOML).unwrap();
        let al = AllowList::from_bound(&ft, None, FlagLog(false)).unwrap();
        assert!(al.has_section(Ecosystem::Npm));
        // a requirements file is a Python-only allow list
        let rq = dir.path().join("requirements.txt");
        fs::write(&rq, "numpy>=2\n").unwrap();
        let al = AllowList::from_bound(&rq, None, FlagLog(false)).unwrap();
        assert!(al.has_section(Ecosystem::Python));
        assert!(!al.has_section(Ecosystem::Npm));
        assert!(al.is_python_only());
        // a pyproject.toml is a Python bound, not a fetter.toml
        let pp = dir.path().join("pyproject.toml");
        fs::write(
            &pp,
            "[project]\nname = \"x\"\nversion = \"1\"\ndependencies = [\"numpy>=2\"]\n",
        )
        .unwrap();
        let al = AllowList::from_bound(&pp, None, FlagLog(false)).unwrap();
        assert!(al.has_section(Ecosystem::Python));
    }

    #[test]
    fn test_from_packages_and_round_trip_a() {
        let packages = vec![
            pkg(Ecosystem::Python, "numpy", "2.1.0"),
            pkg(Ecosystem::Npm, "lodash", "4.17.21"),
            pkg(Ecosystem::Npm, "lodash", "4.17.15"),
            pkg(Ecosystem::Npm, "@babel/core", "7.24.0"),
            pkg(Ecosystem::Cargo, "serde", "1.0.200"),
            pkg(Ecosystem::Gha, "actions/checkout", "v4"),
            pkg(Ecosystem::Gha, "actions/checkout", "v4.2.2"),
            pkg(
                Ecosystem::VsCode,
                "CodeInKlingon.git-worktree-menu",
                "1.0.8",
            ),
        ];
        let al = AllowList::from_packages(&packages, Anchor::Lower).unwrap();
        let text = al.to_toml();
        assert_eq!(
            text,
            r#"# via fetter

[python]
requirements = [
    "numpy>=2.1.0",
]

[npm]
"@babel/core" = ">=7.24.0"
"lodash" = ">=4.17.15"

[cargo]
"serde" = ">=1.0.200"

[gha]
"actions/checkout" = ["v4", "v4.2.2"]

[vscode]
"CodeInKlingon.git-worktree-menu" = ">=1.0.8"
"#
        );
        // the output parses back, and every derived-from package satisfies it
        let back = AllowList::from_fetter_toml(&text).unwrap();
        for p in packages.iter().filter(|p| p.ecosystem != Ecosystem::Python) {
            let entry = back.section(p.ecosystem).unwrap().get(&p.key).unwrap();
            assert!(entry.matches(p), "{p}");
        }
        let upper = AllowList::from_packages(&packages, Anchor::Upper).unwrap();
        assert!(upper.to_toml().contains("\"lodash\" = \"<=4.17.21\""));
        let both = AllowList::from_packages(&packages, Anchor::Both)
            .unwrap()
            .to_toml();
        // several observed versions: the range; one: exact
        assert!(
            both.contains("\"lodash\" = \">=4.17.15 <=4.17.21\""),
            "{both}"
        );
        let back = AllowList::from_fetter_toml(&both).unwrap();
        for p in packages.iter().filter(|p| p.ecosystem != Ecosystem::Python) {
            let entry = back.section(p.ecosystem).unwrap().get(&p.key).unwrap();
            assert!(entry.matches(p), "{p} {entry}");
        }
    }

    #[test]
    fn test_derive_both_a() {
        let packages = vec![
            pkg(Ecosystem::Npm, "a", "1.0.0"),
            pkg(Ecosystem::Npm, "a", "1.2.0"),
            pkg(Ecosystem::Npm, "b", "2.0.0"),
            pkg(Ecosystem::Cargo, "c", "0.9.1"),
            pkg(Ecosystem::Cargo, "c", "0.10.0"),
            pkg(Ecosystem::Cargo, "d", "1.0.0"),
            pkg(Ecosystem::Ruby, "rails", "7.1.3.4"),
            pkg(Ecosystem::Ruby, "rails", "7.10.0"),
            pkg(Ecosystem::Php, "psr/log", "v1.1.4"),
            pkg(Ecosystem::Php, "psr/log", "3.0.0"),
            pkg(Ecosystem::Go, "golang.org/x/net", "v0.17.0"),
            pkg(Ecosystem::Go, "golang.org/x/net", "v0.30.0"),
            pkg(Ecosystem::Swift, "github.com/apple/swift-nio", "2.0.0"),
            // a pre-release outside the npm range is added as an exact alternative
            pkg(Ecosystem::Npm, "e", "1.0.0"),
            pkg(Ecosystem::Npm, "e", "2.0.0"),
            pkg(Ecosystem::Npm, "e", "1.5.0-rc.1"),
        ];
        let text = AllowList::from_packages(&packages, Anchor::Both)
            .unwrap()
            .to_toml();
        for expected in [
            "\"a\" = \">=1.0.0 <=1.2.0\"",
            "\"b\" = \"2.0.0\"",
            "\"c\" = \">=0.9.1, <=0.10.0\"",
            // a bare cargo version would be a caret requirement
            "\"d\" = \"=1.0.0\"",
            "\"rails\" = \">=7.1.3.4, <=7.10.0\"",
            "\"psr/log\" = \">=v1.1.4 <=3.0.0\"",
            "\"golang.org/x/net\" = \">=v0.17.0 <=v0.30.0\"",
            "\"github.com/apple/swift-nio\" = \"2.0.0\"",
            "\"e\" = [\">=1.0.0 <=2.0.0\", \"1.5.0-rc.1\"]",
        ] {
            assert!(text.contains(expected), "{expected} not in {text}");
        }
        let back = AllowList::from_fetter_toml(&text).unwrap();
        for p in &packages {
            let entry = back.section(p.ecosystem).unwrap().get(&p.key).unwrap();
            assert!(entry.matches(p), "{p} {entry}");
        }
        // a version outside the observed range does not satisfy it; a Swift revision pin is exact
        let swift = AllowList::from_packages(
            &[
                pkg(Ecosystem::Swift, "github.com/x/y", "1.0.0"),
                pkg(
                    Ecosystem::Swift,
                    "github.com/x/y",
                    "bace59f90bb276f1c6fb613acfda3935ec4a7a90",
                ),
            ],
            Anchor::Both,
        )
        .unwrap()
        .to_toml();
        assert!(
            swift.contains("\"github.com/x/y\" = [\"1.0.0\", \"bace59f90bb276f1c6fb613acfda3935ec4a7a90\"]"),
            "{swift}"
        );
        let sw = AllowList::from_fetter_toml(&swift).unwrap();
        let e = sw
            .section(Ecosystem::Swift)
            .unwrap()
            .get("github.com/x/y")
            .unwrap();
        assert!(e.matches(&pkg(
            Ecosystem::Swift,
            "github.com/x/y",
            "BACE59F90BB276F1C6FB613ACFDA3935EC4A7A90"
        )));
        assert!(!e.matches(&pkg(
            Ecosystem::Swift,
            "github.com/x/y",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )));
        let a = back.section(Ecosystem::Npm).unwrap().get("a").unwrap();
        assert!(!a.matches(&pkg(Ecosystem::Npm, "a", "1.3.0")));
    }

    #[test]
    fn test_any_of_a() {
        let al = AllowList::from_fetter_toml(
            "[npm]\n\"x\" = [\">=2.0.0\", \"1.5.0\"]\n\"y\" = \"^1.0.0 || ^3.0.0\"\n[cargo]\n\"z\" = [\">=1.2\", \"=2.0.0-rc.1\"]\n",
        )
        .unwrap();
        let npm = al.section(Ecosystem::Npm).unwrap();
        assert!(npm
            .get("x")
            .unwrap()
            .matches(&pkg(Ecosystem::Npm, "x", "1.5.0")));
        assert!(npm
            .get("x")
            .unwrap()
            .matches(&pkg(Ecosystem::Npm, "x", "2.1.0")));
        assert!(!npm
            .get("x")
            .unwrap()
            .matches(&pkg(Ecosystem::Npm, "x", "1.6.0")));
        // a native npm union in one string also works
        assert!(npm
            .get("y")
            .unwrap()
            .matches(&pkg(Ecosystem::Npm, "y", "3.2.0")));
        assert!(!npm
            .get("y")
            .unwrap()
            .matches(&pkg(Ecosystem::Npm, "y", "2.0.0")));
        let cargo = al.section(Ecosystem::Cargo).unwrap();
        assert!(cargo.get("z").unwrap().matches(&pkg(
            Ecosystem::Cargo,
            "z",
            "2.0.0-rc.1"
        )));
        assert_eq!(cargo.get("z").unwrap().to_string(), "z >=1.2 | =2.0.0-rc.1");
        assert!(AllowList::from_fetter_toml("[npm]\n\"x\" = []\n").is_err());
    }

    #[test]
    fn test_derive_prerelease_a() {
        // a range excludes pre-releases (npm and cargo semantics alike); derive adds them as exact alternatives
        let packages = vec![
            pkg(Ecosystem::Npm, "@babel/parser", "7.18.8"),
            pkg(Ecosystem::Npm, "@babel/parser", "8.0.0-rc.1"),
            pkg(Ecosystem::Cargo, "z", "1.2.0"),
            pkg(Ecosystem::Cargo, "z", "2.0.0-rc.1"),
        ];
        for anchor in [Anchor::Lower, Anchor::Upper] {
            let al = AllowList::from_packages(&packages, anchor).unwrap();
            let back = AllowList::from_fetter_toml(&al.to_toml()).unwrap();
            for p in &packages {
                let entry = back.section(p.ecosystem).unwrap().get(&p.key).unwrap();
                assert!(entry.matches(p), "{p} {entry}");
            }
        }
        let text = AllowList::from_packages(&packages, Anchor::Lower)
            .unwrap()
            .to_toml();
        assert!(
            text.contains("\"@babel/parser\" = [\">=7.18.8\", \"8.0.0-rc.1\"]"),
            "{text}"
        );
        assert!(
            text.contains("\"z\" = [\">=1.2.0\", \"=2.0.0-rc.1\"]"),
            "{text}"
        );
    }

    #[test]
    fn test_go_semantics_a() {
        let toml = r#"
[go]
"github.com/BurntSushi/toml" = ">=v1.3.0 <v2"
"golang.org/x/net" = ["v0.17.0", ">=v0.23.0"]
"#;
        let al = AllowList::from_fetter_toml(toml).unwrap();
        let go = al.section(Ecosystem::Go).unwrap();
        // module paths are case-sensitive keys
        assert!(go.get("github.com/burntsushi/toml").is_none());
        let toml_req = go.get("github.com/BurntSushi/toml").unwrap();
        let m = |e: &ReqEntry, v: &str| e.matches(&pkg(Ecosystem::Go, &e.name, v));
        assert!(m(toml_req, "v1.3.0"));
        assert!(m(toml_req, "v1.6.0"));
        assert!(!m(toml_req, "v1.2.1"));
        assert!(!m(toml_req, "v2.0.0+incompatible"));
        // a Go version always has its `v`
        assert!(!m(toml_req, "1.4.0"));
        let net = go.get("golang.org/x/net").unwrap();
        assert!(m(net, "v0.17.0"));
        assert!(!m(net, "v0.18.0"));
        assert!(m(net, "v0.30.0"));
        assert_eq!(net.to_string(), "golang.org/x/net v0.17.0 | >=v0.23.0");
    }

    #[test]
    fn test_ruby_semantics_and_derive_a() {
        let toml = r#"
[ruby]
"rails" = "~> 7.1"
"nokogiri" = [">= 1.16.5, < 2", "1.15.6"]
"CFPropertyList" = "*"
"#;
        let al = AllowList::from_fetter_toml(toml);
        // `*` is not a RubyGems requirement
        assert!(al.is_err());
        let toml = toml.replace("\"*\"", "\">= 0\"");
        let al = AllowList::from_fetter_toml(&toml).unwrap();
        let ruby = al.section(Ecosystem::Ruby).unwrap();
        let m = |key: &str, v: &str| {
            let e = ruby.get(key).unwrap();
            e.matches(&pkg(Ecosystem::Ruby, &e.name, v))
        };
        assert!(m("rails", "7.1.3.4"));
        assert!(m("rails", "7.2.0"));
        assert!(!m("rails", "8.0.0"));
        assert!(!m("rails", "7.0.8"));
        assert!(m("nokogiri", "1.16.5"));
        assert!(m("nokogiri", "1.15.6"));
        assert!(!m("nokogiri", "1.15.7"));
        // keys are case-sensitive
        assert!(ruby.get("cfpropertylist").is_none());
        assert!(m("CFPropertyList", "2.3.6"));

        let packages = vec![
            pkg(Ecosystem::Ruby, "rails", "7.1.3.4"),
            pkg(Ecosystem::Ruby, "rails", "7.10.0"),
            pkg(Ecosystem::Ruby, "rails", "8.0.0.rc1"),
        ];
        let lower = AllowList::from_packages(&packages, Anchor::Lower)
            .unwrap()
            .to_toml();
        // RubyGems order (7.10 > 7.1.3.4), and a range admits pre-releases, so no alternatives
        assert!(lower.contains("\"rails\" = \">=7.1.3.4\""), "{lower}");
        let upper = AllowList::from_packages(&packages, Anchor::Upper)
            .unwrap()
            .to_toml();
        assert!(upper.contains("\"rails\" = \"<=8.0.0.rc1\""), "{upper}");
        for text in [lower, upper] {
            let back = AllowList::from_fetter_toml(&text).unwrap();
            for p in &packages {
                let entry = back.section(p.ecosystem).unwrap().get(&p.key).unwrap();
                assert!(entry.matches(p), "{p} {entry}");
            }
        }
    }

    #[test]
    fn test_php_semantics_and_derive_a() {
        let toml = r#"
[php]
"symfony/http-kernel" = "^5.4.20 || ^6.2.6"
"Lox/XHProf" = "dev-master"
"guzzlehttp/guzzle" = "~7.4"
"#;
        let al = AllowList::from_fetter_toml(toml).unwrap();
        let php = al.section(Ecosystem::Php).unwrap();
        let m = |key: &str, v: &str| {
            let e = php.get(key).unwrap();
            e.matches(&pkg(Ecosystem::Php, &e.name, v))
        };
        assert!(m("symfony/http-kernel", "v5.4.21"));
        assert!(m("symfony/http-kernel", "v6.3.0"));
        assert!(!m("symfony/http-kernel", "v5.4.0"));
        // keys are lowercase
        assert!(m("lox/xhprof", "dev-master"));
        assert!(!m("lox/xhprof", "1.0.0"));
        // composer's ~7.4 is >=7.4 <8.0 (npm's would be <7.5)
        assert!(m("guzzlehttp/guzzle", "7.9.2"));
        assert!(!m("guzzlehttp/guzzle", "8.0.0"));

        let packages = vec![
            pkg(Ecosystem::Php, "psr/log", "1.1.4"),
            pkg(Ecosystem::Php, "psr/log", "v3.0.0"),
            pkg(Ecosystem::Php, "psr/log", "dev-main"),
            pkg(Ecosystem::Php, "lox/xhprof", "dev-master"),
        ];
        for anchor in [Anchor::Lower, Anchor::Upper] {
            let text = AllowList::from_packages(&packages, anchor)
                .unwrap()
                .to_toml();
            let back = AllowList::from_fetter_toml(&text).unwrap();
            for p in &packages {
                let entry = back.section(p.ecosystem).unwrap().get(&p.key).unwrap();
                assert!(entry.matches(p), "{p} {entry} {text}");
            }
        }
        let text = AllowList::from_packages(&packages, Anchor::Lower)
            .unwrap()
            .to_toml();
        assert!(
            text.contains("\"psr/log\" = [\">=1.1.4\", \"dev-main\"]"),
            "{text}"
        );
        assert!(text.contains("\"lox/xhprof\" = \"dev-master\""), "{text}");
    }

    #[test]
    fn test_swift_semantics_a() {
        let toml = "[swift]\n\"github.com/sparkle-project/Sparkle\" = \">=2.6.4 <3\"\n";
        let al = AllowList::from_fetter_toml(toml).unwrap();
        let swift = al.section(Ecosystem::Swift).unwrap();
        // keys are case-insensitive (GitHub URLs); names keep their case for OSV
        let e = swift.get("github.com/sparkle-project/sparkle").unwrap();
        assert_eq!(e.name, "github.com/sparkle-project/Sparkle");
        assert!(e.matches(&pkg(Ecosystem::Swift, &e.name, "2.8.1")));
        assert!(!e.matches(&pkg(Ecosystem::Swift, &e.name, "2.6.3")));
        assert!(!e.matches(&pkg(Ecosystem::Swift, &e.name, "3.0.0")));
    }

    #[test]
    fn test_purls_in_fetter_toml_a() {
        let toml = r#"
purls = [
    "pkg:npm/lodash@4.17.21",
    "pkg:npm/lodash@4.17.15",
    "pkg:npm/%40babel/core@7.24.0",
    "pkg:pypi/Django@4.2.1",
    "pkg:pypi/django@4.2.2",
    "pkg:cargo/serde@1.0.200",
    "pkg:github/actions/checkout@v4",
    "pkg:maven/org.a/b@1.0",
    "pkg:npm/left-pad",
]

[npm]
"lodash" = ">=5"
"#;
        let al = AllowList::from_fetter_toml(toml).unwrap();
        let npm = al.section(Ecosystem::Npm).unwrap();
        // a table entry and purls for the same package are alternatives
        let lodash = npm.get("lodash").unwrap();
        assert_eq!(lodash.specs, vec![">=5", "4.17.21", "4.17.15"]);
        assert!(lodash.matches(&pkg(Ecosystem::Npm, "lodash", "4.17.15")));
        assert!(!lodash.matches(&pkg(Ecosystem::Npm, "lodash", "4.17.16")));
        assert!(npm.get("@babel/core").is_some());
        // no version: any
        assert!(npm.get("left-pad").unwrap().matches(&pkg(
            Ecosystem::Npm,
            "left-pad",
            "9.9.9"
        )));
        // cargo pins are exact, not caret
        let serde = al.section(Ecosystem::Cargo).unwrap().get("serde").unwrap();
        assert!(!serde.matches(&pkg(Ecosystem::Cargo, "serde", "1.0.201")));
        // python: several versions are alternatives
        let py = al.python.as_ref().unwrap();
        assert_eq!(py.len(), 1);
        // github: both GitHub Actions and pre-commit, optional (not required to be present)
        assert!(
            al.section(Ecosystem::Gha)
                .unwrap()
                .get("actions/checkout")
                .unwrap()
                .optional
        );
        assert!(!lodash.optional);
        assert!(al
            .section(Ecosystem::PreCommit)
            .unwrap()
            .get("github.com/actions/checkout")
            .is_some());
        assert!(AllowList::from_fetter_toml("purls = [\"not a purl\"]\n").is_err());
    }

    #[test]
    fn test_from_bound_sbom_and_purl_list_a() {
        let dir = tempfile::tempdir().unwrap();
        let cdx = dir.path().join("bom.cdx.json");
        fs::write(
            &cdx,
            r#"{"bomFormat": "CycloneDX", "specVersion": "1.5", "components": [
                {"name": "lodash", "purl": "pkg:npm/lodash@4.17.15"},
                {"name": "jinja2", "purl": "pkg:pypi/jinja2@3.1.3"}]}"#,
        )
        .unwrap();
        let al = AllowList::from_bound(&cdx, None, FlagLog(false)).unwrap();
        assert!(al.section(Ecosystem::Npm).unwrap().get("lodash").is_some());
        assert_eq!(al.python.as_ref().unwrap().len(), 1);

        let spdx = dir.path().join("sbom.spdx.json");
        fs::write(
            &spdx,
            r#"{"spdxVersion": "SPDX-2.3", "packages": [{"name": "serde", "externalRefs": [
                {"referenceCategory": "PACKAGE-MANAGER", "referenceType": "purl", "referenceLocator": "pkg:cargo/serde@1.0.200"}]}]}"#,
        )
        .unwrap();
        let al = AllowList::from_bound(&spdx, None, FlagLog(false)).unwrap();
        assert!(al.section(Ecosystem::Cargo).unwrap().get("serde").is_some());
        assert!(al.python.is_none());

        let list = dir.path().join("purls.txt");
        fs::write(
            &list,
            "# derived\npkg:gem/nokogiri@1.16.5\n\npkg:golang/golang.org/x/net@v0.17.0\n",
        )
        .unwrap();
        let al = AllowList::from_bound(&list, None, FlagLog(false)).unwrap();
        assert!(al
            .section(Ecosystem::Ruby)
            .unwrap()
            .get("nokogiri")
            .is_some());
        assert!(al
            .section(Ecosystem::Go)
            .unwrap()
            .get("golang.org/x/net")
            .is_some());

        // a requirements file is still a requirements file
        let req = dir.path().join("requirements.txt");
        fs::write(&req, "requests==2.31.0\n").unwrap();
        let al = AllowList::from_bound(&req, None, FlagLog(false)).unwrap();
        assert!(al.is_python_only());
    }

    #[test]
    fn test_go_derive_a() {
        let packages = vec![
            pkg(Ecosystem::Go, "golang.org/x/net", "v0.17.0"),
            pkg(Ecosystem::Go, "golang.org/x/net", "v0.30.0"),
            pkg(
                Ecosystem::Go,
                "github.com/xiang90/probing",
                "v0.0.0-20221125231312-a49e3df8f510",
            ),
        ];
        for anchor in [Anchor::Lower, Anchor::Upper] {
            let al = AllowList::from_packages(&packages, anchor).unwrap();
            let text = al.to_toml();
            let back = AllowList::from_fetter_toml(&text).unwrap();
            for p in &packages {
                let entry = back.section(p.ecosystem).unwrap().get(&p.key).unwrap();
                assert!(entry.matches(p), "{p} {entry} {text}");
            }
        }
        let text = AllowList::from_packages(&packages, Anchor::Lower)
            .unwrap()
            .to_toml();
        assert!(text.contains("[go]"), "{text}");
        assert!(
            text.contains("\"golang.org/x/net\" = \">=v0.17.0\""),
            "{text}"
        );
    }

    #[test]
    fn test_toml_string_a() {
        assert_eq!(toml_string(r#"a "b" \c"#), r#""a \"b\" \\c""#);
        assert_eq!(
            toml_string("x; python_version < \"3.10\""),
            r#""x; python_version < \"3.10\"""#
        );
    }
}
