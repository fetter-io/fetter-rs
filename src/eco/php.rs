//! PHP (Composer / Packagist) scanner.
//!
//! An origin is a project directory with a `composer.json` and either an installed vendor directory (`vendor/composer/installed.json`, or the `config.vendor-dir` of `composer.json`) or a `composer.lock`. Its site follows the "installed where it exists, else lockfile" rule (as for npm): the vendor directory (`SiteKind::Installed`) if installed, otherwise `composer.lock` (`SiteKind::Lockfile`). A vendor directory without a `composer.json` beside it (e.g. shipped inside a WordPress plugin) is an origin of its own.
//!
//! Package names are lowercased (Packagist names are case-insensitive and canonically lowercase; OSV's are case-sensitive and lowercase). Versions are kept as written (`v5.4.0`, `1.2.3`, `dev-main`). A branch version (`dev-*`, `*-dev`) is not a release; it carries its commit as provenance and is not audited, as OSV does not interpret it (a query for `dev-master` matches unrelated advisories).
//!
//! Versions and constraints follow composer/semver: `normalize` ports `VersionParser::normalize`, `ComposerConstraint` ports `VersionParser::parseConstraints` and `Constraint` matching, and `version_compare` ports PHP's `version_compare`.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value as JsonValue;

use crate::eco::Artifacts;
use crate::eco::Detect;
use crate::eco::DirItem;
use crate::eco::EcosystemScanner;
use crate::eco::Hook;
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

const COMPOSER_JSON: &str = "composer.json";
const COMPOSER_LOCK: &str = "composer.lock";
const INSTALLED_JSON: &str = "composer/installed.json";
const DEFAULT_VENDOR_DIR: &str = "vendor";

//------------------------------------------------------------------------------
// PHP version_compare

/// Insert `.` at digit/non-digit boundaries and replace other separators with `.`, as PHP's `php_canonicalize_version`.
fn php_canonicalize(v: &str) -> String {
    let chars: Vec<char> = v.chars().collect();
    let Some(first) = chars.first() else {
        return String::new();
    };
    let isdig = |c: char| c.is_ascii_digit();
    let isndig = |c: char| !c.is_ascii_digit() && c != '.';
    let mut out = String::new();
    out.push(*first);
    let mut lp = *first;
    for &c in &chars[1..] {
        let lq = out.chars().last().unwrap_or('.');
        if c == '-' || c == '_' || c == '+' {
            if lq != '.' {
                out.push('.');
            }
        } else if (isndig(lp) && isdig(c)) || (isdig(lp) && isndig(c)) {
            if lq != '.' {
                out.push('.');
            }
            out.push(c);
        } else if !c.is_ascii_alphanumeric() {
            if lq != '.' {
                out.push('.');
            }
        } else {
            out.push(c);
        }
        lp = c;
    }
    out
}

/// The order of PHP's special version forms (prefix match, as PHP's `compare_special_version_forms`); unknown words sort lowest.
fn special_form_order(s: &str) -> i32 {
    const FORMS: [(&str, i32); 10] = [
        ("dev", 0),
        ("alpha", 1),
        ("a", 1),
        ("beta", 2),
        ("b", 2),
        ("RC", 3),
        ("rc", 3),
        ("#", 4),
        ("pl", 5),
        ("p", 5),
    ];
    FORMS
        .iter()
        .find(|(name, _)| s.starts_with(name))
        .map(|(_, o)| *o)
        .unwrap_or(-1)
}

/// A number's position in special-form order.
const NUMBER_FORM: &str = "#N#";

fn compare_parts(a: &[&str], b: &[&str]) -> Ordering {
    let isnum = |s: &str| s.starts_with(|c: char| c.is_ascii_digit());
    let mut i = 0;
    while i < a.len() && i < b.len() {
        let (p1, p2) = (a[i], b[i]);
        let o = match (isnum(p1), isnum(p2)) {
            (true, true) => {
                let n = |s: &str| -> u128 {
                    s.chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect::<String>()
                        .parse()
                        .unwrap_or(u128::MAX)
                };
                n(p1).cmp(&n(p2))
            }
            (false, false) => special_form_order(p1).cmp(&special_form_order(p2)),
            (true, false) => special_form_order(NUMBER_FORM).cmp(&special_form_order(p2)),
            (false, true) => special_form_order(p1).cmp(&special_form_order(NUMBER_FORM)),
        };
        if o != Ordering::Equal {
            return o;
        }
        i += 1;
    }
    if i < a.len() {
        if isnum(a[i]) {
            Ordering::Greater
        } else {
            compare_parts(&a[i..], &[NUMBER_FORM])
        }
    } else if i < b.len() {
        if isnum(b[i]) {
            Ordering::Less
        } else {
            compare_parts(&[NUMBER_FORM], &b[i..])
        }
    } else {
        Ordering::Equal
    }
}

/// PHP's `version_compare` (without an operator).
pub(crate) fn version_compare(a: &str, b: &str) -> Ordering {
    let (ca, cb) = (php_canonicalize(a), php_canonicalize(b));
    let pa: Vec<&str> = ca.split('.').filter(|s| !s.is_empty()).collect();
    let pb: Vec<&str> = cb.split('.').filter(|s| !s.is_empty()).collect();
    compare_parts(&pa, &pb)
}

//------------------------------------------------------------------------------
// normalization (composer/semver VersionParser)

/// The parts matched by composer's modifier pattern, `[._-]?(?:(stability)((?:[.-]?\d+)*+)?)?([.-]?dev)?`.
#[derive(Debug, Default, Clone)]
struct Modifier {
    stability: Option<String>,
    number: String,
    dev: bool,
}

/// Stability words in composer's alternation order.
const STABILITIES: [&str; 9] = [
    "stable", "beta", "b", "rc", "alpha", "a", "patch", "pl", "p",
];

/// Match all of `s` (case-insensitively) against the modifier pattern.
fn match_modifier(s: &str) -> Option<Modifier> {
    let lower = s.to_ascii_lowercase();
    let after_sep: Vec<&str> = match lower.chars().next() {
        Some('.' | '_' | '-') => vec![&lower[1..], &lower[..]],
        _ => vec![&lower[..]],
    };
    // the dev suffix: `[.-]?dev` or nothing
    let dev_rest = |r: &str| -> Option<bool> {
        match r {
            "" => Some(false),
            "dev" | ".dev" | "-dev" => Some(true),
            _ => None,
        }
    };
    for r in after_sep {
        // without a stability
        if let Some(dev) = dev_rest(r) {
            return Some(Modifier {
                dev,
                ..Default::default()
            });
        }
        for st in STABILITIES {
            let Some(rest) = r.strip_prefix(st) else {
                continue;
            };
            // the number group `(?:[.-]?\d+)*`, then the dev suffix; try the longest number first
            let mut ends = vec![0];
            let b = rest.as_bytes();
            let mut i = 0;
            loop {
                let mut j = i;
                if j < b.len() && (b[j] == b'.' || b[j] == b'-') {
                    j += 1;
                }
                let start = j;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                if j == start {
                    break;
                }
                ends.push(j);
                i = j;
            }
            for &e in ends.iter().rev() {
                if let Some(dev) = dev_rest(&rest[e..]) {
                    return Some(Modifier {
                        stability: Some(st.to_string()),
                        number: rest[..e].to_string(),
                        dev,
                    });
                }
            }
        }
    }
    None
}

fn expand_stability(s: &str) -> &str {
    match s {
        "a" => "alpha",
        "b" => "beta",
        "p" | "pl" => "patch",
        "rc" => "RC",
        other => other,
    }
}

