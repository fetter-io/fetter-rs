use std::cmp::Ordering;
use std::fmt;
use std::hash::Hash;
use std::hash::Hasher;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

//------------------------------------------------------------------------------
#[derive(Debug, Eq, Ord, PartialEq, PartialOrd, Clone, Hash, Serialize, Deserialize)]
enum VersionPart {
    Number(u32),
    Text(String),
}

//------------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub struct VersionSpec(Vec<VersionPart>);

impl Serialize for VersionSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for VersionSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let version_str = String::deserialize(deserializer)?;
        Ok(VersionSpec::new(&version_str))
    }
}

impl VersionSpec {
    /// Main constructor.
    pub fn new(version_str: &str) -> Self {
        let parts = version_str
            .split('.')
            .map(|part| {
                if let Ok(number) = part.parse::<u32>() {
                    VersionPart::Number(number)
                } else {
                    VersionPart::Text(part.to_string())
                }
            })
            .collect();
        VersionSpec(parts)
    }
    // https://packaging.python.org/en/latest/specifications/version-specifiers/#compatible-release
    pub(crate) fn is_compatible(&self, other: &Self) -> bool {
        if other < self {
            return false;
        }
        let mut ub = self.0.clone(); // upper bound
        let ub_len = ub.len();
        let mut numeric_count = 0;
        let numeric_total = self
            .0
            .iter()
            .filter(|part| matches!(part, VersionPart::Number(_)))
            .count();
        if numeric_total == 1 {
            // spec says this MUST NOT be used with a single segment version number such as ~=1; we permit, however, direct equivalent versions
            return self == other;
        }
        // try to find the second to last numeric component and increment it
        for i in 0..ub_len {
            if let VersionPart::Number(n) = ub[i] {
                numeric_count += 1;
                if numeric_count == numeric_total - 1 {
                    ub[i] = VersionPart::Number(n + 1);
                    ub.truncate(i + 1); // remove everything after
                    break;
                }
            }
        }
        other < &VersionSpec(ub)
    }

    // https://packaging.python.org/en/latest/specifications/version-specifiers/#arbitrary-equality
    pub(crate) fn is_arbitrary_equal(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }

    // https://python-poetry.org/docs/dependency-specification/#caret-requirements
    pub(crate) fn is_caret(&self, other: &Self) -> bool {
        if other < self {
            return false;
        }
        let mut ub = self.0.clone(); // upper bound
        let ub_len = ub.len();
        let mut numeric_count = 0;

        // increment the first non-zero, or the last
        for i in 0..ub_len {
            if let VersionPart::Number(n) = ub[i] {
                numeric_count += 1;
                if n != 0 || (numeric_count == ub_len) {
                    ub[i] = VersionPart::Number(n + 1);
                    ub.truncate(i + 1); // remove everything after
                    break;
                }
            }
        }
        other < &VersionSpec(ub)
    }

    // https://python-poetry.org/docs/dependency-specification/#tilde-requirements
    pub(crate) fn is_tilde(&self, other: &Self) -> bool {
        if other < self {
            return false;
        }
        let mut ub = self.0.clone(); // upper bound
        let ub_len = ub.len();
        let mut numeric_count = 0;

        // find the second numeric component and increment it, or if length 1 increment it
        for i in 0..ub_len {
            if let VersionPart::Number(n) = ub[i] {
                numeric_count += 1;
                if numeric_count == 2 || (numeric_count == 1 && ub_len == 1) {
                    ub[i] = VersionPart::Number(n + 1);
                    ub.truncate(i + 1); // remove everything after
                    break;
                }
            }
        }
        other < &VersionSpec(ub)
    }
}
impl fmt::Display for VersionSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let version_string = self
            .0
            .iter()
            .map(|part| match part {
                VersionPart::Number(num) => num.to_string(),
                VersionPart::Text(text) => text.clone(),
            })
            .collect::<Vec<_>>()
            .join(".");
        write!(f, "{version_string}")
    }
}

// This hash implementation does not treat wildcards "*" special, which may be an issue as PartialEq does
impl Hash for VersionSpec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for part in &self.0 {
            part.hash(state);
        }
    }
}

