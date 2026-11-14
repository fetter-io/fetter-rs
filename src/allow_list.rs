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
use crate::ecosystem::Ecosystem;
use crate::package::Package;
use crate::util::Anchor;
use crate::util::FlagLog;
use crate::util::ResultDynError;

const ANY: &str = "*";

//------------------------------------------------------------------------------
#[derive(Debug, Clone)]
enum Requirement {
    /// npm and VS Code: npm range semantics (a bare `1.2.3` is exact); any of the ranges.
    NpmRange(Vec<nodejs_semver::Range>),
    /// Cargo: cargo semantics (a bare `1.2.3` is `^1.2.3`); any of the requirements.
    CargoReq(Vec<semver::VersionReq>),
    /// GitHub Actions: allowed refs, compared as written (or by pinned SHA); None is any.
    GhaRefs(Option<Vec<String>>),
}

/// One requirement for one package of a non-Python ecosystem: one or more specs, satisfied if any is (in a `fetter.toml`, a string or an array of strings). Arrays let any ecosystem express alternatives, e.g. a pre-release outside a range (`[">=7.18.7", "8.0.0-rc.1"]`), which cargo syntax cannot express in one requirement.
#[derive(Debug, Clone)]
pub struct ReqEntry {
    pub(crate) ecosystem: Ecosystem,
    pub(crate) name: String,
    /// The specs as written (for display and output).
    pub(crate) specs: Vec<String>,
    req: Requirement,
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
            Ecosystem::Gha => {
                Requirement::GhaRefs(if specs.iter().any(|s| s.trim() == ANY) {
                    None
                } else {
                    Some(specs.iter().map(|r| r.trim().to_string()).collect())
                })
            }
            Ecosystem::Python => {
                return Err("Python requirements are PEP 508 lines".into())
            }
        };
        Ok(ReqEntry {
            ecosystem,
            name: name.to_string(),
            specs,
            req,
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
fn is_fetter_toml(table: &toml::Table) -> bool {
    !table.is_empty()
        && table.iter().all(|(k, v)| {
            v.is_table() && Ecosystem::ALL.iter().any(|e| e.cli_name() == k)
        })
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
        for (key, value) in &table {
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
                al.python = Some(DepManifest::try_from_iter(reqs)?);
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
        Ok(al)
    }

    /// Load a bound: a `fetter.toml` (any `.toml` file with only ecosystem tables), or any Python bound source.
    pub(crate) fn from_bound(
        bound: &Path,
        bound_options: Option<&Vec<String>>,
        log: FlagLog,
    ) -> ResultDynError<Self> {
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
        if matches!(anchor, Anchor::Both) {
            return Err("Not implemented".into());
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
            let specs = if eco == Ecosystem::Gha {
                let mut refs: Vec<String> =
                    pkgs.iter().map(|p| p.version.to_string()).collect();
                refs.sort();
                refs.dedup();
                refs
            } else {
                // version order per semver
                let mut versions: Vec<(String, semver::Version)> = pkgs
                    .iter()
                    .filter_map(|p| {
                        let v = p.version.to_string();
                        semver::Version::parse(&v).ok().map(|sv| (v, sv))
                    })
                    .collect();
                versions.sort_by(|a, b| a.1.cmp(&b.1));
                versions.dedup_by(|a, b| a.1 == b.1);
                let base = match anchor {
                    Anchor::Lower => versions.first().map(|(v, _)| format!(">={v}")),
                    _ => versions.last().map(|(v, _)| format!("<={v}")),
                };
                let Some(base) = base else {
                    continue;
                };
                let mut specs = vec![base];
                // Observed versions the range excludes (pre-releases, which `>=`/`<=` ranges exclude in both npm and cargo semantics) are added as exact alternatives, so every observed package satisfies the derived requirement.
                if let Ok(entry) = ReqEntry::new(eco, &name, specs.clone()) {
                    for p in &pkgs {
                        if !entry.matches(p)
                            && semver::Version::parse(&p.version.to_string()).is_ok()
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
                    && (eco != Ecosystem::Gha || entry.specs[0] == ANY)
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
        assert!(AllowList::from_packages(&packages, Anchor::Both).is_err());
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
    fn test_toml_string_a() {
        assert_eq!(toml_string(r#"a "b" \c"#), r#""a \"b\" \\c""#);
        assert_eq!(
            toml_string("x; python_version < \"3.10\""),
            r#""x; python_version < \"3.10\"""#
        );
    }
}