/// Append a modifier to a normalized version, as `VersionParser::normalize`.
fn with_modifier(mut version: String, m: &Modifier) -> String {
    if let Some(st) = &m.stability {
        if st == "stable" {
            return version;
        }
        version.push('-');
        version.push_str(expand_stability(st));
        version.push_str(m.number.trim_start_matches(['.', '-']));
    }
    if m.dev {
        version.push_str("-dev");
    }
    version
}

/// Lengths of prefixes of `s` matching `\d{4}(?:[.:-]?\d{2}){1,6}(?:[.:-]?\d{1,3}){0,2}`.
fn date_prefix_lengths(s: &str) -> Vec<usize> {
    let b = s.as_bytes();
    let digits_at = |i: usize, n: usize| {
        i + n <= b.len() && b[i..i + n].iter().all(u8::is_ascii_digit)
    };
    let seps = |i: usize| -> Vec<usize> {
        let mut v = vec![i];
        if i < b.len() && matches!(b[i], b'.' | b':' | b'-') {
            v.push(i + 1);
        }
        v
    };
    if !digits_at(0, 4) {
        return Vec::new();
    }
    // positions after k two-digit groups
    let mut frontier = vec![4];
    let mut after_pairs = Vec::new();
    for _ in 0..6 {
        let mut next = Vec::new();
        for &p in &frontier {
            for q in seps(p) {
                if digits_at(q, 2) {
                    next.push(q + 2);
                }
            }
        }
        next.sort();
        next.dedup();
        after_pairs.extend(next.iter().copied());
        frontier = next;
    }
    let mut out = after_pairs.clone();
    let mut frontier = after_pairs;
    for _ in 0..2 {
        let mut next = Vec::new();
        for &p in &frontier {
            for q in seps(p) {
                for n in 1..=3 {
                    if digits_at(q, n) {
                        next.push(q + n);
                    }
                }
            }
        }
        next.sort();
        next.dedup();
        out.extend(next.iter().copied());
        frontier = next;
    }
    out.sort();
    out.dedup();
    out
}

/// Strip a trailing `@stability` flag (`@dev`, `@stable`, ...).
fn strip_stability_flag(v: &str) -> (&str, Option<String>) {
    if let Some(at) = v.rfind('@') {
        let flag = v[at + 1..].to_ascii_lowercase();
        if ["stable", "rc", "beta", "alpha", "dev"].contains(&flag.as_str()) {
            return (&v[..at], Some(flag));
        }
    }
    (v, None)
}

/// Strip an alias: `X as Y` is `X`.
fn strip_alias(v: &str) -> &str {
    let parts: Vec<&str> = v.split_whitespace().collect();
    if parts.len() == 3 && parts[1] == "as" && !v.contains(',') {
        return parts[0];
    }
    v
}

/// The numeric parts of a version, `v?(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:\.(\d+))?` (at most `max_first` digits in the first), and the rest of the string.
fn leading_numbers(
    s: &str,
    max_first: Option<usize>,
) -> Option<([Option<String>; 4], &str)> {
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() && max_first.is_none_or(|m| i < m) {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    let mut nums: [Option<String>; 4] = [Some(s[..i].to_string()), None, None, None];
    for slot in nums.iter_mut().skip(1) {
        if i < b.len() && b[i] == b'.' {
            let start = i + 1;
            let mut j = start;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > start {
                *slot = Some(s[start..j].to_string());
                i = j;
                continue;
            }
        }
        break;
    }
    Some((nums, &s[i..]))
}

/// `VersionParser::normalizeBranch`.
fn normalize_branch(name: &str) -> String {
    let name = name.trim();
    let s = name.strip_prefix(['v', 'V']).unwrap_or(name);
    let mut parts: Vec<String> = Vec::new();
    let mut ok = !s.is_empty();
    for (k, p) in s.split('.').enumerate() {
        let is_num = !p.is_empty() && p.chars().all(|c| c.is_ascii_digit());
        let is_x = p == "x" || p == "X" || p == "*";
        if k >= 4 || !(is_num || (k > 0 && is_x)) {
            ok = false;
            break;
        }
        parts.push(if is_x { "x".to_string() } else { p.to_string() });
    }
    if ok {
        while parts.len() < 4 {
            parts.push("x".to_string());
        }
        return format!("{}-dev", parts.join(".").replace('x', "9999999"));
    }
    format!("dev-{name}")
}

/// `VersionParser::normalize`: a version to composer's normalized form (`v1.2` is `1.2.0.0`, `1.0.0-b2` is `1.0.0.0-beta2`, `master` is `dev-master`).
pub(crate) fn normalize(version: &str) -> Result<String, String> {
    let orig = version;
    let mut v = strip_alias(version.trim()).trim();
    v = strip_stability_flag(v).0;
    if ["master", "trunk", "default"].contains(&v) {
        return Ok(format!("dev-{v}"));
    }
    if v.len() >= 4 && v[..4].eq_ignore_ascii_case("dev-") {
        return Ok(format!("dev-{}", &v[4..]));
    }
    // build metadata
    if let Some((main, build)) = v.split_once('+') {
        if !main.is_empty()
            && !main.contains([',', ' '])
            && !build.is_empty()
            && !build.contains(char::is_whitespace)
        {
            v = main;
        }
    }
    // classical versioning
    if let Some((nums, rest)) = leading_numbers(v, Some(5)) {
        if let Some(m) = match_modifier(rest) {
            let base = nums
                .iter()
                .map(|n| n.clone().unwrap_or_else(|| "0".to_string()))
                .collect::<Vec<_>>()
                .join(".");
            return Ok(with_modifier(base, &m));
        }
    }
    // date-based versioning
    let dv = v.strip_prefix(['v', 'V']).unwrap_or(v);
    for len in date_prefix_lengths(dv).into_iter().rev() {
        if let Some(m) = match_modifier(&dv[len..]) {
            let base: String = dv[..len]
                .chars()
                .map(|c| if c.is_ascii_digit() { c } else { '.' })
                .collect();
            return Ok(with_modifier(base, &m));
        }
    }
    // numeric branches: `1.x-dev`
    let lower = v.to_ascii_lowercase();
    if let Some(stem) = lower.strip_suffix("dev") {
        let stem_len = stem.len();
        let stem = &v[..stem_len];
        let stem = stem.strip_suffix(['.', '-']).unwrap_or(stem);
        let n = normalize_branch(stem);
        if !n.starts_with("dev-") {
            return Ok(n);
        }
    }
    Err(format!("Invalid version string {orig:?}"))
}

/// True for a branch version (`dev-main`, `2.x-dev`), which names no release.
pub(crate) fn is_branch_version(version: &str) -> bool {
    let v = version.trim().to_ascii_lowercase();
    v.starts_with("dev-") || v.ends_with("-dev") || v.ends_with(".x-dev") || v == "master"
}

/// A version ordered as composer orders normalized versions (for `derive` and registry listings); branch versions do not parse.
#[derive(Debug, Clone)]
pub(crate) struct ComposerVersion(String);

impl ComposerVersion {
    pub(crate) fn parse(v: &str) -> Option<Self> {
        let n = normalize(v).ok()?;
        (!n.starts_with("dev-") && !n.ends_with("-dev")).then_some(ComposerVersion(n))
    }
}
impl Ord for ComposerVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        version_compare(&self.0, &other.0)
    }
}
impl PartialOrd for ComposerVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for ComposerVersion {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for ComposerVersion {}

//------------------------------------------------------------------------------
// constraints (composer/semver VersionParser::parseConstraints)

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone)]
enum Single {
    Any,
    Cmp(Op, String),
}

