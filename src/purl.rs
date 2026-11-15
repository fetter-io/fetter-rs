//! Package URLs (purl, https://github.com/package-url/purl-spec): parsing and building per the specification, and mapping between purls and fetter's packages.
//!
//! | fetter | purl type | name mapping |
//! |---|---|---|
//! | python | `pypi` | name (normalized: lowercase, runs of `-_.` as `-`) |
//! | npm | `npm` | `@scope/name` is namespace `@scope`, name `name` |
//! | cargo | `cargo` | name |
//! | go | `golang` | module path: namespace is all but the last segment |
//! | ruby | `gem` | name |
//! | php | `composer` | `vendor/name` |
//! | swift | `swift` | `github.com/owner/repo`: namespace `github.com/owner` |
//! | vscode | `vscode-extension` | `publisher.name` |
//! | gha | `github` | `owner/repo` |
//! | precommit | `github` | `github.com/owner/repo` (hook repositories on GitHub only) |
//!
//! A `pkg:github` purl names a repository, which may be used as a GitHub Action or a pre-commit hook, so as input it maps to both.

use std::collections::BTreeMap;
use std::fmt;

use crate::ecosystem::Ecosystem;
use crate::package::Package;

//------------------------------------------------------------------------------
// percent-encoding

/// Characters left unencoded in purl components: ASCII letters and digits, `.-_~`, and `:` (which the specification allows anywhere after the scheme).
fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'~' | b':')
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> Result<String, String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s
                .get(i + 1..i + 3)
                .filter(|h| h.chars().all(|c| c.is_ascii_hexdigit()))
                .ok_or_else(|| format!("invalid percent-encoding in {s:?}"))?;
            out.push(u8::from_str_radix(hex, 16).map_err(|e| e.to_string())?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|e| e.to_string())
}

//------------------------------------------------------------------------------
/// A package URL: `pkg:type/namespace/name@version?qualifiers#subpath`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Purl {
    pub ty: String,
    pub namespace: Option<String>,
    pub name: String,
    pub version: Option<String>,
    pub qualifiers: BTreeMap<String, String>,
    pub subpath: Option<String>,
}

fn valid_type(t: &str) -> bool {
    !t.is_empty()
        && !t.starts_with(|c: char| c.is_ascii_digit())
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
}

fn valid_qualifier_key(k: &str) -> bool {
    !k.is_empty()
        && !k.starts_with(|c: char| c.is_ascii_digit())
        && k.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_')
        })
}

