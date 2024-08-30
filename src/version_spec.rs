use std::cmp::Ordering;



//------------------------------------------------------------------------------
#[derive(Debug, Eq, Ord, PartialEq, PartialOrd, Clone, Hash)]
enum VersionPart {
    Number(u32),
    Text(String),
}

//------------------------------------------------------------------------------
#[derive(Debug, Clone, Hash)]
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
    pub fn is_major_compatible(&self, other: &Self) -> bool {
        if let (Some(VersionPart::Number(self_major)), Some(VersionPart::Number(other_major))) =
            (self.0.get(0), other.0.get(0)) {
            return self_major == other_major;
        }
        false
    }
    // pub fn is_compatible(&self, other: &Self) -> bool {
    //     for (self_part, other_part) in self.0.iter().zip(&other.0) {
    //         match (self_part, other_part) {
    //             (VersionPart::Number(a), VersionPart::Number(b)) => {
    //                 if a != b {
    //                     return false;
    //                 }
    //             }
    //             (VersionPart::Text(a), VersionPart::Text(b)) => {
    //                 if a != "*" && b != "*" && a != b {
    //                     return false;
    //                 }
    //             }
    //             // If one part is a number and the other is text, handle "*" as a wildcard
    //             (VersionPart::Number(_), VersionPart::Text(b)) if b == "*" => continue,
    //             (VersionPart::Text(a), VersionPart::Number(_)) if a == "*" => continue,
    //             // Incompatible if none of the above conditions hold
    //             _ => return false,
    //         }
    //     }
    //     true
    // }

}
impl Ord for VersionSpec {
    fn cmp(&self, other: &Self) -> Ordering {
        // println!("cmp: {:?} {:?}", self, other);

        for (self_part, other_part) in self.0.iter().zip(&other.0) {
            // println!("here: {:?} {:?}", self_part, other_part);

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
                return ordering;
            }
        }
        self.0.len().cmp(&other.0.len())
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
            // println!("cmp: {:?} {:?}", self_part, other_part);

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

}