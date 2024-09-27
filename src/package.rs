use std::cmp::Ordering;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::package_durl::DirectURL;
use crate::util::name_to_key;
use crate::version_spec::VersionSpec;

//------------------------------------------------------------------------------
// A Package is package artifact, representing a specific version installed on a file system. This differs from a DepSpec, which might refer to a range of acceptable versions without a specific artifact.
#[derive(PartialEq, Eq, Hash, Clone, Serialize, Deserialize)]
pub(crate) struct Package {
    pub(crate) name: String,
    pub(crate) key: String,
    pub(crate) version: VersionSpec,
    pub(crate) direct_url: Option<DirectURL>,
}
impl Package {
    pub(crate) fn from_name_version_durl(
        name: &str,
        version: &str,
        direct_url: Option<DirectURL>,
    ) -> Option<Self> {
        let ns = name.to_string();
        Some(Package {
            key: name_to_key(&ns),
            name: ns,
            version: VersionSpec::new(version),
            direct_url: direct_url,
        })
    }
    /// Create a Package from a dist_info string.
    pub(crate) fn from_dist_info(
        input: &str,
        direct_url: Option<DirectURL>,
    ) -> Option<Self> {
        let trimmed_input = input.trim_end_matches(".dist-info");
        let parts: Vec<&str> = trimmed_input.split('-').collect();
        if parts.len() >= 2 {
            // NOTE: we expect that dist-info based names have already normalized hyphens to underscores, joingwith '-' may not be meaningful here
            let name = parts[..parts.len() - 1].join("-");
            let version = parts.last()?;
            return Self::from_name_version_durl(&name, version, direct_url);
        }
        None
    }
    /// Create a Package from a dist_info file path.
    pub(crate) fn from_file_path(file_path: &PathBuf) -> Option<Self> {
        let file_name = file_path.file_name().and_then(|name| name.to_str())?;
        if file_name.ends_with(".dist-info") && file_path.is_dir() {
            let durl_fp = file_path.join("direct_url.json");
            let durl = if durl_fp.is_file() {
                DirectURL::from_file(&durl_fp).ok()
            } else {
                None
            };
            return Self::from_dist_info(file_name, durl);
        }
        None
    }
}

// A case insensitive ordering.
impl Ord for Package {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name
            .to_lowercase()
            .cmp(&other.name.to_lowercase())
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
        write!(f, "{}-{}", self.name, self.version)
    }
}
impl fmt::Debug for Package {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<Package: {}-{}>", self.name, self.version)
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_package_a() {
        let p1 = Package::from_dist_info("matplotlib-3.9.0.dist-info", None).unwrap();
        assert_eq!(p1.name, "matplotlib");
        assert_eq!(p1.version.to_string(), "3.9.0");
    }

    #[test]
    fn test_package_b() {
        assert_eq!(
            Package::from_dist_info("matplotlib3.9.0.distin", None),
            None
        );
    }

    #[test]
    fn test_package_c() {
        let p1 = Package::from_dist_info("xarray-0.21.1.dist-info", None).unwrap();
        let p2 = Package::from_dist_info("xarray-2024.6.0.dist-info", None).unwrap();
        let p3 = Package::from_dist_info("xarray-2024.6.0.dist-info", None).unwrap();

        assert_eq!(p2 > p1, true);
        assert_eq!(p1 < p2, true);
        assert_eq!(p1 == p3, false);
        assert_eq!(p2 == p3, true);
    }
    #[test]
    fn test_package_to_string_a() {
        let p1 = Package::from_dist_info("matplotlib-3.9.0.dist-info", None).unwrap();
        assert_eq!(p1.to_string(), "matplotlib-3.9.0");
    }
    #[test]
    fn test_package_to_string_b() {
        let p1 = Package::from_name_version_durl("matplotlib", "3.9.0", None).unwrap();
        assert_eq!(p1.to_string(), "matplotlib-3.9.0");
    }
    #[test]
    fn test_package_to_string_c() {
        let p1 = Package::from_name_version_durl("numpy", "2.1.2", None).unwrap();
        assert_eq!(p1.to_string(), "numpy-2.1.2");
        assert_eq!(format!("{:?}", p1), "<Package: numpy-2.1.2>");
    }
    //--------------------------------------------------------------------------
    #[test]
    fn test_package_json_a() {
        let p1 = Package::from_name_version_durl("numpy", "2.1.2", None);
        let json = serde_json::to_string(&p1).unwrap();
        assert_eq!(json, "{\"name\":\"numpy\",\"key\":\"numpy\",\"version\":[{\"Number\":2},{\"Number\":1},{\"Number\":2}],\"direct_url\":null}");
    }
    #[test]
    fn test_package_json_b() {
        let json_str = r#"
            {"url": "ssh://git@github.com/uqfoundation/dill.git", "vcs_info": {"commit_id": "a0a8e86976708d0436eec5c8f7d25329da727cb5", "requested_revision": "0.3.8", "vcs": "git"}}
            "#;

        let durl: DirectURL = serde_json::from_str(json_str).unwrap();
        let p1 = Package::from_name_version_durl("dill", "0.3.8", Some(durl)).unwrap();
        let json = serde_json::to_string(&p1).unwrap();
        assert_eq!(json, "{\"name\":\"dill\",\"key\":\"dill\",\"version\":[{\"Number\":0},{\"Number\":3},{\"Number\":8}],\"direct_url\":{\"url\":\"ssh://git@github.com/uqfoundation/dill.git\",\"vcs_info\":{\"commit_id\":\"a0a8e86976708d0436eec5c8f7d25329da727cb5\",\"vcs\":\"git\",\"requested_revision\":\"0.3.8\"}}}");
    }
}