/// PEP 503 normalization, as the `pypi` type requires.
fn pypi_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut sep = false;
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            sep = true;
        } else {
            if sep && !out.is_empty() {
                out.push('-');
            }
            sep = false;
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// Clean a subpath: no leading or trailing `/`, and no empty, `.` or `..` segments.
fn clean_subpath(segments: impl Iterator<Item = String>) -> Option<String> {
    let parts: Vec<String> = segments
        .filter(|s| !s.is_empty() && s != "." && s != "..")
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

impl Purl {
    /// Construct and validate, applying the type's normalization (the specification's "build").
    pub fn new(
        ty: &str,
        namespace: Option<&str>,
        name: &str,
        version: Option<&str>,
        qualifiers: BTreeMap<String, String>,
        subpath: Option<&str>,
    ) -> Result<Self, String> {
        let ty = ty.to_ascii_lowercase();
        if !valid_type(&ty) {
            return Err(format!("invalid purl type {ty:?}"));
        }
        if name.is_empty() {
            return Err("a purl name is required".to_string());
        }
        for k in qualifiers.keys() {
            if !valid_qualifier_key(k) {
                return Err(format!("invalid purl qualifier key {k:?}"));
            }
        }
        let mut namespace = namespace
            .map(|n| {
                n.split('/')
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .filter(|n| !n.is_empty());
        let mut name = name.to_string();
        // type-specific rules
        let ns_required = matches!(
            ty.as_str(),
            "golang" | "composer" | "swift" | "vscode-extension" | "github"
        );
        let ns_prohibited = matches!(ty.as_str(), "pypi" | "cargo" | "gem");
        if ns_required && namespace.is_none() {
            return Err(format!("a namespace is required for {ty} purls"));
        }
        if ns_prohibited && namespace.is_some() {
            return Err(format!("a namespace is not allowed for {ty} purls"));
        }
        match ty.as_str() {
            "pypi" => name = pypi_name(&name),
            "composer" | "vscode-extension" | "github" => {
                name = name.to_lowercase();
                namespace = namespace.map(|n| n.to_lowercase());
            }
            _ => {}
        }
        Ok(Purl {
            ty,
            namespace,
            name,
            version: version.filter(|v| !v.is_empty()).map(str::to_string),
            qualifiers: qualifiers
                .into_iter()
                .filter(|(_, v)| !v.is_empty())
                .collect(),
            subpath: subpath
                .and_then(|s| clean_subpath(s.split('/').map(str::to_string))),
        })
    }

    /// Parse a purl string per the specification.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (rest, subpath) = match s.rsplit_once('#') {
            Some((r, sp)) => (
                r,
                clean_subpath(sp.split('/').map(|seg| decode(seg).unwrap_or_default())),
            ),
            None => (s, None),
        };
        let (rest, qualifiers) = match rest.rsplit_once('?') {
            Some((r, q)) => {
                let mut map = BTreeMap::new();
                for pair in q.split('&').filter(|p| !p.is_empty()) {
                    let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                    if !valid_qualifier_key(k) {
                        return Err(format!("invalid purl qualifier key {k:?}"));
                    }
                    let v = decode(v)?;
                    if !v.is_empty() {
                        map.insert(k.to_string(), v);
                    }
                }
                (r, map)
            }
            None => (rest, BTreeMap::new()),
        };
        let (scheme, rest) = rest
            .split_once(':')
            .ok_or_else(|| format!("not a purl (no scheme): {s:?}"))?;
        if !scheme.eq_ignore_ascii_case("pkg") {
            return Err(format!("not a purl (scheme {scheme:?}): {s:?}"));
        }
        let rest = rest.trim_start_matches('/');
        let (ty, rest) = rest
            .split_once('/')
            .ok_or_else(|| format!("a purl type is required: {s:?}"))?;
        if !valid_type(ty) {
            return Err(format!("invalid purl type {ty:?}"));
        }
        // a version follows the last `@`; an `@` before the last `/` is an unencoded npm scope (`pkg:npm/@babel/core`)
        let (rest, version) = match rest.rsplit_once('@') {
            Some((r, v)) if !v.contains('/') => (r, Some(decode(v)?)),
            _ => (rest, None),
        };
        let (namespace, name) = match rest.rsplit_once('/') {
            Some((ns, n)) => {
                let segs: Result<Vec<String>, String> = ns
                    .split('/')
                    .filter(|seg| !seg.is_empty())
                    .map(decode)
                    .collect();
                let segs = segs?;
                ((!segs.is_empty()).then(|| segs.join("/")), decode(n)?)
            }
            None => (None, decode(rest)?),
        };
        Purl::new(
            ty,
            namespace.as_deref(),
            &name,
            version.as_deref(),
            qualifiers,
            subpath.as_deref(),
        )
    }
}

impl fmt::Display for Purl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pkg:{}/", self.ty)?;
        if let Some(ns) = &self.namespace {
            let segs: Vec<String> = ns.split('/').map(encode).collect();
            write!(f, "{}/", segs.join("/"))?;
        }
        write!(f, "{}", encode(&self.name))?;
        if let Some(v) = &self.version {
            write!(f, "@{}", encode(v))?;
        }
        if !self.qualifiers.is_empty() {
            let q: Vec<String> = self
                .qualifiers
                .iter()
                .map(|(k, v)| format!("{k}={}", encode(v)))
                .collect();
            write!(f, "?{}", q.join("&"))?;
        }
        if let Some(sp) = &self.subpath {
            let segs: Vec<String> = sp.split('/').map(encode).collect();
            write!(f, "#{}", segs.join("/"))?;
        }
        Ok(())
    }
}

//------------------------------------------------------------------------------
// mapping to and from fetter packages

/// Split `a/b/c` at its last `/` into `(Some("a/b"), "c")`.
fn split_last(path: &str) -> (Option<&str>, &str) {
    match path.rsplit_once('/') {
        Some((ns, n)) => (Some(ns), n),
        None => (None, path),
    }
}

