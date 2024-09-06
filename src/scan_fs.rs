use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use rayon::prelude::*;

use crate::dep_manifest::DepManifest;
use crate::exe_search::find_exe;
use crate::package::Package;

//------------------------------------------------------------------------------
/// Given a path to a Python binary, call out to Python to get all known site packages; some site packages may not exist; we do not filter them here. This will include "dist-packages" on Linux.
fn get_site_package_dirs(executable: &Path) -> Vec<PathBuf> {
    return match Command::new(executable)
            .arg("-c")
            .arg("import site;print(\"\\n\".join(site.getsitepackages()));print(site.getusersitepackages())") // since Python 3.2
            .output() {
        Ok(output) => {
            let paths_lines = std::str::from_utf8(&output.stdout)
                    .expect("Failed to convert to UTF-8")
                    .trim();
            paths_lines
                    .lines()
                    .map(|line| PathBuf::from(line.trim()))
                    .collect()
        }
        Err(e) => {
            eprintln!("Failed to execute command: {}", e); // log this
            Vec::with_capacity(0)
        }
    };
}

// Given a package directory, collect the name of all packages.
fn get_packages(site_packages: &Path) -> Vec<Package> {
    let mut packages = Vec::new();
    if let Ok(entries) = fs::read_dir(site_packages) {
        for entry in entries.flatten() {
            if let Some(file_name) = entry.path().file_name().and_then(|name| name.to_str()) {
                if let Some(package) = Package::from_dist_info(file_name) {
                    packages.push(package);
                }
            }
        }
    }
    packages
}

//------------------------------------------------------------------------------
// #[derive(Debug)]
pub(crate) struct ScanFS {
    exe_to_sites: HashMap<PathBuf, Vec<PathBuf>>,
    package_to_sites: HashMap<Package, Vec<PathBuf>>,
}

// The results of a file-system scan.
impl ScanFS {
    pub(crate) fn from_exe_to_sites(
        exe_to_sites: HashMap<PathBuf, Vec<PathBuf>>,
    ) -> Result<Self, String> {
        // Some site packages will be repeated; let them be processed more than once here, as it seems easier than filtering them out
        let site_to_packages = exe_to_sites
            .par_iter()
            .flat_map(|(_, site_packages)| {
                site_packages.par_iter().map(|site_package_path| {
                    let packages = get_packages(site_package_path);
                    (site_package_path.clone(), packages)
                })
            })
            .collect::<HashMap<PathBuf, Vec<Package>>>();

        let mut package_to_sites: HashMap<Package, Vec<PathBuf>> = HashMap::new();
        for (site_package_path, packages) in site_to_packages.iter() {
            for package in packages {
                package_to_sites
                    .entry(package.clone())
                    .or_default()
                    .push(site_package_path.clone());
            }
        }
        Ok(ScanFS {
            exe_to_sites,
            package_to_sites,
        })
    }
    pub(crate) fn from_defaults() -> Result<Self, String> {
        // For every unique exe, we hae a list of site packages; some site packages might be associated with more than one exe, meaning that a reverse lookup would have to be site-package to Vec of exe
        let exe_to_sites: HashMap<PathBuf, Vec<PathBuf>> = find_exe()
            .into_par_iter()
            .map(|exe| {
                let dirs = get_site_package_dirs(&exe);
                (exe, dirs)
            })
            .collect();
        Self::from_exe_to_sites(exe_to_sites)
    }
    //--------------------------------------------------------------------------
    /// The length of the scan is then number of unique packages.
    pub fn len(&self) -> usize {
        self.package_to_sites.len()
    }
    pub(crate) fn validate(&self, dm: DepManifest) -> HashSet<Package> {
        let mut invalid: HashSet<Package> = HashSet::new();
        for p in self.package_to_sites.keys() {
            if !dm.validate(p) {
                invalid.insert(p.clone());
            }
        }
        invalid
    }
    //--------------------------------------------------------------------------
    // draft implementations
    pub(crate) fn report(&self) {
        let mut packages: Vec<Package> = self.package_to_sites.keys().cloned().collect();
        packages.sort();

        let mut site_packages: HashSet<&PathBuf> = HashSet::new();

        for package in packages {
            println!("{:?}", package);
            if let Some(site_paths) = self.package_to_sites.get(&package) {
                for path in site_paths {
                    site_packages.insert(path);
                    println!("    {:?}", path);
                }
            }
        }
        println!("exes: {:?}", self.exe_to_sites.len());
        println!("site packages: {:?}", site_packages.len());
        println!("packages: {:?}", self.package_to_sites.len());
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    // use crate::dep_spec::DepSpec;
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn test_get_site_package_dirs_a() {
        let p1 = Path::new("python3");
        let paths = get_site_package_dirs(p1);
        assert_eq!(paths.len() > 0, true)
    }
    #[test]
    fn test_from_exe_to_sites_a() {
        let temp_dir = tempdir().unwrap();
        let fp_dir = temp_dir.path();
        let fp_exe = fp_dir.join("python");
        let _ = File::create(fp_exe.clone()).unwrap();

        let fp_sp = fp_dir.join("site-packages");
        fs::create_dir(fp_sp.clone()).unwrap();

        let fp_p1 = fp_sp.join("numpy-1.19.1.dist-info");
        let _ = File::create(fp_p1.clone()).unwrap();
        let fp_p2 = fp_sp.join("foo-3.0.dist-info");
        let _ = File::create(fp_p2.clone()).unwrap();

        let mut exe_to_sites = HashMap::<PathBuf, Vec<PathBuf>>::new();
        exe_to_sites.insert(fp_exe.clone(), vec![fp_sp]);
        let sfs = ScanFS::from_exe_to_sites(exe_to_sites).unwrap();
        assert_eq!(sfs.len(), 2);

        let dm1 = DepManifest::from_iter(vec!["numpy >= 1.19", "foo==3"]).unwrap();
        assert_eq!(dm1.len(), 2);
        let invalid1 = sfs.validate(dm1);
        assert_eq!(invalid1.len(), 0);

        let dm2 = DepManifest::from_iter(vec!["numpy >= 2", "foo==3"]).unwrap();
        let invalid2 = sfs.validate(dm2);
        assert_eq!(invalid2.len(), 1);

        // sfs.report();
    }
}
