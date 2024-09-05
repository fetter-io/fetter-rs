use std::fmt;
use std::cmp::Ordering;

use crate::version_spec::VersionSpec;

//------------------------------------------------------------------------------
// A Package is package release artifact, representing one specific version installed. This differs from a DepSpec, which might refer to a range of acceptable versions.
#[derive(PartialEq, Eq, Hash, Clone)]
pub(crate) struct Package {
    pub(crate) name: String,
    pub(crate) version: VersionSpec,
}
impl Package {
    pub(crate) fn from_name_and_version(name: &str, version: &str) -> Option<Self> {
        return Some(Package { name: name.to_string(), version: VersionSpec::new(version) });
    }
    pub(crate) fn from_dist_info(input: &str) -> Option<Self> {
        if input.ends_with(".dist-info") {
            let trimmed_input = input.trim_end_matches(".dist-info");
            let parts: Vec<&str> = trimmed_input.split('-').collect();
            if parts.len() >= 2 {
                let name = parts[..parts.len() - 1].join("-");
                let version = parts.last()?;
                return Self::from_name_and_version(&name, version);
            }
        }
        None
    }
    pub(crate) fn to_string(&self) -> String {
        format!("{}-{}", self.name, self.version.to_string())
    }
}
impl Ord for Package {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name
            .cmp(&other.name)
            .then_with(|| self.version.cmp(&other.version))
    }
}
impl PartialOrd for Package {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl fmt::Display for Package {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<Package: {}>", self.to_string())
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
        let p1 = Package::from_dist_info("matplotlib-3.9.0.dist-info").unwrap();
        assert_eq!(p1.name, "matplotlib");
        assert_eq!(p1.version.to_string(), "3.9.0");
    }

    #[test]
    fn test_package_b() {
        assert_eq!(Package::from_dist_info("matplotlib-3.9.0.dist-in"), None);
    }

    #[test]
    fn test_package_c() {
        let p1 = Package::from_dist_info("xarray-0.21.1.dist-info").unwrap();
        let p2 = Package::from_dist_info("xarray-2024.6.0.dist-info").unwrap();
        let p3 = Package::from_dist_info("xarray-2024.6.0.dist-info").unwrap();

        assert_eq!(p2 > p1, true);
        assert_eq!(p1 < p2, true);
        assert_eq!(p1 == p3, false);
        assert_eq!(p2== p3, true);
    }
    #[test]
    fn test_package_to_string_a() {
        let p1 = Package::from_dist_info("matplotlib-3.9.0.dist-info").unwrap();
        assert_eq!(p1.to_string(), "matplotlib-3.9.0");
    }
    #[test]
    fn test_package_to_string_b() {
        let p1 = Package::from_name_and_version("matplotlib", "3.9.0").unwrap();
        assert_eq!(p1.to_string(), "matplotlib-3.9.0");
    }
    #[test]
    fn test_package_to_string_c() {
        let p1 = Package::from_name_and_version("numpy", "2.1.2").unwrap();
        assert_eq!(p1.to_string(), "numpy-2.1.2");
        assert_eq!(format!("{}", p1), "<Package: numpy-2.1.2>");
    }


}