/// The purl of a package, if its ecosystem has a purl type (pre-commit hooks only on GitHub; Swift registry identities have none).
pub fn package_purl(p: &Package) -> Option<Purl> {
    let version = p.version.to_string();
    let v = (!version.is_empty()).then_some(version.as_str());
    let none = BTreeMap::new;
    match p.ecosystem {
        Ecosystem::Python => Purl::new("pypi", None, &p.name, v, none(), None).ok(),
        Ecosystem::Npm => {
            let (ns, n) = split_last(&p.name);
            Purl::new("npm", ns, n, v, none(), None).ok()
        }
        Ecosystem::Cargo => Purl::new("cargo", None, &p.name, v, none(), None).ok(),
        Ecosystem::Go => {
            let (ns, n) = split_last(&p.name);
            Purl::new("golang", ns, n, v, none(), None).ok()
        }
        Ecosystem::Ruby => Purl::new("gem", None, &p.name, v, none(), None).ok(),
        Ecosystem::Php | Ecosystem::Gha => {
            let ty = if p.ecosystem == Ecosystem::Php {
                "composer"
            } else {
                "github"
            };
            let (ns, n) = split_last(&p.name);
            Purl::new(ty, ns, n, v, none(), None).ok()
        }
        Ecosystem::Swift => {
            let (ns, n) = split_last(&p.name);
            Purl::new("swift", ns, n, v, none(), None).ok()
        }
        Ecosystem::VsCode => {
            let (publisher, name) = p.name.split_once('.')?;
            Purl::new("vscode-extension", Some(publisher), name, v, none(), None).ok()
        }
        Ecosystem::PreCommit => {
            let owner_repo = p.name.strip_prefix("github.com/")?;
            let (ns, n) = split_last(owner_repo);
            Purl::new("github", ns, n, v, none(), None).ok()
        }
    }
}

/// The fetter packages a purl names, as `(ecosystem, name, version)`; empty for a purl type fetter does not map. A `pkg:github` purl names both a GitHub Action and a pre-commit hook repository.
pub fn purl_targets(purl: &Purl) -> Vec<(Ecosystem, String, Option<String>)> {
    let path = || match &purl.namespace {
        Some(ns) => format!("{ns}/{}", purl.name),
        None => purl.name.clone(),
    };
    let v = purl.version.clone();
    match purl.ty.as_str() {
        "pypi" => vec![(Ecosystem::Python, purl.name.clone(), v)],
        "npm" => vec![(Ecosystem::Npm, path(), v)],
        "cargo" => vec![(Ecosystem::Cargo, purl.name.clone(), v)],
        "golang" => vec![(Ecosystem::Go, path(), v)],
        "gem" => vec![(Ecosystem::Ruby, purl.name.clone(), v)],
        "composer" => vec![(Ecosystem::Php, path(), v)],
        "swift" => vec![(Ecosystem::Swift, path(), v)],
        "vscode-extension" => match &purl.namespace {
            Some(ns) => vec![(Ecosystem::VsCode, format!("{ns}.{}", purl.name), v)],
            None => Vec::new(),
        },
        "github" => vec![
            (Ecosystem::Gha, path(), v.clone()),
            (Ecosystem::PreCommit, format!("github.com/{}", path()), v),
        ],
        _ => Vec::new(),
    }
}

/// A requirement naming exactly `version` in an ecosystem's allow-list syntax (`=1.0.0` for Cargo, where a bare version is a caret requirement; `name==1.0.0` for Python), or any version if none.
pub fn exact_requirement(eco: Ecosystem, name: &str, version: Option<&str>) -> String {
    match (eco, version) {
        (Ecosystem::Python, Some(v)) => format!("{name}=={v}"),
        (Ecosystem::Python, None) => name.to_string(),
        (Ecosystem::Cargo, Some(v)) => format!("={v}"),
        (Ecosystem::Ruby, None) => ">= 0".to_string(),
        (_, Some(v)) => v.to_string(),
        (_, None) => "*".to_string(),
    }
}

/// True if a text looks like a purl list: every non-empty line that is not a `#` comment starts with `pkg:`, and there is at least one.
pub fn is_purl_list(content: &str) -> bool {
    let mut lines = content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .peekable();
    lines.peek().is_some() && lines.all(|l| l.starts_with("pkg:"))
}

