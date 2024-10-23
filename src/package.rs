use std::cmp::Ordering;
use std::fmt;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::package_durl::DirectURL;
use crate::path_shared::PathShared;
use crate::util::name_to_key;
use crate::version_spec::VersionSpec;

//------------------------------------------------------------------------------
// Given a name from the dist-info dir, try to find the src dir int the site dir doing a case-insenstive search. Then, return the case-sensitve name of the src dir.
fn find_dir_src(site: &PathBuf, name_from_di: &str) -> Option<String> {
    if let Ok(entries) = fs::read_dir(site) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    if file_name.eq_ignore_ascii_case(name_from_di) {
                        return Some(file_name.to_string());
                    }
                }
            }
        }
    }
    None
}

// Given the name of dist-info directory, get a the name and the version
fn extract_from_dist_info(file_name: &str) -> Option<(String, String)> {
    let trimmed_input = file_name.trim_end_matches(".dist-info");
    let parts: Vec<&str> = trimmed_input.split('-').collect();
    if parts.len() >= 2 {
        // NOTE: we expect that dist-info based names have already normalized hyphens to underscores
        let name = parts[..parts.len() - 1].join("-");
        let version = parts.last()?.to_string();
        Some((name, version))
    } else {
        None
    }
}


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
    /// Create a Package from a dist-info string. As the name of the package / source dir may be different than the dist-info representation, optionall provide a `name`
    #[allow(dead_code)]
    pub(crate) fn from_dist_info(
        file_name: &str,
        name: Option<&str>,
        direct_url: Option<DirectURL>,
    ) -> Option<Self> {
        if let Some((name_from_di, version)) = extract_from_dist_info(file_name) {
            if let Some(name) = name { // favor name if passed in
                return Self::from_name_version_durl(name, &version, direct_url);
            } else {
                return Self::from_name_version_durl(&name_from_di, &version, direct_url);
            }
        }
        None
    }
    /// Create a Package from a dist_info file path. This is the main constructor for live usage.
    pub(crate) fn from_file_path(file_path: &PathBuf) -> Option<Self> {
        let file_name = file_path.file_name().and_then(|name| name.to_str())?;

        if file_name.ends_with(".dist-info") && file_path.is_dir() {
            let fp_durl = file_path.join("direct_url.json");
            let durl = if fp_durl.is_file() {
                DirectURL::from_file(&fp_durl).ok()
            } else {
                None
            };

            let dir_site = file_path.parent()?.to_path_buf(); // TODO: propagate package errors

            if let Some((name_from_di, version)) = extract_from_dist_info(file_name) {
                let name = match find_dir_src(&dir_site, &name_from_di) {
                    Some(name) => name,
                    None => name_from_di,
                };
                return Self::from_name_version_durl(&name, &version, durl);
            }
        }
        None
    }

    /// Given a site directory, return a `PathBuf` to this Package's dist info directory.
    pub(crate) fn to_dist_info_dir(&self, site: &PathShared) -> PathBuf {
        site.join(&format!("{}.dist-info", self))
    }

    /// Given a site directory, return a `PathBuf` to this Package's dist info directory.
    pub(crate) fn to_src_dir(&self, site: &PathShared) -> PathBuf {
        site.join(&self.name)
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
        let p1 =
            Package::from_dist_info("matplotlib-3.9.0.dist-info", None, None).unwrap();
        assert_eq!(p1.name, "matplotlib");
        assert_eq!(p1.version.to_string(), "3.9.0");
    }

    #[test]
    fn test_package_b() {
        assert_eq!(
            Package::from_dist_info("matplotlib3.9.0.distin", None, None),
            None
        );
    }

    #[test]
    fn test_package_c() {
        let p1 = Package::from_dist_info("xarray-0.21.1.dist-info", None, None).unwrap();
        let p2 =
            Package::from_dist_info("xarray-2024.6.0.dist-info", None, None).unwrap();
        let p3 =
            Package::from_dist_info("xarray-2024.6.0.dist-info", None, None).unwrap();

        assert_eq!(p2 > p1, true);
        assert_eq!(p1 < p2, true);
        assert_eq!(p1 == p3, false);
        assert_eq!(p2 == p3, true);
    }
    #[test]
    fn test_package_to_string_a() {
        let p1 =
            Package::from_dist_info("matplotlib-3.9.0.dist-info", None, None).unwrap();
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
