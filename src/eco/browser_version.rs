//! Browser extension versions and allow-list requirements.
//!
//! Chromium versions are one to four dot-separated integers (https://developer.chrome.com/docs/extensions/reference/manifest/version), compared numerically with missing parts as 0. Firefox versions follow Mozilla's toolkit format (https://firefox-source-docs.mozilla.org/toolkit/mozapps/extensions/addon-manager/AddonManager.html, `nsVersionComparator`): each dot-separated part is `<number><string><number><extra>`, where a missing string sorts after any string, so `1.0a1 < 1.0`, and `1.0+` is `1.1pre`.
//!
//! A requirement is `*` (any version), or comma-separated clauses that must all hold, each an operator (`>=`, `<=`, `>`, `<`, `=`, `==`, `!=`) and a version, or a bare version (exact): `">=1.2, <2"`.

use std::cmp::Ordering;

use crate::ecosystem::Ecosystem;
use crate::version_spec::VersionSpec;

/// True for a Chromium extension version: one to four integers, each at most 65535, without leading zeros.
pub(crate) fn is_chromium_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    (1..=4).contains(&parts.len())
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
                && p.parse::<u32>().is_ok_and(|n| n <= 65535)
        })
}

/// One part of a Mozilla toolkit version.
#[derive(Debug, PartialEq, Eq)]
struct MozPart<'a> {
    a: i64,
    b: Option<&'a str>,
    c: i64,
    d: Option<&'a str>,
}

fn leading_number(s: &str) -> (i64, &str) {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    (s[..end].parse().unwrap_or(0), &s[end..])
}

fn moz_part(s: &str) -> MozPart<'_> {
    if s == "*" {
        return MozPart {
            a: i64::MAX,
            b: None,
            c: 0,
            d: None,
        };
    }
    let (mut a, rest) = leading_number(s);
    if let Some(after) = rest.strip_prefix('+') {
        // `1+` is `2pre`
        a += 1;
        let (c, d) = leading_number(after);
        return MozPart {
            a,
            b: Some("pre"),
            c,
            d: (!d.is_empty()).then_some(d),
        };
    }
    let end = rest
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(rest.len());
    let (b, rest) = rest.split_at(end);
    let (c, d) = leading_number(rest);
    MozPart {
        a,
        b: (!b.is_empty()).then_some(b),
        c,
        d: (!d.is_empty()).then_some(d),
    }
}

/// A missing string sorts after any string.
fn cmp_str(x: Option<&str>, y: Option<&str>) -> Ordering {
    match (x, y) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => x.cmp(y),
    }
}