/// The purls of an SBOM, and the number of its packages without one: CycloneDX JSON (`components[].purl`, nested components included) or SPDX 2.x JSON (`packages[].externalRefs[]` with `referenceType` `purl`). `None` if the JSON is neither.
pub fn sbom_purls(json: &serde_json::Value) -> Option<(Vec<String>, usize)> {
    fn components(v: &serde_json::Value, out: &mut Vec<String>, without: &mut usize) {
        for c in v
            .get("components")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
            match c.get("purl").and_then(|p| p.as_str()) {
                Some(p) => out.push(p.to_string()),
                None => *without += 1,
            }
            components(c, out, without);
        }
    }
    let mut out = Vec::new();
    let mut without = 0;
    if json.get("bomFormat").and_then(|f| f.as_str()) == Some("CycloneDX") {
        if let Some(meta) = json.get("metadata").and_then(|m| m.get("component")) {
            // the described component itself (the project) is not a dependency; its subcomponents are
            components(meta, &mut out, &mut without);
        }
        components(json, &mut out, &mut without);
        return Some((out, without));
    }
    if json.get("spdxVersion").is_some() {
        for p in json
            .get("packages")
            .and_then(|p| p.as_array())
            .into_iter()
            .flatten()
        {
            let before = out.len();
            for r in p
                .get("externalRefs")
                .and_then(|r| r.as_array())
                .into_iter()
                .flatten()
            {
                if r.get("referenceType").and_then(|t| t.as_str()) == Some("purl") {
                    if let Some(loc) = r.get("referenceLocator").and_then(|l| l.as_str())
                    {
                        out.push(loc.to_string());
                    }
                }
            }
            if out.len() == before {
                without += 1;
            }
        }
        return Some((out, without));
    }
    None
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::version_spec::VersionSpec;
    use serde_json::Value;

    const FIXTURES: [(&str, &str); 10] = [
        (
            "specification",
            include_str!("../tests/fixtures/purl-spec/specification-test.json"),
        ),
        (
            "pypi",
            include_str!("../tests/fixtures/purl-spec/pypi-test.json"),
        ),
        (
            "npm",
            include_str!("../tests/fixtures/purl-spec/npm-test.json"),
        ),
        (
            "cargo",
            include_str!("../tests/fixtures/purl-spec/cargo-test.json"),
        ),
        (
            "golang",
            include_str!("../tests/fixtures/purl-spec/golang-test.json"),
        ),
        (
            "gem",
            include_str!("../tests/fixtures/purl-spec/gem-test.json"),
        ),
        (
            "composer",
            include_str!("../tests/fixtures/purl-spec/composer-test.json"),
        ),
        (
            "swift",
            include_str!("../tests/fixtures/purl-spec/swift-test.json"),
        ),
        (
            "vscode-extension",
            include_str!("../tests/fixtures/purl-spec/vscode-extension-test.json"),
        ),
        (
            "github",
            include_str!("../tests/fixtures/purl-spec/github-test.json"),
        ),
    ];

    fn opt(v: &Value) -> Option<String> {
        v.as_str().map(str::to_string)
    }

    /// A purl's components, as the test data lists them.
    type Parts = (
        String,
        Option<String>,
        String,
        Option<String>,
        BTreeMap<String, String>,
        Option<String>,
    );

    fn components(p: &Purl) -> Parts {
        (
            p.ty.clone(),
            p.namespace.clone(),
            p.name.clone(),
            p.version.clone(),
            p.qualifiers.clone(),
            p.subpath.clone(),
        )
    }

    fn expected(o: &Value) -> Parts {
        let q = o
            .get("qualifiers")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        (
            o["type"].as_str().unwrap_or_default().to_string(),
            opt(&o["namespace"]),
            o["name"].as_str().unwrap_or_default().to_string(),
            opt(&o["version"]),
            q,
            opt(&o["subpath"]),
        )
    }

    fn build(o: &Value) -> Result<Purl, String> {
        let q: BTreeMap<String, String> = o
            .get("qualifiers")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let ty = o["type"].as_str().ok_or("a purl type is required")?;
        let name = o["name"].as_str().ok_or("a purl name is required")?;
        Purl::new(
            ty,
            o["namespace"].as_str(),
            name,
            o["version"].as_str(),
            q,
            o["subpath"].as_str(),
        )
    }

    /// Recommended cases that contradict a required case: the required `gem` parse test rejects an uppercase qualifier key, which this recommended round-trip normalizes.
    const SKIPPED: [&str; 1] = ["pkg:gem/jruby-launcher@1.1.2?Platform=java"];

    #[test]
    fn test_purl_spec_suite_a() {
        let mut run = 0;
        for (file, content) in FIXTURES {
            let doc: Value = serde_json::from_str(content).unwrap();
            for t in doc["tests"].as_array().unwrap() {
                let input = &t["input"];
                if input.as_str().is_some_and(|s| SKIPPED.contains(&s))
                    && t["test_group"] == "recommended"
                {
                    continue;
                }
                // upstream errata: two pypi cases carry their message in the `subpath` field ("Applied normalization_rules ..."); the build case then expects no subpath. The validate case for the same purl is consistent and runs.
                let subpath_errata = |v: &Value| {
                    v.get("subpath")
                        .and_then(Value::as_str)
                        .is_some_and(|s| s.starts_with("Applied normalization_rules"))
                };
                if subpath_errata(input) || subpath_errata(&t["expected_output"]) {
                    continue;
                }
                let fail = t["expected_failure"].as_bool().unwrap_or(false);
                let desc = format!("{file}: {} {input}", t["description"]);
                match t["test_type"].as_str().unwrap() {
                    "parse" => {
                        let r = Purl::parse(input.as_str().unwrap());
                        if fail {
                            assert!(r.is_err(), "{desc}: expected failure, got {r:?}");
                        } else {
                            let p = r.unwrap_or_else(|e| panic!("{desc}: {e}"));
                            assert_eq!(
                                components(&p),
                                expected(&t["expected_output"]),
                                "{desc}"
                            );
                        }
                    }
                    "build" => {
                        let r = build(input);
                        if fail {
                            assert!(r.is_err(), "{desc}: expected failure, got {r:?}");
                        } else {
                            let p = r.unwrap_or_else(|e| panic!("{desc}: {e}"));
                            assert_eq!(
                                p.to_string(),
                                t["expected_output"].as_str().unwrap(),
                                "{desc}"
                            );
                        }
                    }
                    "validate" => {
                        let r = Purl::parse(input.as_str().unwrap());
                        if fail {
                            assert!(r.is_err(), "{desc}");
                        } else {
                            let p = r.unwrap_or_else(|e| panic!("{desc}: {e}"));
                            assert_eq!(
                                p.to_string(),
                                t["expected_output"].as_str().unwrap(),
                                "{desc}"
                            );
                        }
                    }
                    other => panic!("unknown test type {other}"),
                }
                run += 1;
            }
        }
        assert_eq!(run, 139);
    }

    fn pkg(eco: Ecosystem, name: &str, v: &str) -> Package {
        Package::new(eco, name, VersionSpec::new(v), None)
    }

    #[test]
    fn test_package_purl_round_trip_a() {
        let cases = [
            (
                pkg(Ecosystem::Python, "Django_Package", "4.2.1"),
                "pkg:pypi/django-package@4.2.1",
            ),
            (
                pkg(Ecosystem::Npm, "@babel/core", "7.24.0"),
                "pkg:npm/%40babel/core@7.24.0",
            ),
            (
                pkg(Ecosystem::Npm, "lodash", "4.17.21"),
                "pkg:npm/lodash@4.17.21",
            ),
            (
                pkg(Ecosystem::Cargo, "serde", "1.0.200"),
                "pkg:cargo/serde@1.0.200",
            ),
            (
                pkg(Ecosystem::Go, "golang.org/x/net", "v0.17.0"),
                "pkg:golang/golang.org/x/net@v0.17.0",
            ),
            (
                pkg(Ecosystem::Ruby, "nokogiri", "1.16.5"),
                "pkg:gem/nokogiri@1.16.5",
            ),
            (
                pkg(Ecosystem::Php, "symfony/http-kernel", "v5.4.0"),
                "pkg:composer/symfony/http-kernel@v5.4.0",
            ),
            (
                pkg(
                    Ecosystem::Swift,
                    "github.com/sparkle-project/Sparkle",
                    "2.8.1",
                ),
                "pkg:swift/github.com/sparkle-project/Sparkle@2.8.1",
            ),
            (
                pkg(Ecosystem::VsCode, "ms-python.python", "2024.1.0"),
                "pkg:vscode-extension/ms-python/python@2024.1.0",
            ),
            (
                pkg(Ecosystem::Gha, "actions/checkout", "v4"),
                "pkg:github/actions/checkout@v4",
            ),
            (
                pkg(Ecosystem::PreCommit, "github.com/psf/black", "24.1.0"),
                "pkg:github/psf/black@24.1.0",
            ),
        ];
        for (p, expected) in &cases {
            let purl = package_purl(p).unwrap_or_else(|| panic!("{p:?}"));
            assert_eq!(purl.to_string(), *expected);
            // and back: the purl names this package (keys compare per ecosystem)
            let targets = purl_targets(&Purl::parse(expected).unwrap());
            let (eco, name, version) =
                targets.iter().find(|t| t.0 == p.ecosystem).unwrap();
            assert_eq!(eco.name_to_key(name), p.key, "{expected}");
            assert_eq!(version.as_deref(), Some(p.version.to_string().as_str()));
        }
        // no purl: a pre-commit hook not on GitHub, a Swift registry identity
        assert!(
            package_purl(&pkg(Ecosystem::PreCommit, "gitlab.com/a/b", "v1")).is_none()
        );
        assert!(
            package_purl(&pkg(Ecosystem::Swift, "mona.linkedlist", "1.0.0")).is_none()
        );
        // a github purl names both a GitHub Action and a pre-commit hook
        let t = purl_targets(&Purl::parse("pkg:github/psf/black@24.1.0").unwrap());
        assert_eq!(t.len(), 2);
        assert!(purl_targets(&Purl::parse("pkg:maven/org.a/b@1.0").unwrap()).is_empty());
    }

    #[test]
    fn test_exact_requirement_a() {
        assert_eq!(
            exact_requirement(Ecosystem::Python, "django", Some("4.2.1")),
            "django==4.2.1"
        );
        assert_eq!(
            exact_requirement(Ecosystem::Cargo, "serde", Some("1.0.0")),
            "=1.0.0"
        );
        assert_eq!(
            exact_requirement(Ecosystem::Npm, "x", Some("1.0.0")),
            "1.0.0"
        );
        assert_eq!(exact_requirement(Ecosystem::Npm, "x", None), "*");
        assert_eq!(exact_requirement(Ecosystem::Ruby, "x", None), ">= 0");
    }

    #[test]
    fn test_is_purl_list_a() {
        assert!(is_purl_list(
            "# from syft\npkg:npm/a@1.0.0\n\npkg:pypi/b@2.0\n"
        ));
        assert!(!is_purl_list("requests==2.0\n"));
        assert!(!is_purl_list("pkg:npm/a@1\nrequests\n"));
        assert!(!is_purl_list("# only a comment\n"));
    }

    #[test]
    fn test_sbom_purls_a() {
        let cdx: Value = serde_json::from_str(
            r#"{"bomFormat": "CycloneDX", "specVersion": "1.5",
                "metadata": {"component": {"name": "app", "purl": "pkg:npm/app@1.0.0",
                    "components": [{"name": "inner", "purl": "pkg:npm/inner@0.1.0"}]}},
                "components": [
                    {"name": "lodash", "purl": "pkg:npm/lodash@4.17.15"},
                    {"name": "no-purl"},
                    {"name": "outer", "purl": "pkg:pypi/django@4.2.1",
                     "components": [{"name": "nested", "purl": "pkg:cargo/serde@1.0.0"}]}
                ]}"#,
        )
        .unwrap();
        let (mut got, without) = sbom_purls(&cdx).unwrap();
        got.sort();
        // `no-purl`, and the described component (not counted: it is the project) is excluded
        assert_eq!(without, 1);
        assert_eq!(
            got,
            vec![
                "pkg:cargo/serde@1.0.0",
                "pkg:npm/inner@0.1.0",
                "pkg:npm/lodash@4.17.15",
                "pkg:pypi/django@4.2.1"
            ]
        );
        let spdx: Value = serde_json::from_str(
            r#"{"spdxVersion": "SPDX-2.3", "packages": [
                {"name": "lodash", "externalRefs": [
                    {"referenceCategory": "PACKAGE-MANAGER", "referenceType": "purl", "referenceLocator": "pkg:npm/lodash@4.17.15"},
                    {"referenceCategory": "SECURITY", "referenceType": "cpe23Type", "referenceLocator": "cpe:2.3:a:lodash:lodash:4.17.15"}]},
                {"name": "none"}]}"#,
        )
        .unwrap();
        assert_eq!(
            sbom_purls(&spdx).unwrap(),
            (vec!["pkg:npm/lodash@4.17.15".to_string()], 1)
        );
        assert!(sbom_purls(&serde_json::json!({"packages": []})).is_none());
    }
}