/// A composer version constraint: `||` (or `|`) separated alternatives, each a set of constraints separated by spaces or commas that must all hold. Supports `^`, `~`, wildcards (`1.2.*`), hyphen ranges (`1.0 - 2.0`), comparisons, `@stability` flags and aliases, as composer/semver does.
#[derive(Debug, Clone)]
pub(crate) struct ComposerConstraint(Vec<Vec<Single>>);

/// The version pattern used within constraints: numbers, then a modifier or `.x-dev`, then optional build metadata.
#[derive(Debug, Default)]
struct VersionMatch {
    nums: [Option<String>; 4],
    modifier: Modifier,
    x_dev: bool,
}

fn match_version_pattern(s: &str) -> Option<VersionMatch> {
    let (nums, rest) = leading_numbers(s, None)?;
    let rest = match rest.split_once('+') {
        Some((r, build)) if !build.is_empty() && !build.contains(char::is_whitespace) => {
            r
        }
        Some(_) => return None,
        None => rest,
    };
    if let Some(modifier) = match_modifier(rest) {
        return Some(VersionMatch {
            nums,
            modifier,
            x_dev: false,
        });
    }
    let r = rest.to_ascii_lowercase();
    if let Some(x) = r.strip_prefix('.') {
        let mut c = x.chars();
        if matches!(c.next(), Some('x' | '*')) {
            let tail: String = c.collect();
            if tail == "dev" || tail == ".dev" || tail == "-dev" {
                return Some(VersionMatch {
                    nums,
                    x_dev: true,
                    ..Default::default()
                });
            }
        }
    }
    None
}

/// `VersionParser::manipulateVersionString`.
fn manipulate(nums: &[Option<String>; 4], position: usize, increment: u64) -> String {
    let mut parts: Vec<String> =
        nums.iter().map(|n| n.clone().unwrap_or_default()).collect();
    for i in (1..=4).rev() {
        if i > position {
            parts[i - 1] = "0".to_string();
        } else if i == position && increment > 0 {
            let n: u64 = parts[i - 1].parse().unwrap_or(0);
            parts[i - 1] = (n + increment).to_string();
        }
    }
    parts.join(".")
}

fn has_modifier_stability(m: &VersionMatch) -> bool {
    m.modifier.stability.is_some() || m.modifier.dev || m.x_dev
}

/// True if `s` ends with `-` followed by a modifier (composer's `/-modifier$/i`).
fn ends_with_dash_modifier(s: &str) -> bool {
    s.char_indices()
        .filter(|(_, c)| *c == '-')
        .any(|(i, _)| match_modifier(&s[i + 1..]).is_some())
}

fn parse_single(constraint: &str) -> Result<Vec<Single>, String> {
    let err = || format!("Could not parse version constraint {constraint:?}");
    let mut c = strip_alias(constraint).trim();
    let (stripped, flag) = strip_stability_flag(c);
    let mut stability_modifier = None;
    if flag.is_some() && !stripped.contains([',', ' ']) {
        c = if stripped.is_empty() { "*" } else { stripped };
        stability_modifier = flag.filter(|f| f != "stable");
    }
    // `#ref` on branch constraints
    if let Some((head, _)) = c.split_once('#') {
        let lh = head.to_ascii_lowercase();
        if lh.starts_with("dev-") || lh.ends_with(".x-dev") {
            c = head;
        }
    }
    // wildcard
    {
        let w = c.strip_prefix(['v', 'V']);
        let body = w.unwrap_or(c);
        let parts: Vec<&str> = body.split('.').collect();
        if !body.is_empty() && parts.iter().all(|p| matches!(*p, "x" | "X" | "*")) {
            return Ok(if w.is_some() || parts.len() > 1 {
                vec![Single::Cmp(Op::Ge, "0.0.0.0-dev".to_string())]
            } else {
                vec![Single::Any]
            });
        }
    }
    let lower_bound = |body: &str, m: &VersionMatch| -> Result<String, String> {
        let suffix = if has_modifier_stability(m) {
            ""
        } else {
            "-dev"
        };
        normalize(&format!("{body}{suffix}"))
    };
    // tilde
    if let Some(body) = c.strip_prefix('~') {
        if body.starts_with('>') {
            return Err(format!("{}: invalid operator \"~>\"", err()));
        }
        let m = match_version_pattern(body).ok_or_else(err)?;
        let mut position = (1..=4).rev().find(|i| m.nums[i - 1].is_some()).unwrap_or(1);
        if m.x_dev {
            position += 1;
        }
        let low = lower_bound(body, &m)?;
        let high = format!(
            "{}-dev",
            manipulate(&m.nums, position.saturating_sub(1).max(1), 1)
        );
        return Ok(vec![Single::Cmp(Op::Ge, low), Single::Cmp(Op::Lt, high)]);
    }
    // caret
    if let Some(body) = c.strip_prefix('^') {
        let m = match_version_pattern(body).ok_or_else(err)?;
        let n = |i: usize| m.nums[i].as_deref();
        let position = if n(0) != Some("0") || n(1).is_none() {
            1
        } else if n(1) != Some("0") || n(2).is_none() {
            2
        } else {
            3
        };
        let low = lower_bound(body, &m)?;
        let high = format!("{}-dev", manipulate(&m.nums, position, 1));
        return Ok(vec![Single::Cmp(Op::Ge, low), Single::Cmp(Op::Lt, high)]);
    }
    // x-range: `1.2.*`, `1.x.x`
    if let Some((nums, rest)) = leading_numbers(c, None) {
        let r: Vec<&str> = rest.split('.').collect();
        if nums[3].is_none()
            && r.len() >= 2
            && r[0].is_empty()
            && r[1..].iter().all(|p| matches!(*p, "x" | "X" | "*"))
        {
            let position = (1..=3).rev().find(|i| nums[i - 1].is_some()).unwrap_or(1);
            let low = format!("{}-dev", manipulate(&nums, position, 0));
            let high = format!("{}-dev", manipulate(&nums, position, 1));
            return Ok(if low == "0.0.0.0-dev" {
                vec![Single::Cmp(Op::Lt, high)]
            } else {
                vec![Single::Cmp(Op::Ge, low), Single::Cmp(Op::Lt, high)]
            });
        }
    }
    // hyphen range: `1.0 - 2.0`
    if let Some((from, to)) = c.split_once(" - ") {
        let (from, to) = (from.trim_end(), to.trim_start());
        if let (Some(fm), Some(tm)) =
            (match_version_pattern(from), match_version_pattern(to))
        {
            let low_suffix =
                if fm.modifier.stability.is_some() || fm.modifier.dev || fm.x_dev {
                    ""
                } else {
                    "-dev"
                };
            let low = format!("{}{low_suffix}", normalize(from)?);
            let full = tm.nums[1].is_some() && tm.nums[2].is_some();
            let upper = if full || has_modifier_stability(&tm) {
                Single::Cmp(Op::Le, normalize(to)?)
            } else {
                normalize(to)?;
                let position = if tm.nums[1].is_none() { 1 } else { 2 };
                Single::Cmp(Op::Lt, format!("{}-dev", manipulate(&tm.nums, position, 1)))
            };
            return Ok(vec![Single::Cmp(Op::Ge, low), upper]);
        }
    }
    // basic comparators
    let (op, rest) = [
        ("<>", Op::Ne),
        ("!=", Op::Ne),
        (">=", Op::Ge),
        ("<=", Op::Le),
        ("==", Op::Eq),
        (">", Op::Gt),
        ("<", Op::Lt),
        ("=", Op::Eq),
    ]
    .iter()
    .find_map(|(t, o)| c.strip_prefix(t).map(|r| (Some(*o), r)))
    .unwrap_or((None, c));
    let rest = rest.trim_start();
    let mut version = match normalize(rest) {
        Ok(v) => v,
        Err(e) => {
            let recover = rest.strip_suffix("-dev").filter(|_| {
                rest.chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || "-./".contains(ch))
            });
            match recover {
                Some(stem) => normalize(&format!("dev-{stem}"))?,
                None => return Err(format!("{}: {e}", err())),
            }
        }
    };
    let op_v = op.unwrap_or(Op::Eq);
    if op_v != Op::Eq
        && stability_modifier.is_some()
        && stability_of(&version) == "stable"
    {
        version = format!("{version}-{}", stability_modifier.unwrap_or_default());
    } else if matches!(op_v, Op::Lt | Op::Ge)
        && !ends_with_dash_modifier(rest)
        && !rest.starts_with("dev-")
    {
        version.push_str("-dev");
    }
    Ok(vec![Single::Cmp(op_v, version)])
}

