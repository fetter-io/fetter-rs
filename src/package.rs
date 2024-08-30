use std::fmt;
use std::cmp::Ordering;

use crate::version_spec::VersionSpec;


//------------------------------------------------------------------------------
#[derive(PartialEq, Eq, Hash, Clone)]
pub(crate) struct Package {
    name: String,
    version: String,
    version_spec: VersionSpec,
}
impl Package {
    pub(crate) fn new(input: &str) -> Option<Self> {
        if input.ends_with(".dist-info") {
            let trimmed_input = input.trim_end_matches(".dist-info");
            let parts: Vec<&str> = trimmed_input.split('-').collect();
            if parts.len() >= 2 {
                let name = parts[..parts.len() - 1].join("-");
                let version = parts.last()?.to_string();
                let version_spec = VersionSpec::new(&version);
                return Some(Package { name, version, version_spec });
            }
        }
        None
    }
}
impl Ord for Package {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name
            .cmp(&other.name)
            .then_with(|| self.version_spec.cmp(&other.version_spec))
    }
}
impl PartialOrd for Package {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl fmt::Display for Package {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "<Package: {} version: {} version_spec: {:?}>",
            self.name, self.version, self.version_spec
        )
    }
}
impl fmt::Debug for Package {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}



#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_package_a() {
        let p1 = Package::new("matplotlib-3.9.0.dist-info").unwrap();
        assert_eq!(p1.name, "matplotlib");
        assert_eq!(p1.version, "3.9.0");
    }

    #[test]
    fn test_package_b() {
        assert_eq!(Package::new("matplotlib-3.9.0.dist-in"), None);
    }

    #[test]
    fn test_package_c() {
        let p1 = Package::new("xarray-0.21.1.dist-info").unwrap();
        let p2 = Package::new("xarray-2024.6.0.dist-info").unwrap();
        let p3 = Package::new("xarray-2024.6.0.dist-info").unwrap();

        assert_eq!(p2 > p1, true);
        assert_eq!(p1 < p2, true);
        assert_eq!(p1 == p3, false);
        assert_eq!(p2== p3, true);
    }


}