/// Compare two Mozilla toolkit versions.
pub(crate) fn moz_cmp(x: &str, y: &str) -> Ordering {
    let xs: Vec<&str> = x.split('.').collect();
    let ys: Vec<&str> = y.split('.').collect();
    for i in 0..xs.len().max(ys.len()) {
        let p = moz_part(xs.get(i).copied().unwrap_or(""));
        let q = moz_part(ys.get(i).copied().unwrap_or(""));
        let o =
            p.a.cmp(&q.a)
                .then_with(|| cmp_str(p.b, q.b))
                .then_with(|| p.c.cmp(&q.c))
                .then_with(|| cmp_str(p.d, q.d));
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

/// Compare two versions of a browser ecosystem; `None` if a Chromium version is malformed (or the ecosystem is not a browser).
pub(crate) fn compare(eco: Ecosystem, x: &str, y: &str) -> Option<Ordering> {
    match eco {
        Ecosystem::Chromium => (is_chromium_version(x) && is_chromium_version(y))
            .then(|| VersionSpec::new(x).cmp(&VersionSpec::new(y))),
        Ecosystem::Firefox => Some(moz_cmp(x, y)),
        _ => None,
    }
}

//------------------------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Ge,
    Le,
    Gt,
    Lt,
    Eq,
    Ne,
}

/// A browser extension version requirement (see the module documentation).
#[derive(Debug, Clone)]
pub(crate) struct BrowserReq {
    /// None is any version.
    clauses: Option<Vec<(Op, String)>>,
}

impl BrowserReq {
    pub(crate) fn parse(eco: Ecosystem, s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s == "*" {
            return Ok(BrowserReq { clauses: None });
        }
        let mut clauses = Vec::new();
        for c in s.split(',') {
            let c = c.trim();
            let (op, v) = [
                (">=", Op::Ge),
                ("<=", Op::Le),
                ("!=", Op::Ne),
                ("==", Op::Eq),
                (">", Op::Gt),
                ("<", Op::Lt),
                ("=", Op::Eq),
            ]
            .iter()
            .find_map(|(p, op)| c.strip_prefix(p).map(|v| (*op, v.trim())))
            .unwrap_or((Op::Eq, c));
            if v.is_empty() || v.contains(char::is_whitespace) {
                return Err(format!("invalid clause {c:?}"));
            }
            if eco == Ecosystem::Chromium && !is_chromium_version(v) {
                return Err(format!(
                    "{v:?} is not a Chromium extension version (one to four integers)"
                ));
            }
            clauses.push((op, v.to_string()));
        }
        Ok(BrowserReq {
            clauses: Some(clauses),
        })
    }

    pub(crate) fn matches(&self, eco: Ecosystem, version: &str) -> bool {
        let Some(clauses) = &self.clauses else {
            return true;
        };
        clauses.iter().all(|(op, v)| {
            compare(eco, version, v).is_some_and(|o| match op {
                Op::Ge => o != Ordering::Less,
                Op::Le => o != Ordering::Greater,
                Op::Gt => o == Ordering::Greater,
                Op::Lt => o == Ordering::Less,
                Op::Eq => o == Ordering::Equal,
                Op::Ne => o != Ordering::Equal,
            })
        })
    }

    /// The version, if the requirement names exactly one (a single `=` clause or a bare version).
    pub(crate) fn exact(&self) -> Option<&str> {
        match self.clauses.as_deref() {
            Some([(Op::Eq, v)]) => Some(v),
            _ => None,
        }
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_chromium_version_a() {
        for v in ["1", "1.0", "1.0.0.6", "65535.0.0.1", "0.2"] {
            assert!(is_chromium_version(v), "{v}");
        }
        for v in ["", "1.0.0.0.1", "01.2", "1.a", "65536", "1..2", "v1"] {
            assert!(!is_chromium_version(v), "{v}");
        }
    }

    #[test]
    fn test_moz_cmp_a() {
        // the ordering example in Mozilla's documentation
        let ordered = [
            "1.0pre1",
            "1.0pre2",
            "1.0",
            "1.0.0",
            "1.0.0.0",
            "1.1pre",
            "1.1pre0",
            "1.0+",
            "1.1pre1a",
            "1.1pre1",
            "1.1pre10a",
            "1.1pre10",
        ];
        for w in ordered.windows(2) {
            let o = moz_cmp(w[0], w[1]);
            assert!(o != Ordering::Greater, "{} <= {}", w[0], w[1]);
        }
        assert_eq!(moz_cmp("1.0", "1.0.0.0"), Ordering::Equal);
        assert_eq!(moz_cmp("1.1pre", "1.0+"), Ordering::Equal);
        assert_eq!(moz_cmp("1.0a1", "1.0"), Ordering::Less);
        assert_eq!(moz_cmp("1.10", "1.9"), Ordering::Greater);
        assert_eq!(moz_cmp("1.*", "1.99"), Ordering::Greater);
    }

    #[test]
    fn test_browser_req_a() {
        let c = Ecosystem::Chromium;
        let r = BrowserReq::parse(c, ">=1.2, <2").unwrap();
        assert!(r.matches(c, "1.2"));
        assert!(r.matches(c, "1.10.0.3"));
        assert!(!r.matches(c, "2.0"));
        assert!(!r.matches(c, "1.1.9"));
        assert!(BrowserReq::parse(c, "*").unwrap().matches(c, "9"));
        assert!(BrowserReq::parse(c, "1.0.0.6")
            .unwrap()
            .matches(c, "1.0.0.6"));
        assert_eq!(BrowserReq::parse(c, "=1.0").unwrap().exact(), Some("1.0"));
        assert_eq!(BrowserReq::parse(c, ">=1.0").unwrap().exact(), None);
        assert!(BrowserReq::parse(c, ">=1.0a").is_err());
        assert!(BrowserReq::parse(c, ">= 1, ").is_err());
        let f = Ecosystem::Firefox;
        let r = BrowserReq::parse(f, ">=6.0, !=6.49.0").unwrap();
        assert!(r.matches(f, "6.50.1"));
        assert!(!r.matches(f, "6.49.0"));
        assert!(!r.matches(f, "6.0a1"));
    }
}