/// The stability of a normalized version (`VersionParser::parseStability`, simplified to the cases normalization produces).
fn stability_of(normalized: &str) -> &'static str {
    let l = normalized.to_ascii_lowercase();
    if l.starts_with("dev-") || l.ends_with("-dev") {
        "dev"
    } else if l.contains("-alpha") {
        "alpha"
    } else if l.contains("-beta") {
        "beta"
    } else if l.contains("-rc") {
        "RC"
    } else {
        "stable"
    }
}

/// Split one `||` alternative into its space- or comma-separated constraints, as composer's `(?<!^|as|[=>< ,]) *(?<!-)[, ](?!-) *(?!,|as|$)`: no split after an operator, around a hyphen-range `-`, or next to `as`.
fn split_and(s: &str) -> Vec<String> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] != b' ' && b[i] != b',' {
            i += 1;
            continue;
        }
        // the run: spaces, at most one comma, spaces
        let run_start = i;
        let mut j = i;
        while j < b.len() && b[j] == b' ' {
            j += 1;
        }
        let mut commas = 0;
        if j < b.len() && b[j] == b',' {
            commas += 1;
            j += 1;
        }
        while j < b.len() && b[j] == b' ' {
            j += 1;
        }
        let before = &s[..run_start];
        let after = &s[j..];
        let prev = before.chars().last();
        let split = commas <= 1
            && !before.is_empty()
            && !before.ends_with("as")
            && !prev.is_some_and(|p| "=><,-".contains(p))
            && !after.is_empty()
            && !after.starts_with(['-', ','])
            && !after.starts_with("as");
        if split {
            out.push(s[start..run_start].to_string());
            start = j;
        }
        i = j.max(i + 1);
    }
    out.push(s[start..].to_string());
    out
}

impl ComposerConstraint {
    pub(crate) fn parse(s: &str) -> Result<Self, String> {
        let t = s.trim();
        if t.is_empty() {
            return Err("empty constraint".to_string());
        }
        let mut groups = Vec::new();
        for alt in t.split('|').filter(|a| !a.trim().is_empty()) {
            let mut group = Vec::new();
            for part in split_and(alt.trim()) {
                group.extend(parse_single(part.trim())?);
            }
            groups.push(group);
        }
        if groups.is_empty() {
            return Err(format!("Could not parse version constraint {s:?}"));
        }
        Ok(ComposerConstraint(groups))
    }

    /// Does a version (as written) satisfy this constraint (`Semver::satisfies`)? Branch versions match only an equal branch constraint (or `*`).
    pub(crate) fn matches(&self, version: &str) -> bool {
        let Ok(v) = normalize(version) else {
            return false;
        };
        self.0.iter().any(|group| {
            group.iter().all(|c| match c {
                Single::Any => true,
                Single::Cmp(op, cv) => compare_op(&v, *op, cv),
            })
        })
    }

    /// The version, if this constraint names exactly one release.
    pub(crate) fn exact(&self) -> Option<&str> {
        match self.0.as_slice() {
            [g] => match g.as_slice() {
                [Single::Cmp(Op::Eq, v)] if !v.starts_with("dev-") => Some(v.as_str()),
                _ => None,
            },
            _ => None,
        }
    }
}

/// `Constraint::versionCompare` with branches not compared.
fn compare_op(a: &str, op: Op, b: &str) -> bool {
    let (ab, bb) = (a.starts_with("dev-"), b.starts_with("dev-"));
    if op == Op::Ne && (ab || bb) {
        return a != b;
    }
    if ab && bb {
        return op == Op::Eq && a == b;
    }
    if ab || bb {
        return false;
    }
    let o = version_compare(a, b);
    match op {
        Op::Eq => o == Ordering::Equal,
        Op::Ne => o != Ordering::Equal,
        Op::Lt => o == Ordering::Less,
        Op::Le => o != Ordering::Greater,
        Op::Gt => o == Ordering::Greater,
        Op::Ge => o != Ordering::Less,
    }
}

//------------------------------------------------------------------------------
// lockfiles and installed.json

/// A package from a `composer.lock` or `installed.json` entry. Path packages (local sources) are excluded. A branch version carries its source repository and commit.
fn entry_package(e: &JsonValue) -> Option<Package> {
    let name = e.get("name")?.as_str()?.to_ascii_lowercase();
    let version = e.get("version")?.as_str()?;
    let kind = |k: &str| {
        e.get(k)
            .and_then(|d| d.get("type"))
            .and_then(JsonValue::as_str)
    };
    if kind("dist") == Some("path") || kind("source") == Some("path") {
        return None;
    }
    let direct_url = if is_branch_version(version) {
        let src = e.get("source");
        let field = |f: &str| src.and_then(|s| s.get(f)).and_then(JsonValue::as_str);
        field("url").map(|url| DirectURL {
            url: url.to_string(),
            vcs_info: field("reference").map(|r| VcsInfo {
                commit_id: r.to_string(),
                vcs: field("type").unwrap_or("git").to_string(),
                revision: Some(version.to_string()),
            }),
        })
    } else {
        None
    };
    Some(Package::new(
        Ecosystem::Php,
        &name,
        VersionSpec::new(version),
        direct_url,
    ))
}

/// The package entries of a `composer.lock` (`packages` and `packages-dev`) or an `installed.json` (Composer 2: `{"packages": [...]}`; Composer 1: `[...]`).
fn entries(content: &str) -> Vec<JsonValue> {
    let Ok(v) = serde_json::from_str::<JsonValue>(content) else {
        return Vec::new();
    };
    if let Some(a) = v.as_array() {
        return a.clone();
    }
    ["packages", "packages-dev"]
        .iter()
        .filter_map(|k| v.get(*k).and_then(JsonValue::as_array))
        .flatten()
        .cloned()
        .collect()
}

fn packages_from(content: &str) -> Vec<Package> {
    let unique: HashSet<Package> =
        entries(content).iter().filter_map(entry_package).collect();
    unique.into_iter().collect()
}