// This ordering implemenation is handling wild cards and zero-padding, but may not yet be handling "post" release correctly
// https://packaging.python.org/en/latest/specifications/version-specifiers/#post-releases
impl Ord for VersionSpec {
    fn cmp(&self, other: &Self) -> Ordering {
        let max_len = self.0.len().max(other.0.len());
        for i in 0..max_len {
            // extend to max with zero padding
            let self_part = self.0.get(i).unwrap_or(&VersionPart::Number(0));
            let other_part = other.0.get(i).unwrap_or(&VersionPart::Number(0));

            let ordering = match (self_part, other_part) {
                (VersionPart::Number(a), VersionPart::Number(b)) => a.cmp(b),
                (VersionPart::Text(a), VersionPart::Text(b)) => {
                    if a == "*" || b == "*" {
                        Ordering::Equal
                    } else {
                        a.cmp(b)
                    }
                }
                (VersionPart::Number(_), VersionPart::Text(b)) => {
                    if b == "*" {
                        Ordering::Equal
                    } else {
                        Ordering::Greater // numbers are always greater than text
                    }
                }
                (VersionPart::Text(a), VersionPart::Number(_)) => {
                    if a == "*" {
                        Ordering::Equal
                    } else {
                        Ordering::Less
                    }
                }
            };
            if ordering != Ordering::Equal {
                return ordering; // else, continue iteration
            }
        }
        // self.0.len().cmp(&other.0.len())
        Ordering::Equal
    }
}
impl PartialOrd for VersionSpec {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for VersionSpec {
    fn eq(&self, other: &Self) -> bool {
        let max_len = self.0.len().max(other.0.len());
        for i in 0..max_len {
            // extend to max with zero padding
            let self_part = self.0.get(i).unwrap_or(&VersionPart::Number(0));
            let other_part = other.0.get(i).unwrap_or(&VersionPart::Number(0));

            match (self_part, other_part) {
                // if wildcard "*" both equal
                (VersionPart::Text(a), VersionPart::Text(b)) if a == "*" || b == "*" => {
                    continue
                }
                (VersionPart::Text(a), VersionPart::Number(_)) if a == "*" => continue,
                (VersionPart::Number(_), VersionPart::Text(b)) if b == "*" => continue,
                // parts must match exactly
                (VersionPart::Number(a), VersionPart::Number(b)) if a != b => {
                    return false
                }
                (VersionPart::Text(a), VersionPart::Text(b)) if a != b => return false,
                // not equal
                (VersionPart::Number(_), VersionPart::Text(_)) => return false,
                (VersionPart::Text(_), VersionPart::Number(_)) => return false,
                _ => {} // continue
            }
        }
        true
    }
}

impl Eq for VersionSpec {}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {

    use super::*;
    use serde_json;

