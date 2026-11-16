//! Version range specifiers (VERS, https://github.com/package-url/vers-spec): `vers:<type>/<constraints>`, a companion to purls for ranges of versions, e.g. `vers:npm/>=1.0.0|<2.0.0` or `vers:pypi/>=2.31|!=2.32.0|<3`.
//!
//! Parsing is strict, as the specification requires: input must already be canonical (lowercase scheme and type; no whitespace; no explicit `=`; canonical percent-encoding; constraints sorted by version, each version once, with valid comparator sequences), and anything else is an error rather than being normalized.
//!
//! A version is in a range if it is in any of the intervals the constraints define, comparing versions by the type's rules, which are fetter's own per-ecosystem orderings:
//!
//! | type | ecosystem | ordering |
//! |---|---|---|
//! | `pypi` | python | PEP 440 (as `VersionSpec`) |
//! | `npm`, `cargo`, `vscode-extension`, `swift`, `semver` | npm, cargo, vscode, swift | SemVer 2.0 |
//! | `golang` | go | SemVer 2.0 of `v`-prefixed versions |
//! | `gem` | ruby | RubyGems (`Gem::Version`) |
//! | `composer` | php | composer |
//! | `github` | gha, precommit | none: refs are matched exactly |
//! | `lexicographic` | (general) | byte order |
//! | `all`, `none` | (general) | only `*`: every version, or none |
//!
//! Two extensions for versions with no order: in a `github` range every version is an exact ref (only bare versions and `!=`), and in any other type a version that does not parse in the type's ordering (a Swift revision SHA, a composer `dev-` branch) may appear as a bare version or with `!=`, anywhere in the sequence, and is matched exactly. A range of only `!=` constraints contains every version but those (the reading the specification's examples give `vers:pypi/!=5`).

use std::fmt;

use crate::eco::go::semver_of as go_semver;
use crate::eco::php::ComposerVersion;
use crate::eco::ruby::GemVersion;
use crate::osv_match::is_sha;
use crate::version_spec::VersionSpec;

/// The vers types fetter can compare.
const TYPES: [&str; 13] = [
    "pypi",
    "chrome-extension",
    "npm",
    "cargo",
    "vscode-extension",
    "swift",
    "semver",
    "golang",
    "gem",
    "composer",
    "github",
    "lexicographic",
    "all",
];
/// The other general type, of the empty range.
const NONE: &str = "none";

/// Characters that are percent-encoded in a version (and the only ones that may be).
fn must_encode(c: char) -> bool {
    matches!(c, '>' | '<' | '=' | '!' | '*' | '|' | '%' | ' ')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparator {
    /// A bare version (the implicit equality comparator).
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Comparator {
    fn as_str(self) -> &'static str {
        match self {
            Comparator::Eq => "",
            Comparator::Ne => "!=",
            Comparator::Lt => "<",
            Comparator::Le => "<=",
            Comparator::Gt => ">",
            Comparator::Ge => ">=",
        }
    }
    fn is_lower(self) -> bool {
        matches!(self, Comparator::Gt | Comparator::Ge)
    }
    fn is_upper(self) -> bool {
        matches!(self, Comparator::Lt | Comparator::Le)
    }
}

/// A version as ordered by a vers type.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Key {
    Pep440(VersionSpec),
    Semver(semver::Version),
    Gem(GemVersion),
    Composer(ComposerVersion),
    Lex(String),
}

/// The version's ordering key in a type, or None if it has no order there.
pub(crate) fn key(ty: &str, v: &str) -> Option<Key> {
    match ty {
        "pypi" => Some(Key::Pep440(VersionSpec::new(v))),
        "npm" | "cargo" | "vscode-extension" | "swift" | "semver" => {
            semver::Version::parse(v).ok().map(Key::Semver)
        }
        "golang" => go_semver(v).map(Key::Semver),
        "gem" => GemVersion::parse(v).map(Key::Gem),
        "composer" => ComposerVersion::parse(v).map(Key::Composer),
        "lexicographic" => Some(Key::Lex(v.to_string())),
        // one to four integers, compared numerically
        "chrome-extension" => crate::eco::browser_version::is_chromium_version(v)
            .then(|| Key::Pep440(VersionSpec::new(v))),
        _ => None,
    }
}

/// Exact match of unordered versions: SHAs case-insensitively, others as written.
fn same_text(a: &str, b: &str) -> bool {
    a == b || (is_sha(a) && is_sha(b) && a.eq_ignore_ascii_case(b))
}

