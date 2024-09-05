use std::cmp::Ordering;
use serde::{Serialize, Deserialize};


//------------------------------------------------------------------------------
#[derive(Debug, Eq, Ord, PartialEq, PartialOrd, Clone, Hash, Serialize, Deserialize)]
enum VersionPart {
    Number(u32),
    Text(String),
}

//------------------------------------------------------------------------------
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub(crate) struct VersionSpec(Vec<VersionPart>);

impl VersionSpec {
    pub(crate) fn new(version_str: &str) -> Self {
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
    pub(crate) fn to_string(&self) -> String {
        self.0
            .iter()
            .map(|part| match part {
                    VersionPart::Number(num) => num.to_string(),
                    VersionPart::Text(text) => text.clone(),
            })
            .collect::<Vec<_>>()
            .join(".")
    }
    pub fn is_compatible(&self, other: &Self) -> bool {
        // https://packaging.python.org/en/latest/specifications/version-specifiers/#compatible-release
        if let (Some(VersionPart::Number(self_major)), Some(VersionPart::Number(other_major))) =
            (self.0.get(0), other.0.get(0)) {
            return self_major == other_major;
        }
        false
    }
    pub fn is_arbitrary_equal(&self, other: &Self) -> bool {
        // https://packaging.python.org/en/latest/specifications/version-specifiers/#arbitrary-equality
        self.to_string() == other.to_string()
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
                // If either part is a wildcard "*", consider them equal
                (VersionPart::Text(a), VersionPart::Text(b)) if a == "*" || b == "*" => continue,
                (VersionPart::Text(a), VersionPart::Number(_)) if a == "*" => continue,
                (VersionPart::Number(_), VersionPart::Text(b)) if b == "*" => continue,
                // Otherwise, parts must match exactly
                (VersionPart::Number(a), VersionPart::Number(b)) if a != b => return false,
                (VersionPart::Text(a), VersionPart::Text(b)) if a != b => return false,
                // If types differ and no wildcard is involved, they are not equal
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
        assert_eq!(VersionSpec::new("1.7.0.post1") > VersionSpec::new("1.7"), false);
        assert_eq!(VersionSpec::new("1.7.1") > VersionSpec::new("1.7.post1"), true);
        // this is supposed to be true: >1.7.post2 will allow 1.7.1 and 1.7.0.post3 but not 1.7.0.
        // assert_eq!(VersionSpec::new("1.7.0") > VersionSpec::new("1.7.post1"), false);
    }
    #[test]
    fn test_version_is_major_compatible_a() {
        assert_eq!(VersionSpec::new("2.2").is_compatible(&VersionSpec::new("2.2")), true);
        assert_eq!(VersionSpec::new("2.2").is_compatible(&VersionSpec::new("3.2")), false);
        assert_eq!(VersionSpec::new("2.2").is_compatible(&VersionSpec::new("2.2.3.9")), true);
    }
    #[test]
    fn test_version_is_major_compatible_b() {
        assert_eq!(VersionSpec::new("2.2-2").is_arbitrary_equal(&VersionSpec::new("2.2-2")), true);
        assert_eq!(VersionSpec::new("foobar").is_arbitrary_equal(&VersionSpec::new("foobar")), true);
        assert_eq!(VersionSpec::new("foobar").is_arbitrary_equal(&VersionSpec::new("foobars")), false);
        assert_eq!(VersionSpec::new("1.0").is_arbitrary_equal(&VersionSpec::new("1.0+downstream1")), false);
    }
    //--------------------------------------------------------------------------
    #[test]
    fn test_version_spec_json_a() {
        let vs1 = VersionSpec::new("2.2.3rc2");
        let json = serde_json::to_string(&vs1).unwrap();
        assert_eq!(json, "[{\"Number\":2},{\"Number\":2},{\"Text\":\"3rc2\"}]");
        let vs2: VersionSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(vs2, VersionSpec::new("2.2.3rc2"));
    }
}