/// A composer plugin (`"type": "composer-plugin"`) runs code during `composer install` and `update`; the detail is its plugin class.
fn entry_hook(e: &JsonValue) -> Option<Hook> {
    if e.get("type").and_then(JsonValue::as_str) != Some("composer-plugin") {
        return None;
    }
    let p = entry_package(e)?;
    let detail = match e.get("extra").and_then(|x| x.get("class")) {
        Some(JsonValue::String(s)) => s.clone(),
        Some(JsonValue::Array(a)) => a
            .iter()
            .filter_map(JsonValue::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        _ => "composer-plugin".to_string(),
    };
    Some(Hook {
        name: format!("{p} composer-plugin"),
        detail,
    })
}

/// The vendor directory of a project: `config.vendor-dir` of `composer.json`, else `vendor`.
fn vendor_dir(project: &Path) -> PathBuf {
    let configured = fs::read_to_string(project.join(COMPOSER_JSON))
        .ok()
        .and_then(|c| serde_json::from_str::<JsonValue>(&c).ok())
        .and_then(|v| {
            v.get("config")?
                .get("vendor-dir")?
                .as_str()
                .map(str::to_string)
        });
    match configured {
        Some(d) if d == "." || d.is_empty() => project.to_path_buf(),
        Some(d) => project.join(d),
        None => project.join(DEFAULT_VENDOR_DIR),
    }
}

fn is_installed(vendor: &Path) -> bool {
    vendor.join(INSTALLED_JSON).is_file()
}

/// A vendor directory with no `composer.json` beside it or in its parent (not part of a project).
fn is_standalone_vendor(dir: &Path) -> bool {
    is_installed(dir)
        && !dir.join(COMPOSER_JSON).is_file()
        && !dir
            .parent()
            .is_some_and(|p| p.join(COMPOSER_JSON).is_file())
}

fn origin(dir: PathBuf) -> Origin {
    Origin::new(Ecosystem::Php, PathShared::from(dir))
}

fn is_project(dir: &Path) -> bool {
    dir.join(COMPOSER_JSON).is_file()
        && (dir.join(COMPOSER_LOCK).is_file() || is_installed(&vendor_dir(dir)))
}

//------------------------------------------------------------------------------
pub(crate) struct PhpScanner;

impl EcosystemScanner for PhpScanner {
    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect {
        let has = |name: &str| items.iter().any(|i| i.file_name() == Some(name));
        if has(COMPOSER_JSON) {
            if !is_project(dir) {
                return Detect::default();
            }
            let vendor = vendor_dir(dir);
            // installed packages have composer.json files of their own; they are not projects
            let (prune, skip_children) = if vendor == dir {
                (true, Vec::new())
            } else if vendor == dir.join(DEFAULT_VENDOR_DIR) {
                (false, vec![DEFAULT_VENDOR_DIR])
            } else {
                (false, Vec::new())
            };
            return Detect {
                origins: vec![origin(dir.to_path_buf())],
                prune,
                skip_children,
            };
        }
        if has("composer") && is_standalone_vendor(dir) {
            return Detect {
                origins: vec![origin(dir.to_path_buf())],
                prune: true,
                ..Default::default()
            };
        }
        Detect::default()
    }

    /// Accept a `composer.json` or `composer.lock`, a project directory, or a vendor directory.
    fn accepts(&self, path: &Path) -> bool {
        let Ok(p) = path_normalize(path, false) else {
            return false;
        };
        if p.is_file() {
            return p
                .file_name()
                .is_some_and(|n| n == COMPOSER_JSON || n == COMPOSER_LOCK);
        }
        p.is_dir() && (is_project(&p) || is_installed(&p))
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
        if is_project(&dir) || is_installed(&dir) {
            Ok(origin(dir))
        } else {
            Err(format!(
                "No composer.lock or installed vendor directory found for {dir:?}; run `composer update --lock`"
            )
            .into())
        }
    }

    fn origin_to_sites(
        &self,
        origin: &Origin,
        _config: &ScanConfig,
        _log: FlagLog,
    ) -> Vec<Site> {
        let dir = origin.as_path();
        let site =
            |kind, p: PathBuf| vec![Site::new(Ecosystem::Php, kind, PathShared::from(p))];
        if dir.join(COMPOSER_JSON).is_file() {
            let vendor = vendor_dir(dir);
            if is_installed(&vendor) {
                return site(SiteKind::Installed, vendor);
            }
            let lock = dir.join(COMPOSER_LOCK);
            if lock.is_file() {
                return site(SiteKind::Lockfile, lock);
            }
            return Vec::new();
        }
        if is_installed(dir) {
            return site(SiteKind::Installed, dir.to_path_buf());
        }
        Vec::new()
    }

    fn site_to_packages(&self, site: &Site) -> Vec<Package> {
        let fp = match site.kind {
            SiteKind::Installed => site.as_path().join(INSTALLED_JSON),
            _ => site.as_path().to_path_buf(),
        };
        fs::read_to_string(fp)
            .map(|c| packages_from(&c))
            .unwrap_or_default()
    }

    /// Composer plugins, which run during `composer install` and `update` (from installed packages or the lockfile).
    fn site_to_hooks(&self, site: &Site) -> Vec<Hook> {
        let fp = match site.kind {
            SiteKind::Installed => site.as_path().join(INSTALLED_JSON),
            _ => site.as_path().to_path_buf(),
        };
        let mut hooks: Vec<Hook> = fs::read_to_string(fp)
            .map(|c| entries(&c).iter().filter_map(entry_hook).collect())
            .unwrap_or_default();
        hooks.sort();
        hooks.dedup();
        hooks
    }

    /// Each installed package's directory (`install-path`, relative to `vendor/composer`; Composer 1: `vendor/<name>`) and its files.
    fn site_to_artifacts(
        &self,
        site: &Site,
        wanted: &HashSet<&Package>,
    ) -> Vec<(Package, Artifacts)> {
        if site.kind != SiteKind::Installed {
            return Vec::new();
        }
        let vendor = site.as_path();
        let Ok(content) = fs::read_to_string(vendor.join(INSTALLED_JSON)) else {
            return Vec::new();
        };
        entries(&content)
            .iter()
            .filter_map(|e| {
                let p = entry_package(e)?;
                if !wanted.contains(&p) {
                    return None;
                }
                let dir = match e.get("install-path").and_then(JsonValue::as_str) {
                    Some(rel) => vendor.join("composer").join(rel),
                    None => vendor.join(&p.name),
                };
                let dir = fs::canonicalize(&dir).unwrap_or(dir);
                dir.is_dir()
                    .then(|| (p, Artifacts::from_dirs(vec![dir], &[])))
            })
            .collect()
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// `(input, normalized)` from composer/semver `VersionParserTest::successfulNormalizedVersions`.
    const NORMALIZE: &[(&str, &str)] = &[
        ("1.0.0", "1.0.0.0"),
        ("1.2.3.4", "1.2.3.4"),
        ("1.0.0RC1dev", "1.0.0.0-RC1-dev"),
        ("1.0.0-rC15-dev", "1.0.0.0-RC15-dev"),
        ("1.0.0.RC.15-dev", "1.0.0.0-RC15-dev"),
        ("1.0.0-rc1", "1.0.0.0-RC1"),
        ("1.0.0.pl3-dev", "1.0.0.0-patch3-dev"),
        ("1.0-dev", "1.0.0.0-dev"),
        ("0", "0.0.0.0"),
        ("99999", "99999.0.0.0"),
        ("10.4.13-beta", "10.4.13.0-beta"),
        ("10.4.13beta2", "10.4.13.0-beta2"),
        ("10.4.13beta.2", "10.4.13.0-beta2"),
        ("v1.13.11-beta.0", "1.13.11.0-beta0"),
        ("1.13.11.0-beta0", "1.13.11.0-beta0"),
        ("10.4.13-b", "10.4.13.0-beta"),
        ("10.4.13-b5", "10.4.13.0-beta5"),
        ("v1.0.0", "1.0.0.0"),
        ("2010.01", "2010.01.0.0"),
        ("2010.01.02", "2010.01.02.0"),
        ("2010.1.555", "2010.1.555.0"),
        ("2010.10.200", "2010.10.200.0"),
        ("20230131.0.0", "20230131.0.0"),
        ("202301310000.0.0", "202301310000.0.0"),
        ("v20100102", "20100102"),
        ("20100102", "20100102"),
        ("20100102.0", "20100102.0"),
        ("20100102.1.0", "20100102.1.0"),
        ("20100102.0.3", "20100102.0.3"),
        ("100000", "100000"),
        ("2010-01-02-10-20-30.0.3", "2010.01.02.10.20.30.0.3"),
        ("2010-01-02-10-20-30.5", "2010.01.02.10.20.30.5"),
        ("2010-01-02", "2010.01.02"),
        ("2012.06.07", "2012.06.07.0"),
        ("2010-01-02.5", "2010.01.02.5"),
        ("2010.1.555", "2010.1.555.0"),
        ("20100102-203040", "20100102.203040"),
        ("20100102.x-dev", "20100102.9999999.9999999.9999999-dev"),
        (
            "20100102.203040.x-dev",
            "20100102.203040.9999999.9999999-dev",
        ),
        ("20100102203040-10", "20100102203040.10"),
        ("20100102-203040-p1", "20100102.203040-patch1"),
        ("201903.0", "201903.0"),
        ("201903.x-dev", "201903.9999999.9999999.9999999-dev"),
        ("201903.0-p2", "201903.0-patch2"),
        ("dev-master", "dev-master"),
        ("master", "dev-master"),
        ("dev-trunk", "dev-trunk"),
        ("1.x-dev", "1.9999999.9999999.9999999-dev"),
        ("dev-feature-foo", "dev-feature-foo"),
        ("DEV-FOOBAR", "dev-FOOBAR"),
        ("dev-feature/foo", "dev-feature/foo"),
        ("dev-feature+issue-1", "dev-feature+issue-1"),
        ("dev-master as 1.0.0", "dev-master"),
        (
            "dev-load-varnish-only-when-used as ^2.0",
            "dev-load-varnish-only-when-used",
        ),
        (
            "dev-load-varnish-only-when-used@dev as ^2.0@dev",
            "dev-load-varnish-only-when-used",
        ),
        ("1.0.0+foo@dev", "1.0.0.0"),
        (
            "dev-load-varnish-only-when-used@stable",
            "dev-load-varnish-only-when-used",
        ),
        ("1.0.0-beta.5+foo", "1.0.0.0-beta5"),
        ("1.0.0+foo", "1.0.0.0"),
        ("1.0.0-alpha.3.1+foo", "1.0.0.0-alpha3.1"),
        ("1.0.0-alpha2.1+foo", "1.0.0.0-alpha2.1"),
        ("1.0.0-alpha-2.1-3+foo", "1.0.0.0-alpha2.1-3"),
        ("1.0.0+foo as 2.0", "1.0.0.0"),
        ("00.01.03.04", "00.01.03.04"),
        ("000.001.003.004", "000.001.003.004"),
        ("0.000.103.204", "0.000.103.204"),
        ("0700", "0700.0.0.0"),
        ("041.x-dev", "041.9999999.9999999.9999999-dev"),
        ("dev-041.003", "dev-041.003"),
        ("dev-1.0.0-dev<1.0.5-dev", "dev-1.0.0-dev<1.0.5-dev"),
        ("dev-foo bar", "dev-foo bar"),
        (" 1.0.0", "1.0.0.0"),
        ("1.0.0 ", "1.0.0.0"),
    ];

    /// `(satisfied, version, constraint)` from composer/semver `SemverTest::satisfiesProvider`.
    const SATISFIES: &[(bool, &str, &str)] = &[
        (true, "1.2.3", "1.0.0 - 2.0.0"),
        (true, "1.2.3", "^1.2.3+build"),
        (true, "1.3.0", "^1.2.3+build"),
        (true, "2.4.3-alpha", "1.2.3+asdf - 2.4.3+asdf"),
        (true, "1.3.0-beta", ">1.2"),
        (true, "1.2.3-beta", "<=1.2.3"),
        (true, "1.2.3-beta", "^1.2.3"),
        (true, "1.2.3", "1.2.3+asdf - 2.4.3+asdf"),
        (true, "1.0.0", "1.0.0"),
        (true, "1.2.3", "*"),
        (true, "v1.2.3", "*"),
        (true, "1.0.0", ">=1.0.0"),
        (true, "1.0.1", ">=1.0.0"),
        (true, "1.1.0", ">=1.0.0"),
        (true, "1.0.1", ">1.0.0"),
        (true, "1.1.0", ">1.0.0"),
        (true, "2.0.0", "<=2.0.0"),
        (true, "1.9999.9999", "<=2.0.0"),
        (true, "0.2.9", "<=2.0.0"),
        (true, "1.9999.9999", "<2.0.0"),
        (true, "0.2.9", "<2.0.0"),
        (true, "1.0.0", ">= 1.0.0"),
        (true, "1.0.1", ">=  1.0.0"),
        (true, "1.1.0", ">=   1.0.0"),
        (true, "1.0.1", "> 1.0.0"),
        (true, "1.1.0", ">  1.0.0"),
        (true, "2.0.0", "<=   2.0.0"),
        (true, "1.9999.9999", "<= 2.0.0"),
        (true, "0.2.9", "<=  2.0.0"),
        (true, "1.9999.9999", "<    2.0.0"),
        (true, "0.2.9", "<\t2.0.0"),
        (true, "v0.1.97", ">=0.1.97"),
        (true, "0.1.97", ">=0.1.97"),
        (true, "1.2.4", "0.1.20 || 1.2.4"),
        (true, "0.0.0", ">=0.2.3 || <0.0.1"),
        (true, "0.2.3", ">=0.2.3 || <0.0.1"),
        (true, "0.2.4", ">=0.2.3 || <0.0.1"),
        (true, "2.1.3", "2.x.x"),
        (true, "1.2.3", "1.2.x"),
        (true, "2.1.3", "1.2.x || 2.x"),
        (true, "1.2.3", "1.2.x || 2.x"),
        (true, "1.2.3", "x"),
        (true, "2.1.3", "2.*.*"),
        (true, "1.2.3", "1.2.*"),
        (true, "2.1.3", "1.2.* || 2.*"),
        (true, "1.2.3", "1.2.* || 2.*"),
        (true, "1.2.3", "*"),
        (true, "2.9.0", "~2.4"),
        (true, "2.4.5", "~2.4"),
        (true, "1.2.3", "~1"),
        (true, "1.4.7", "~1.0"),
        (true, "1.0.0", ">=1"),
        (true, "1.0.0", ">= 1"),
        (true, "1.2.8", ">1.2"),
        (true, "1.1.1", "<1.2"),
        (true, "1.1.1", "< 1.2"),
        (true, "1.2.3", "~1.2.1 >=1.2.3"),
        (true, "1.2.3", "~1.2.1 =1.2.3"),
        (true, "1.2.3", "~1.2.1 1.2.3"),
        (true, "1.2.3", "~1.2.1 >=1.2.3 1.2.3"),
        (true, "1.2.3", "~1.2.1 1.2.3 >=1.2.3"),
        (true, "1.2.3", "~1.2.1 1.2.3"),
        (true, "1.2.3", ">=1.2.1 1.2.3"),
        (true, "1.2.3", "1.2.3 >=1.2.1"),
        (true, "1.2.3", ">=1.2.3 >=1.2.1"),
        (true, "1.2.3", ">=1.2.1 >=1.2.3"),
        (true, "1.2.8", ">=1.2"),
        (true, "1.8.1", "^1.2.3"),
        (true, "0.1.2", "^0.1.2"),
        (true, "0.1.2", "^0.1"),
        (true, "1.4.2", "^1.2"),
        (true, "1.4.2", "^1.2 ^1"),
        (true, "0.0.1-beta", "^0.0.1-alpha"),
        (false, "2.2.3", "1.0.0 - 2.0.0"),
        (false, "2.0.0", "^1.2.3+build"),
        (false, "1.2.0", "^1.2.3+build"),
        (false, "1.0.0beta", "1"),
        (false, "1.0.0beta", "<1"),
        (false, "1.0.0beta", "< 1"),
        (false, "1.0.1", "1.0.0"),
        (false, "0.0.0", ">=1.0.0"),
        (false, "0.0.1", ">=1.0.0"),
        (false, "0.1.0", ">=1.0.0"),
        (false, "0.0.1", ">1.0.0"),
        (false, "0.1.0", ">1.0.0"),
        (false, "3.0.0", "<=2.0.0"),
        (false, "2.9999.9999", "<=2.0.0"),
        (false, "2.2.9", "<=2.0.0"),
        (false, "2.9999.9999", "<2.0.0"),
        (false, "2.2.9", "<2.0.0"),
        (false, "v0.1.93", ">=0.1.97"),
        (false, "0.1.93", ">=0.1.97"),
        (false, "1.2.3", "0.1.20 || 1.2.4"),
        (false, "0.0.3", ">=0.2.3 || <0.0.1"),
        (false, "0.2.2", ">=0.2.3 || <0.0.1"),
        (false, "1.1.3", "2.x.x"),
        (false, "3.1.3", "2.x.x"),
        (false, "1.3.3", "1.2.x"),
        (false, "3.1.3", "1.2.x || 2.x"),
        (false, "1.1.3", "1.2.x || 2.x"),
        (false, "1.1.3", "2.*.*"),
        (false, "3.1.3", "2.*.*"),
        (false, "1.3.3", "1.2.*"),
        (false, "3.1.3", "1.2.* || 2.*"),
        (false, "1.1.3", "1.2.* || 2.*"),
        (false, "1.1.2", "2"),
        (false, "2.4.1", "2.3"),
        (false, "3.0.0", "~2.4"),
        (false, "2.3.9", "~2.4"),
        (false, "0.2.3", "~1"),
        (false, "1.0.0", "<1"),
        (false, "1.1.1", ">=1.2"),
        (false, "2.0.0beta", "1"),
        (false, "0.5.4-alpha", "~v0.5.4-beta"),
        (false, "1.2.3-beta", "<1.2.3"),
        (false, "2.0.0-alpha", "^1.2.3"),
        (false, "1.2.2", "^1.2.3"),
        (false, "1.1.9", "^1.2"),
    ];

    /// `(a, operator, b, result)` from composer/semver `ComparatorTest::compareProvider`.
    const COMPARE: &[(&str, &str, &str, bool)] = &[
        ("1.25.0", ">", "1.24.0", true),
        ("1.25.0", ">", "1.25.0", false),
        ("1.25.0", ">", "1.26.0", false),
        ("1.25.0", ">=", "1.24.0", true),
        ("1.25.0", ">=", "1.25.0", true),
        ("1.25.0", ">=", "1.26.0", false),
        ("1.25.0", "<", "1.24.0", false),
        ("1.25.0", "<", "1.25.0", false),
        ("1.25.0", "<", "1.26.0", true),
        ("1.25.0-beta2.1", "<", "1.25.0-b.3", true),
        ("1.25.0-b2.1", "<", "1.25.0beta.3", true),
        ("1.25.0-b-2.1", "<", "1.25.0-rc", true),
        ("1.25.0", "<=", "1.24.0", false),
        ("1.25.0", "<=", "1.25.0", true),
        ("1.25.0", "<=", "1.26.0", true),
        ("1.25.0", "==", "1.24.0", false),
        ("1.25.0", "==", "1.25.0", true),
        ("1.25.0", "==", "1.26.0", false),
        ("1.25.0-beta2.1", "==", "1.25.0-b.2.1", true),
        ("1.25.0beta2.1", "==", "1.25.0-b2.1", true),
        ("1.25.0", "=", "1.24.0", false),
        ("1.25.0", "=", "1.25.0", true),
        ("1.25.0", "=", "1.26.0", false),
        ("1.25.0", "!=", "1.24.0", true),
        ("1.25.0", "!=", "1.25.0", false),
        ("1.25.0", "!=", "1.26.0", true),
        ("1.25.0", "<>", "1.24.0", true),
        ("1.25.0", "<>", "1.25.0", false),
        ("1.25.0", "<>", "1.26.0", true),
    ];

    #[test]
    fn test_normalize_vectors_a() {
        for (input, expected) in NORMALIZE {
            assert_eq!(normalize(input).as_deref(), Ok(*expected), "{input:?}");
        }
        for bad in ["", "foo", "1.0.0-meh", "1.0.0.0.0", "feature-foo", "a"] {
            assert!(normalize(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn test_satisfies_vectors_a() {
        for (expected, version, constraint) in SATISFIES {
            let c = ComposerConstraint::parse(constraint)
                .unwrap_or_else(|e| panic!("{constraint:?}: {e}"));
            assert_eq!(
                c.matches(version),
                *expected,
                "{version} satisfies {constraint:?}"
            );
        }
    }

    #[test]
    fn test_compare_vectors_a() {
        for (a, op, b, expected) in COMPARE {
            let (na, nb) = (normalize(a).unwrap(), normalize(b).unwrap());
            let o = version_compare(&na, &nb);
            let got = match *op {
                ">" => o == Ordering::Greater,
                ">=" => o != Ordering::Less,
                "<" => o == Ordering::Less,
                "<=" => o != Ordering::Greater,
                "==" | "=" => o == Ordering::Equal,
                _ => o != Ordering::Equal,
            };
            assert_eq!(got, *expected, "{a} {op} {b}");
        }
    }

    #[test]
    fn test_constraints_a() {
        let m = |c: &str, v: &str| ComposerConstraint::parse(c).unwrap().matches(v);
        // composer's tilde differs from npm's: ~1.2 is >=1.2 <2.0
        assert!(m("~1.2", "1.9.0"));
        assert!(!m("~1.2.3", "1.3.0"));
        // ranges admit pre-releases above their lower bound
        assert!(m("^1.2", "1.3.0-beta1"));
        // branches match only themselves
        assert!(m("dev-master", "dev-master"));
        assert!(!m("^1.0", "dev-master"));
        assert!(m("*", "dev-master"));
        assert!(m(">=5.4", "v5.4.0"));
        assert!(m("^7.4 || ^8.0", "v8.1.2"));
        assert!(m(">=2.0, <3.0", "2.5.0"));
        assert!(ComposerConstraint::parse("~>1.0").is_err());
        assert!(ComposerConstraint::parse("nonsense").is_err());
        assert_eq!(
            ComposerConstraint::parse("1.2.3").unwrap().exact(),
            Some("1.2.3.0")
        );
        assert!(ComposerConstraint::parse("^1.2").unwrap().exact().is_none());
    }

    #[test]
    fn test_composer_version_order_a() {
        let mut v: Vec<&str> =
            vec!["v1.10.0", "1.2.0", "1.2.0-RC1", "1.2.0-beta2", "v2.0.0"];
        v.sort_by_key(|s| ComposerVersion::parse(s).unwrap());
        assert_eq!(
            v,
            vec!["1.2.0-beta2", "1.2.0-RC1", "1.2.0", "v1.10.0", "v2.0.0"]
        );
        assert!(ComposerVersion::parse("dev-main").is_none());
        assert!(ComposerVersion::parse("2.x-dev").is_none());
    }

    const LOCK: &str = r#"{
    "packages": [
        {"name": "Symfony/Http-Kernel", "version": "v5.4.0", "type": "library",
         "source": {"type": "git", "url": "https://github.com/symfony/http-kernel.git", "reference": "abc"}},
        {"name": "lox/xhprof", "version": "dev-master", "type": "library",
         "source": {"type": "git", "url": "https://github.com/lox/xhprof.git", "reference": "c64571f892bda1298bad9c5e94ede0bc3f0e4627"}},
        {"name": "acme/local", "version": "1.0.0", "dist": {"type": "path", "url": "../local"}},
        {"name": "php-http/discovery", "version": "1.19.2", "type": "composer-plugin",
         "extra": {"class": "Http\\Discovery\\Composer\\Plugin"}}
    ],
    "packages-dev": [
        {"name": "phpunit/phpunit", "version": "10.5.0", "type": "library"}
    ]
}"#;

    fn names(packages: &[Package]) -> Vec<String> {
        let mut out: Vec<String> = packages.iter().map(|p| p.to_string()).collect();
        out.sort();
        out
    }

    #[test]
    fn test_packages_from_lock_a() {
        let packages = packages_from(LOCK);
        assert_eq!(
            names(&packages),
            vec![
                "lox/xhprof-dev-master",
                "php-http/discovery-1.19.2",
                "phpunit/phpunit-10.5.0",
                "symfony/http-kernel-v5.4.0",
            ]
        );
        let branch = packages.iter().find(|p| p.name == "lox/xhprof").unwrap();
        let du = branch.direct_url.as_ref().unwrap();
        assert_eq!(du.url, "https://github.com/lox/xhprof.git");
        assert_eq!(
            du.vcs_info.as_ref().unwrap().commit_id,
            "c64571f892bda1298bad9c5e94ede0bc3f0e4627"
        );
        assert!(packages
            .iter()
            .filter(|p| p.name != "lox/xhprof")
            .all(|p| p.direct_url.is_none()));
    }

    #[test]
    fn test_is_branch_version_a() {
        assert!(is_branch_version("dev-master"));
        assert!(is_branch_version("2.x-dev"));
        assert!(is_branch_version("dev-feature/foo"));
        assert!(!is_branch_version("v5.4.0"));
        assert!(!is_branch_version("1.0.0-beta1"));
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

    fn write(fp: &Path, content: &str) {
        fs::create_dir_all(fp.parent().unwrap()).unwrap();
        fs::write(fp, content).unwrap();
    }

    #[test]
    fn test_lockfile_project_a() {
        let dir = tempdir().unwrap();
        let p = dir.path();
        write(&p.join(COMPOSER_JSON), "{}");
        write(&p.join(COMPOSER_LOCK), LOCK);
        let d = PhpScanner.detect(p, &items_of(p));
        assert_eq!(d.origins.len(), 1);
        assert_eq!(d.skip_children, vec!["vendor"]);
        let o = PhpScanner.normalize_origin(&p.join(COMPOSER_LOCK)).unwrap();
        let sites = PhpScanner.origin_to_sites(
            &o,
            &ScanConfig::new(false, false),
            FlagLog(false),
        );
        assert_eq!(sites[0].kind, SiteKind::Lockfile);
        assert_eq!(PhpScanner.site_to_packages(&sites[0]).len(), 4);
        let hooks = PhpScanner.site_to_hooks(&sites[0]);
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].name, "php-http/discovery-1.19.2 composer-plugin");
        assert_eq!(hooks[0].detail, "Http\\Discovery\\Composer\\Plugin");

        // a composer.json alone is not a project
        let bare = tempdir().unwrap();
        write(&bare.path().join(COMPOSER_JSON), "{}");
        assert!(PhpScanner
            .detect(bare.path(), &items_of(bare.path()))
            .origins
            .is_empty());
        assert!(PhpScanner.normalize_origin(bare.path()).is_err());
    }

    const INSTALLED: &str = r#"{"packages": [
        {"name": "psr/log", "version": "3.0.0", "type": "library", "install-path": "../psr/log"},
        {"name": "aws/aws-crt-php", "version": "v1.2.7", "type": "library", "install-path": "../aws/aws-crt-php"}
    ], "dev": false, "dev-package-names": []}"#;

    #[test]
    fn test_installed_project_a() {
        let dir = tempdir().unwrap();
        let p = dir.path();
        // the installed vendor directory takes precedence over the lockfile
        write(
            &p.join(COMPOSER_JSON),
            r#"{"config": {"vendor-dir": "lib"}}"#,
        );
        write(&p.join(COMPOSER_LOCK), LOCK);
        write(&p.join("lib").join(INSTALLED_JSON), INSTALLED);
        write(&p.join("lib/psr/log/src/Logger.php"), "<?php");
        write(&p.join("lib/psr/log/composer.json"), "{}");
        let d = PhpScanner.detect(p, &items_of(p));
        assert_eq!(d.origins.len(), 1);
        let sites = PhpScanner.origin_to_sites(
            &d.origins[0],
            &ScanConfig::new(false, false),
            FlagLog(false),
        );
        assert_eq!(sites[0].kind, SiteKind::Installed);
        assert_eq!(sites[0].as_path(), p.join("lib"));
        let packages = PhpScanner.site_to_packages(&sites[0]);
        assert_eq!(
            names(&packages),
            vec!["aws/aws-crt-php-v1.2.7", "psr/log-3.0.0"]
        );
        let wanted: HashSet<&Package> = packages.iter().collect();
        let arts = PhpScanner.site_to_artifacts(&sites[0], &wanted);
        // aws/aws-crt-php has no directory here
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].1.files.len(), 2);
    }

    #[test]
    fn test_vendor_dir_dot_and_standalone_a() {
        // vendor-dir "." (as nextcloud/3rdparty): the project is its own vendor directory, so it is pruned
        let dir = tempdir().unwrap();
        let p = dir.path();
        write(&p.join(COMPOSER_JSON), r#"{"config": {"vendor-dir": "."}}"#);
        write(&p.join(INSTALLED_JSON), INSTALLED);
        let d = PhpScanner.detect(p, &items_of(p));
        assert!(d.prune);
        assert_eq!(d.origins.len(), 1);

        // a vendor directory shipped without its project
        let dir = tempdir().unwrap();
        let v = dir.path().join("plugin").join("vendor");
        write(&v.join(INSTALLED_JSON), INSTALLED);
        let d = PhpScanner.detect(&v, &items_of(&v));
        assert!(d.prune);
        let sites = PhpScanner.origin_to_sites(
            &d.origins[0],
            &ScanConfig::new(false, false),
            FlagLog(false),
        );
        assert_eq!(sites[0].kind, SiteKind::Installed);
        assert_eq!(PhpScanner.site_to_packages(&sites[0]).len(), 2);
    }
}