#[derive(Clone, Debug)]
pub struct Constraint {
    pub comparator: Comparator,
    /// The version, percent-decoded.
    pub version: String,
    key: Option<Key>,
}

impl Constraint {
    pub fn new(comparator: Comparator, version: &str, ty: &str) -> Self {
        Constraint {
            comparator,
            version: version.to_string(),
            key: key(ty, version),
        }
    }
}

/// A parsed, validated version range.
#[derive(Clone, Debug)]
pub struct Vers {
    pub ty: String,
    /// None is `*`.
    pub constraints: Option<Vec<Constraint>>,
}

fn decode(s: &str) -> Result<String, String> {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'%' {
            let hex = b
                .get(i + 1..i + 3)
                .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
                .ok_or("invalid percent-encoding in version")?;
            if hex.iter().any(u8::is_ascii_lowercase) {
                return Err("percent-encoding in version is not canonical".into());
            }
            let d = u8::from_str_radix(std::str::from_utf8(hex).unwrap(), 16).unwrap()
                as char;
            if d.is_ascii_whitespace() && d != ' ' {
                return Err("whitespace is not permitted".into());
            }
            if !must_encode(d) {
                return Err("percent-encoding in version is not canonical".into());
            }
            out.push(d);
            i += 3;
        } else {
            if must_encode(c as char) {
                return Err(format!(
                    "character {:?} in a version must be percent-encoded",
                    c as char
                ));
            }
            out.push(c as char);
            i += 1;
        }
    }
    Ok(out)
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if must_encode(c) {
            out.push_str(&format!("%{:02X}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

impl Vers {
    /// Parse a canonical vers string; any non-canonical form is an error.
    pub fn parse(s: &str) -> Result<Self, String> {
        let err = |m: &str| format!("non-canonical VERS: {m}");
        if s.chars().any(|c| c.is_ascii_whitespace()) {
            return Err(err("whitespace is not permitted"));
        }
        if !s.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(err("only printable ASCII is permitted"));
        }
        let (scheme, rest) = s.split_once(':').ok_or_else(|| err("missing scheme"))?;
        if scheme != "vers" {
            return Err(err("the scheme must be \"vers\""));
        }
        let (ty, cs) = rest.split_once('/').ok_or_else(|| err("missing type"))?;
        let valid_type = ty.starts_with(|c: char| c.is_ascii_lowercase())
            && ty.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-'
            });
        if !valid_type {
            return Err(err("the type must be lowercase ASCII letters, digits, '.' or '-', starting with a letter"));
        }
        if !TYPES.contains(&ty) && ty != NONE {
            return Err(format!("unsupported VERS type: {ty}"));
        }
        if cs.is_empty() {
            return Err(err("constraints must not be empty"));
        }
        if cs == "*" {
            return Ok(Vers {
                ty: ty.to_string(),
                constraints: None,
            });
        }
        if ty == "all" || ty == NONE {
            return Err(err("the only range of this type is '*'"));
        }
        if cs.starts_with('|') {
            return Err(err("leading pipe is not permitted"));
        }
        if cs.ends_with('|') {
            return Err(err("trailing pipe is not permitted"));
        }
        if cs.contains("||") {
            return Err(err("consecutive pipes are not permitted"));
        }
        let mut constraints = Vec::new();
        for part in cs.split('|') {
            if part.starts_with('*') {
                return Err(err("'*' must be used alone"));
            }
            let (comparator, version) = [
                (">=", Comparator::Ge),
                ("<=", Comparator::Le),
                ("!=", Comparator::Ne),
                ("<", Comparator::Lt),
                (">", Comparator::Gt),
            ]
            .iter()
            .find_map(|(p, c)| part.strip_prefix(p).map(|v| (*c, v)))
            .unwrap_or((Comparator::Eq, part));
            if version.starts_with('=') {
                return Err(err("explicit equality comparator is not permitted"));
            }
            if version.is_empty() {
                return Err(err("version must not be empty"));
            }
            let version = decode(version).map_err(|e| err(&e))?;
            constraints.push(Constraint::new(comparator, &version, ty));
        }
        let v = Vers {
            ty: ty.to_string(),
            constraints: Some(constraints),
        };
        v.validate().map_err(|e| err(&e))?;
        Ok(v)
    }

    /// A range from constraints in any order: ordered versions are sorted by version, then unordered ones by text; the result must be valid.
    pub fn new(ty: &str, mut constraints: Vec<Constraint>) -> Result<Self, String> {
        if !TYPES.contains(&ty) && ty != NONE {
            return Err(format!("unsupported VERS type: {ty}"));
        }
        constraints.sort_by(|a, b| match (&a.key, &b.key) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.version.cmp(&b.version),
        });
        let v = Vers {
            ty: ty.to_string(),
            constraints: Some(constraints),
        };
        v.validate()?;
        Ok(v)
    }

    /// The specification's validation rules: versions sorted and unique; `*` alone; ignoring `!=`, a bare version followed only by a bare version or a lower bound; ignoring both, lower and upper bounds alternating.
    fn validate(&self) -> Result<(), String> {
        let Some(cs) = &self.constraints else {
            return Ok(());
        };
        if cs.is_empty() {
            return Err("constraints must not be empty".into());
        }
        let mut unordered: Vec<&str> = Vec::new();
        let mut ordered: Vec<&Constraint> = Vec::new();
        for c in cs {
            if c.key.is_none() {
                if !matches!(c.comparator, Comparator::Eq | Comparator::Ne) {
                    return Err(format!(
                        "version {:?} has no {} order, so only an exact version or '!=' is permitted",
                        c.version, self.ty
                    ));
                }
                if unordered.iter().any(|u| same_text(u, &c.version)) {
                    return Err(format!("duplicated version {:?}", c.version));
                }
                unordered.push(&c.version);
            } else {
                ordered.push(c);
            }
        }
        for w in ordered.windows(2) {
            match w[0].key.cmp(&w[1].key) {
                std::cmp::Ordering::Equal => {
                    return Err(format!("duplicated version {:?}", w[1].version))
                }
                std::cmp::Ordering::Greater => {
                    return Err("constraints are not sorted by version".into())
                }
                std::cmp::Ordering::Less => {}
            }
        }
        let no_ne: Vec<Comparator> = ordered
            .iter()
            .map(|c| c.comparator)
            .filter(|c| *c != Comparator::Ne)
            .collect();
        for w in no_ne.windows(2) {
            if w[0] == Comparator::Eq && !(w[1] == Comparator::Eq || w[1].is_lower()) {
                return Err(format!(
                    "invalid comparator sequence: a version followed by '{}'",
                    w[1].as_str()
                ));
            }
        }
        let bounds: Vec<Comparator> =
            no_ne.into_iter().filter(|c| *c != Comparator::Eq).collect();
        for w in bounds.windows(2) {
            if w[0].is_upper() == w[1].is_upper() {
                return Err(format!(
                    "invalid comparator sequence: '{}' followed by '{}'",
                    w[0].as_str(),
                    w[1].as_str()
                ));
            }
        }
        Ok(())
    }

    /// Is the version in the range?
    pub fn contains(&self, version: &str) -> bool {
        let Some(cs) = &self.constraints else {
            return self.ty != NONE;
        };
        let k = key(&self.ty, version);
        let equal = |c: &Constraint| match (&k, &c.key) {
            (Some(a), Some(b)) => a == b,
            (None, None) => same_text(version, &c.version),
            _ => false,
        };
        if cs.iter().any(|c| {
            matches!(
                c.comparator,
                Comparator::Eq | Comparator::Le | Comparator::Ge
            ) && equal(c)
        }) {
            return true;
        }
        if cs
            .iter()
            .any(|c| c.comparator == Comparator::Ne && equal(c))
        {
            return false;
        }
        let bounds: Vec<(Comparator, &Key)> = cs
            .iter()
            .filter(|c| !matches!(c.comparator, Comparator::Eq | Comparator::Ne))
            .filter_map(|c| c.key.as_ref().map(|k| (c.comparator, k)))
            .collect();
        if bounds.is_empty() {
            // only exact versions (none matched), or only exclusions
            return cs.iter().all(|c| c.comparator == Comparator::Ne);
        }
        let Some(k) = k else {
            return false;
        };
        let mut i = 0;
        if bounds[0].0.is_upper() {
            if k < *bounds[0].1 {
                return true;
            }
            i = 1;
        }
        while i < bounds.len() {
            let above = k > *bounds[i].1;
            let below = bounds.get(i + 1).is_none_or(|(_, hi)| k < **hi);
            if above && below {
                return true;
            }
            i += 2;
        }
        false
    }

    /// The single version a range names, if it is exactly one version.
    pub fn exact(&self) -> Option<&str> {
        match self.constraints.as_deref() {
            Some([c]) if c.comparator == Comparator::Eq => Some(&c.version),
            _ => None,
        }
    }

    /// The range as PEP 508 requirement lines for `name`, any of which may be satisfied (as fetter reads several lines for one package): one per exact version and one per interval, each with the `!=` exclusions.
    pub fn to_pep508(&self, name: &str) -> Result<Vec<String>, String> {
        let Some(cs) = &self.constraints else {
            return if self.ty == NONE {
                Err(format!("vers:none for {name} allows no version"))
            } else {
                Ok(vec![name.to_string()])
            };
        };
        let ne: String = cs
            .iter()
            .filter(|c| c.comparator == Comparator::Ne)
            .map(|c| format!(",!={}", c.version))
            .collect();
        let mut lines: Vec<String> = cs
            .iter()
            .filter(|c| c.comparator == Comparator::Eq)
            .map(|c| format!("{name}=={}", c.version))
            .collect();
        let bounds: Vec<&Constraint> = cs
            .iter()
            .filter(|c| !matches!(c.comparator, Comparator::Eq | Comparator::Ne))
            .collect();
        let spec = |c: &Constraint| format!("{}{}", c.comparator.as_str(), c.version);
        let mut i = 0;
        if bounds.first().is_some_and(|c| c.comparator.is_upper()) {
            lines.push(format!("{name}{}{ne}", spec(bounds[0])));
            i = 1;
        }
        while i < bounds.len() {
            let hi = bounds
                .get(i + 1)
                .map(|c| format!(",{}", spec(c)))
                .unwrap_or_default();
            lines.push(format!("{name}{}{hi}{ne}", spec(bounds[i])));
            i += 2;
        }
        if lines.is_empty() {
            // only exclusions
            lines.push(format!("{name}{}", ne.replacen(',', "", 1)));
        }
        Ok(lines)
    }
}

