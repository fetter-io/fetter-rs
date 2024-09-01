use std::collections::HashMap;
// use std::fs;
// use std::path::Path;
// use std::process::Command;
// use tempfile::tempdir;

use crate::dep_spec::DepSpec;
use crate::package::Package;


#[derive(Debug)]
struct DepManifest {
    packages: HashMap<String, DepSpec>,
}


impl DepManifest {
    pub fn from_vec(package_specs: Vec<String>) -> Result<Self, String> {
        let mut packages = HashMap::new();
        for spec in package_specs {
            let dep_spec = DepSpec::new(&spec)?;
            packages.insert(dep_spec.name.clone(), dep_spec);
        }
        Ok(DepManifest { packages })
    }

    // pub fn from_requirements_file<P: AsRef<Path>>(file_path: P) -> Result<Self, String> {
    //     let contents = fs::read_to_string(file_path)
    //         .map_err(|e| format!("Failed to read requirements file: {}", e))?;
    //     let lines = contents.lines();
    //     let mut packages = HashMap::new();

    //     for line in lines {
    //         let trimmed_line = line.trim();
    //         if !trimmed_line.is_empty() && !trimmed_line.starts_with("#") {
    //             let dep_spec = DepSpec::new(trimmed_line)?;
    //             packages.insert(dep_spec.name.clone(), dep_spec);
    //         }
    //     }
    //     Ok(DepManifest { packages })
    // }

    // pub fn from_pyproject_toml<P: AsRef<Path>>(file_path: P) -> Result<Self, String> {
    //     let contents = fs::read_to_string(file_path)
    //         .map_err(|e| format!("Failed to read pyproject.toml file: {}", e))?;
    //     let parsed_toml: toml::Value = toml::from_str(&contents)
    //         .map_err(|e| format!("Failed to parse pyproject.toml file: {}", e))?;

    //     let mut packages = HashMap::new();
    //     if let Some(dependencies) = parsed_toml.get("tool").and_then(|t| t.get("poetry")).and_then(|p| p.get("dependencies")) {
    //         for (name, version) in dependencies.as_table().ok_or("Dependencies is not a table")? {
    //             let spec = format!("{} {}", name, version.as_str().unwrap_or(""));
    //             let dep_spec = DepSpec::new(&spec)?;
    //             packages.insert(dep_spec.name.clone(), dep_spec);
    //         }
    //     } else {
    //         return Err("No dependencies found in pyproject.toml".to_string());
    //     }
    //     Ok(DepManifest { packages })
    // }

    // pub fn from_git_repo(repo_url: &str) -> Result<Self, String> {
    //     // Create a temporary directory
    //     let tmp_dir = tempdir().map_err(|e| format!("Failed to create temporary directory: {}", e))?;
    //     let repo_path = tmp_dir.path().join("repo");

    //     // Shell out to git to perform a shallow clone
    //     let status = Command::new("git")
    //         .args(&["clone", "--depth", "1", repo_url, repo_path.to_str().unwrap()])
    //         .status()
    //         .map_err(|e| format!("Failed to execute git: {}", e))?;

    //     if !status.success() {
    //         return Err("Git clone failed".to_string());
    //     }

    //     // Construct the path to the requirements.txt file
    //     let requirements_path = repo_path.join("requirements.txt");

    //     // Load the requirements.txt file into a DepManifest
    //     let manifest = DepManifest::from_requirements_file(&requirements_path)?;

    //     // TempDir will be cleaned up when it goes out of scope
    //     Ok(manifest)
    // }

    pub fn validate(&self, package: &Package) -> bool {
        if let Some(dep_spec) = self.packages.get(&package.name) {
            dep_spec.validate_version(&package.version_spec)
        } else {
            false
        }
    }
}


//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dep_spec_a() {
        let dm = DepManifest::from_vec(vec![
            "pk1>=0.2,<0.3".to_string(),
            "pk2>=1,<3".to_string(),
            ]).unwrap();

        let p1 = Package::from_dist_info("pk2-2.0.dist-info").unwrap();
        assert_eq!(dm.validate(&p1), true);

        let p2 = Package::from_dist_info("foo-2.0.dist-info").unwrap();
        assert_eq!(dm.validate(&p2), false);

        let p3 = Package::from_dist_info("pk1-0.2.5.dist-info").unwrap();
        assert_eq!(dm.validate(&p3), true);

    }
}