    #[test]
    fn test_version_spec_a() {
        assert_eq!(VersionSpec::new("2.2"), VersionSpec::new("2.2"));
        assert_eq!(VersionSpec::new("2.*"), VersionSpec::new("2.2"));
        assert_eq!(VersionSpec::new("2.2"), VersionSpec::new("2.*"));
    }
    #[test]
    fn test_version_spec_b() {
        assert_eq!(VersionSpec::new("2.*.1"), VersionSpec::new("2.2.1"));
        assert_ne!(VersionSpec::new("2.*.1"), VersionSpec::new("2.2.2"));
    }
    #[test]
    fn test_version_spec_c() {
        // NOTE: not sure these falses are what we want
        assert_eq!(VersionSpec::new("2.*") > VersionSpec::new("2.2.1"), false);
        assert_eq!(VersionSpec::new("2.2") > VersionSpec::new("2.*"), false);
    }
    #[test]
    fn test_version_spec_d() {
        assert_eq!(VersionSpec::new("2.1") != VersionSpec::new("2.2"), true);
        assert_eq!(VersionSpec::new("2.2") != VersionSpec::new("2.2"), false);
        assert_eq!(VersionSpec::new("2.2.0") != VersionSpec::new("2.2"), false);
    }
    #[test]
    fn test_version_spec_e() {
        assert_eq!(VersionSpec::new("1.7.1") > VersionSpec::new("1.7"), true);
        assert_eq!(VersionSpec::new("1.7.1") < VersionSpec::new("1.8"), true);
        assert_eq!(
            VersionSpec::new("1.7.0.post1") > VersionSpec::new("1.7"),
            false
        );
        assert_eq!(
            VersionSpec::new("1.7.1") > VersionSpec::new("1.7.post1"),
            true
        );
        // this is supposed to be true: >1.7.post2 will allow 1.7.1 and 1.7.0.post3 but not 1.7.0.
        // assert_eq!(VersionSpec::new("1.7.0") > VersionSpec::new("1.7.post1"), false);
    }
    //--------------------------------------------------------------------------
    #[test]
    fn test_version_is_compatible_a() {
        assert_eq!(
            VersionSpec::new("2.2").is_compatible(&VersionSpec::new("2.2")),
            true
        );
        assert_eq!(
            VersionSpec::new("2.2").is_compatible(&VersionSpec::new("3.2")),
            false
        );
        assert_eq!(
            VersionSpec::new("2.2").is_compatible(&VersionSpec::new("2.2.3.9")),
            true
        );
    }
    #[test]
    fn test_version_is_compatible_b() {
        assert_eq!(
            VersionSpec::new("2.2-2").is_arbitrary_equal(&VersionSpec::new("2.2-2")),
            true
        );
        assert_eq!(
            VersionSpec::new("foobar").is_arbitrary_equal(&VersionSpec::new("foobar")),
            true
        );
        assert_eq!(
            VersionSpec::new("foobar").is_arbitrary_equal(&VersionSpec::new("foobars")),
            false
        );
        assert_eq!(
            VersionSpec::new("1.0")
                .is_arbitrary_equal(&VersionSpec::new("1.0+downstream1")),
            false
        );
    }
    #[test]
    fn test_version_is_compatible_c() {
        assert_eq!(
            VersionSpec::new("2.2.1").is_compatible(&VersionSpec::new("2.2.0")),
            false
        );
        assert_eq!(
            VersionSpec::new("2.2.1").is_compatible(&VersionSpec::new("2.2.9")),
            true
        );
        assert_eq!(
            VersionSpec::new("2.2.1").is_compatible(&VersionSpec::new("2.3")),
            false
        );
    }
    #[test]
    fn test_version_is_compatible_d() {
        assert_eq!(
            VersionSpec::new("1.4.5.0").is_compatible(&VersionSpec::new("2.2.0")),
            false
        );
        assert_eq!(
            VersionSpec::new("1.4.5.0").is_compatible(&VersionSpec::new("1.4.5.9")),
            true
        );
        assert_eq!(
            VersionSpec::new("1.4.5.0").is_compatible(&VersionSpec::new("1.4.6.0")),
            false
        );
    }
    #[test]
    fn test_version_is_compatible_e() {
        assert_eq!(
            VersionSpec::new("2").is_compatible(&VersionSpec::new("2.0.0.0")),
            true
        );
        assert_eq!(
            VersionSpec::new("2").is_compatible(&VersionSpec::new("2.1")),
            false
        );
        assert_eq!(
            VersionSpec::new("2").is_compatible(&VersionSpec::new("3")),
            false
        );
    }
    //--------------------------------------------------------------------------
    #[test]
    fn test_version_spec_json_a() {
        let vs1 = VersionSpec::new("2.2.3rc2");
        let json = serde_json::to_string(&vs1).unwrap();
        assert_eq!(json, "\"2.2.3rc2\"");
        let vs2: VersionSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(vs2, VersionSpec::new("2.2.3rc2"));
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_version_spec_tilde_a() {
        assert_eq!(
            VersionSpec::new("1.7.1").is_tilde(&VersionSpec::new("1.7.2")),
            true
        );
        assert_eq!(
            VersionSpec::new("1.7.1").is_tilde(&VersionSpec::new("1.7")),
            false
        );
        assert_eq!(
            VersionSpec::new("1.7.1").is_tilde(&VersionSpec::new("1.8")),
            false
        );
        assert_eq!(
            VersionSpec::new("1.7.1").is_tilde(&VersionSpec::new("2")),
            false
        );
        assert_eq!(
            VersionSpec::new("1.7.1").is_tilde(&VersionSpec::new("0.8")),
            false
        );
    }
    #[test]
    fn test_version_spec_tilde_b() {
        assert_eq!(
            VersionSpec::new("1.2").is_tilde(&VersionSpec::new("1.2.1")),
            true
        );
        assert_eq!(
            VersionSpec::new("1.2").is_tilde(&VersionSpec::new("1.2.9.1")),
            true
        );
        assert_eq!(
            VersionSpec::new("1.2").is_tilde(&VersionSpec::new("1.8")),
            false
        );
        assert_eq!(
            VersionSpec::new("1.2").is_tilde(&VersionSpec::new("2")),
            false
        );
        assert_eq!(
            VersionSpec::new("1.2").is_tilde(&VersionSpec::new("1.3")),
            false
        );
    }
    #[test]
    fn test_version_spec_tilde_c() {
        assert_eq!(
            VersionSpec::new("2").is_tilde(&VersionSpec::new("2.1")),
            true
        );
        assert_eq!(
            VersionSpec::new("2").is_tilde(&VersionSpec::new("2.9.1")),
            true
        );
        assert_eq!(
            VersionSpec::new("2").is_tilde(&VersionSpec::new("1.8")),
            false
        );
        assert_eq!(
            VersionSpec::new("2").is_tilde(&VersionSpec::new("3")),
            false
        );
        assert_eq!(
            VersionSpec::new("2").is_tilde(&VersionSpec::new("4")),
            false
        );
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_version_spec_caret_a() {
        assert_eq!(
            VersionSpec::new("1.7.1").is_caret(&VersionSpec::new("1.7.2")),
            true
        );
        assert_eq!(
            VersionSpec::new("1.7.1").is_caret(&VersionSpec::new("1.20")),
            true
        );
        assert_eq!(
            VersionSpec::new("1.7.1").is_caret(&VersionSpec::new("1.6")),
            false
        );
        assert_eq!(
            VersionSpec::new("1.7.1").is_caret(&VersionSpec::new("2")),
            false
        );
        assert_eq!(
            VersionSpec::new("1.7.1").is_caret(&VersionSpec::new("0.8")),
            false
        );
    }
    #[test]
    fn test_version_spec_caret_b() {
        assert_eq!(
            VersionSpec::new("1").is_caret(&VersionSpec::new("1.7.2")),
            true
        );
        assert_eq!(
            VersionSpec::new("1").is_caret(&VersionSpec::new("1.0.1")),
            true
        );
        assert_eq!(
            VersionSpec::new("1").is_caret(&VersionSpec::new("1.6")),
            true
        );
        assert_eq!(
            VersionSpec::new("1").is_caret(&VersionSpec::new("2")),
            false
        );
        assert_eq!(
            VersionSpec::new("1").is_caret(&VersionSpec::new("0.8")),
            false
        );
    }
    #[test]
    fn test_version_spec_caret_c() {
        assert_eq!(
            VersionSpec::new("0").is_caret(&VersionSpec::new("1.7.2")),
            false
        );
        assert_eq!(
            VersionSpec::new("0").is_caret(&VersionSpec::new("1.0.1")),
            false
        );
        assert_eq!(
            VersionSpec::new("0").is_caret(&VersionSpec::new("0.6")),
            true
        );
        assert_eq!(
            VersionSpec::new("0").is_caret(&VersionSpec::new("0.1.2")),
            true
        );
        assert_eq!(
            VersionSpec::new("0").is_caret(&VersionSpec::new("0.8")),
            true
        );
    }
    #[test]
    fn test_version_spec_caret_d() {
        assert_eq!(
            VersionSpec::new("0.0.3").is_caret(&VersionSpec::new("1.7.2")),
            false
        );
        assert_eq!(
            VersionSpec::new("0.0.3").is_caret(&VersionSpec::new("0.0.2")),
            false
        );
        assert_eq!(
            VersionSpec::new("0.0.3").is_caret(&VersionSpec::new("0.0.4")),
            false
        );
        assert_eq!(
            VersionSpec::new("0.0.3").is_caret(&VersionSpec::new("0.0.3.1")),
            true
        );
        assert_eq!(
            VersionSpec::new("0.0.3").is_caret(&VersionSpec::new("0.0.3.9")),
            true
        );
    }
    #[test]
    fn test_version_spec_caret_e() {
        assert_eq!(
            VersionSpec::new("0.0").is_caret(&VersionSpec::new("0.0.2")),
            true
        );
        assert_eq!(
            VersionSpec::new("0.0").is_caret(&VersionSpec::new("0.0.2.5")),
            true
        );
        assert_eq!(
            VersionSpec::new("0.0").is_caret(&VersionSpec::new("0.1.0")),
            false
        );
        assert_eq!(
            VersionSpec::new("0.0").is_caret(&VersionSpec::new("1")),
            false
        );
    }
}