impl fmt::Display for Vers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "vers:{}/", self.ty)?;
        match &self.constraints {
            None => write!(f, "*"),
            Some(cs) => {
                let parts: Vec<String> = cs
                    .iter()
                    .map(|c| format!("{}{}", c.comparator.as_str(), encode(&c.version)))
                    .collect();
                write!(f, "{}", parts.join("|"))
            }
        }
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn tests_of(json: &str) -> Vec<Value> {
        let v: Value = serde_json::from_str(json).unwrap();
        v["tests"].as_array().unwrap().clone()
    }

    /// Upstream cases skipped: the `datetime` type, which fetter does not support (so its expected failures would pass for the wrong reason).
    fn skipped(input: &str) -> bool {
        input.starts_with("vers:datetime/")
    }

    #[test]
    fn test_vers_canonical_parse_spec_a() {
        let mut checked = 0;
        for t in tests_of(include_str!(
            "../tests/fixtures/vers-spec/vers_canonical_parse_test.json"
        )) {
            let input = t["input"].as_str().unwrap();
            if skipped(input) {
                continue;
            }
            let r = Vers::parse(input);
            match t["test_type"].as_str().unwrap() {
                "parse" if t["expected_failure"].as_bool() == Some(true) => {
                    assert!(r.is_err(), "{input} should fail");
                }
                "parse" => {
                    let v = r.unwrap_or_else(|e| panic!("{input}: {e}"));
                    let exp = &t["expected_output"];
                    assert_eq!(v.ty, exp["type"].as_str().unwrap());
                    let got: Vec<(String, String)> = v
                        .constraints
                        .unwrap()
                        .iter()
                        .map(|c| {
                            let op = match c.comparator {
                                Comparator::Eq => "=",
                                o => o.as_str(),
                            };
                            (op.to_string(), c.version.clone())
                        })
                        .collect();
                    let want: Vec<(String, String)> = exp["constraints"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|p| {
                            (
                                p[0].as_str().unwrap().to_string(),
                                p[1].as_str().unwrap().to_string(),
                            )
                        })
                        .collect();
                    assert_eq!(got, want, "{input}");
                    // canonical input round-trips
                    assert_eq!(Vers::parse(input).unwrap().to_string(), input);
                }
                // a recommended normalization of non-canonical input; fetter's parser is strict and rejects it
                "validate" => assert!(r.is_err(), "{input}"),
                other => panic!("unexpected test type {other}"),
            }
            checked += 1;
        }
        assert_eq!(checked, 16);
    }

    #[test]
    fn test_vers_containment_spec_a() {
        let mut checked = 0;
        for f in [
            include_str!("../tests/fixtures/vers-spec/npm_range_containment_test.json"),
            include_str!("../tests/fixtures/vers-spec/pypi_range_containment_test.json"),
        ] {
            for t in tests_of(f) {
                let vers = t["input"]["vers"].as_str().unwrap();
                let version = t["input"]["version"].as_str().unwrap();
                let expected = t["expected_output"].as_bool().unwrap();
                match Vers::parse(vers) {
                    Ok(v) => {
                        assert_eq!(v.contains(version), expected, "{vers} {version}")
                    }
                    // three upstream ranges are not canonically ordered (e.g. `>=3.0.0|2.0.3`), which the specification requires parsers to reject
                    Err(e) => {
                        assert!(e.contains("not sorted"), "{vers}: {e}");
                        continue;
                    }
                }
                checked += 1;
            }
        }
        assert_eq!(checked, 8);
    }

    #[test]
    fn test_vers_validate_spec_a() {
        // `validate` cases: canonical input must parse and round-trip; for non-canonical input (expected output differs) fetter reports an error rather than normalizing. Upstream errata: cases with duplicated versions (`>=0.0.1|>=0.0.1`) or redundant bounds (`>0.0.0|>=0.0.1`), which the specification's validation rules make invalid, are expected to be returned unchanged; fetter rejects them.
        let mut ok = 0;
        let mut rejected = 0;
        for t in tests_of(include_str!(
            "../tests/fixtures/vers-spec/pypi_range_validate_test.json"
        )) {
            let input = t["input"].as_str().unwrap();
            let expected = t["expected_output"].as_str().unwrap();
            match Vers::parse(input) {
                Ok(v) => {
                    assert_eq!(input, expected);
                    assert_eq!(v.to_string(), expected);
                    ok += 1;
                }
                Err(e) => {
                    assert!(
                        input != expected
                            || e.contains("duplicated")
                            || e.contains("comparator sequence"),
                        "{input}: {e}"
                    );
                    rejected += 1;
                }
            }
        }
        assert_eq!((ok, rejected), (14, 5));
    }

    #[test]
    fn test_vers_gem_from_native_a() {
        // the upstream conversion of `~>2.0.8` contains the same versions as RubyGems' own requirement
        for t in tests_of(include_str!(
            "../tests/fixtures/vers-spec/gem_range_from_native_test.json"
        )) {
            let native = t["input"]["native_range"].as_str().unwrap();
            let v = Vers::parse(t["expected_output"].as_str().unwrap()).unwrap();
            let req = crate::eco::ruby::GemRequirement::parse(native).unwrap();
            for version in ["2.0.7", "2.0.8", "2.0.9", "2.0.10", "2.1", "2.1.0", "3.0"] {
                assert_eq!(
                    v.contains(version),
                    req.matches(&GemVersion::parse(version).unwrap()),
                    "{native} {version}"
                );
            }
        }
    }

    #[test]
    fn test_vers_contains_a() {
        let v = Vers::parse("vers:npm/1.2.3|>=2.0.0|<5.0.0").unwrap();
        for (version, inside) in [
            ("1.2.3", true),
            ("1.2.4", false),
            ("2.0.0", true),
            ("4.9.9", true),
            ("5.0.0-rc.1", true), // pure version order: a pre-release of 5.0.0 is below it
            ("5.0.0", false),
            ("not-a-version", false),
        ] {
            assert_eq!(v.contains(version), inside, "{version}");
        }
        let v = Vers::parse("vers:gem/>=2.2.0|!=2.2.1|<2.3.0").unwrap();
        assert!(v.contains("2.2.0") && v.contains("2.2.2"));
        assert!(!v.contains("2.2.1") && !v.contains("2.3.0"));
        let v = Vers::parse("vers:pypi/<=1.0|>=2.0").unwrap();
        assert!(v.contains("0.5") && v.contains("1.0.0") && v.contains("3"));
        assert!(!v.contains("1.5"));
        let v = Vers::parse("vers:pypi/!=5").unwrap();
        assert!(v.contains("4") && !v.contains("5.0"));
        assert!(Vers::parse("vers:all/*").unwrap().contains("x"));
        assert!(!Vers::parse("vers:none/*").unwrap().contains("x"));
        let v = Vers::parse("vers:golang/>=v0.17.0").unwrap();
        assert!(v.contains("v0.18.0") && !v.contains("0.18.0"));
    }

    #[test]
    fn test_vers_chrome_extension_a() {
        // one to four integers, compared numerically
        let v = Vers::parse("vers:chrome-extension/>=1.9|<1.10.0.2").unwrap();
        assert!(v.contains("1.10"));
        assert!(v.contains("1.10.0.1"));
        assert!(!v.contains("1.10.0.2"));
        assert!(!v.contains("1.8.99"));
        assert!(Vers::parse("vers:chrome-extension/>=1.10|<1.9").is_err());
    }

    #[test]
    fn test_vers_unordered_a() {
        let v = Vers::parse("vers:github/v4|v4.2.2").unwrap();
        assert!(v.contains("v4") && v.contains("v4.2.2") && !v.contains("v5"));
        assert!(Vers::parse("vers:github/>=v4").is_err());
        assert!(Vers::parse("vers:github/v4|v4").is_err());
        let sha = "5581748cef2bae787496fe6d61139aebe0a451f6";
        let v = Vers::parse(&format!("vers:swift/>=2.0.0|<3.0.0|{sha}")).unwrap();
        assert!(v.contains("2.8.1") && v.contains(&sha.to_uppercase()));
        assert!(!v.contains("3.0.0"));
        let v = Vers::parse("vers:composer/dev-main|>=1.0").unwrap();
        assert!(v.contains("dev-main") && v.contains("1.5") && !v.contains("dev-next"));
    }

    #[test]
    fn test_vers_errors_a() {
        for bad in [
            "vers:npm/>=1.0.0|1.5.0|<2.0.0", // a version inside an interval
            "vers:npm/>=1.0.0|>=1.5.0",      // two lower bounds
            "vers:npm/<1.0.0|<2.0.0",        // two upper bounds
            "vers:npm/1.0.0|<2.0.0",         // a version before an upper bound
            "vers:npm/1.0.0|1.0.0",
            "vers:npm/*|1.0.0",
            "vers:all/1.0",
            "vers:NPM/1.0.0",
            "VERS:npm/1.0.0",
            "vers:maven/1.0",
            "vers:npm/1<2",
            "vers:pypi/1.0|1.0.0", // equal in PEP 440
            "vers:npm/>=",
            "npm/1.0.0",
        ] {
            assert!(Vers::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn test_vers_new_and_display_a() {
        let v = Vers::new(
            "npm",
            vec![
                Constraint::new(Comparator::Le, "2.0.0", "npm"),
                Constraint::new(Comparator::Ge, "1.0.0", "npm"),
            ],
        )
        .unwrap();
        assert_eq!(v.to_string(), "vers:npm/>=1.0.0|<=2.0.0");
        let v = Vers::new(
            "github",
            vec![
                Constraint::new(Comparator::Eq, "v4.2.2", "github"),
                Constraint::new(Comparator::Eq, "v4", "github"),
            ],
        )
        .unwrap();
        assert_eq!(v.to_string(), "vers:github/v4|v4.2.2");
        let v = Vers::new(
            "lexicographic",
            vec![Constraint::new(Comparator::Eq, "a b|c", "lexicographic")],
        )
        .unwrap();
        assert_eq!(v.to_string(), "vers:lexicographic/a%20b%7Cc");
        assert_eq!(Vers::parse(&v.to_string()).unwrap().exact(), Some("a b|c"));
    }

    #[test]
    fn test_vers_to_pep508_a() {
        let lines = |s: &str| Vers::parse(s).unwrap().to_pep508("requests").unwrap();
        assert_eq!(
            lines("vers:pypi/>=2.31|!=2.32.0|<3"),
            vec!["requests>=2.31,<3,!=2.32.0"]
        );
        assert_eq!(
            lines("vers:pypi/1.0|>=2.0|<3|>=4"),
            vec!["requests==1.0", "requests>=2.0,<3", "requests>=4"]
        );
        assert_eq!(
            lines("vers:pypi/<=1.5|2.0|>=4|!=4.1"),
            vec!["requests==2.0", "requests<=1.5,!=4.1", "requests>=4,!=4.1"]
        );
        assert_eq!(lines("vers:pypi/!=1|!=2"), vec!["requests!=1,!=2"]);
        assert_eq!(lines("vers:pypi/*"), vec!["requests"]);
        assert!(Vers::parse("vers:none/*").unwrap().to_pep508("x").is_err());
    }
}
