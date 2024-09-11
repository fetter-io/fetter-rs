use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use rayon::prelude::*;

use crate::dep_manifest::DepManifest;
use crate::dep_spec::DepSpec;
use crate::dep_spec::DepOperator;
use crate::exe_search::find_exe;
use crate::package::Package;

//------------------------------------------------------------------------------
#[derive(Debug, Copy, Clone)]
pub(crate) enum Anchor {
    Lower,
    Upper,
    Both,
}

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
    // Alternative constructor from in-memory objects, mostly for testing. Here we provide notional exe and site paths, and focus just on collecting Packages.
    fn from_exe_site_packages(
        exe: PathBuf,
        site: PathBuf,
        packages: Vec<Package>,
    ) -> Result<Self, String> {
        let mut exe_to_sites = HashMap::new();
        exe_to_sites.insert(exe.clone(), vec![site.clone()]);

        let mut package_to_sites = HashMap::new();
        for package in packages {
            package_to_sites
                .entry(package)
                .or_insert_with(Vec::new)
                .push(site.clone());
        }
        Ok(ScanFS {
            exe_to_sites,
            package_to_sites,
        })
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
    // anchor: lower, upper, both
    // operator: greater, eq,
    pub(crate) fn to_dep_manifest(&self,
            anchor: Anchor,
            ) -> Result<DepManifest, String>  {
        let mut package_name_to_package: HashMap<String, Vec<Package>> = HashMap::new();

        for package in self.package_to_sites.keys() {
            package_name_to_package
                .entry(package.name.clone())
                .or_insert_with(Vec::new)
                .push(package.clone());
        }
        // TODO: need case insensitive sort
        let mut names: Vec<String> = package_name_to_package.keys().cloned().collect();
        names.sort();

        let mut dep_specs: Vec<DepSpec> = Vec::new();

        for name in names {
            if let Some(packages) = package_name_to_package.get_mut(&name) {
                packages.sort();
                if let Some(pkg_min) = packages.first() {
                    if let Some(pkg_max) = packages.last() {
                        match anchor {
                            Anchor::Lower => {
                                if let Ok(ds) = DepSpec::from_package(pkg_min, DepOperator::GreaterThanOrEq) {
                                    dep_specs.push(ds);
                                }
                            }
                            Anchor::Upper => {
                                if let Ok(ds) = DepSpec::from_package(pkg_max, DepOperator::LessThanOrEq) {
                                    dep_specs.push(ds);
                                }
                            },
                            Anchor::Both => return Err("Not implemented".to_string()),
                        }
                    }
                }
            }
        }
        DepManifest::from_dep_specs(&dep_specs)
    }

    //--------------------------------------------------------------------------
    // draft implementations
    pub(crate) fn display(&self) {
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
    //--------------------------------------------------------------------------
    #[test]
    fn from_exe_site_packages_a() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3.8/site-packages");
        let packages = vec![
            Package::from_name_and_version("numpy", "1.19.3").unwrap(),
            Package::from_name_and_version("numpy", "1.20.1").unwrap(),
            Package::from_name_and_version("numpy", "2.1.1").unwrap(),
            Package::from_name_and_version("requests", "0.7.6").unwrap(),
            Package::from_name_and_version("requests", "2.32.3").unwrap(),
            Package::from_name_and_version("flask", "3.0.3").unwrap(),
            Package::from_name_and_version("flask", "1.1.3").unwrap(),
        ];
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();
        assert_eq!(sfs.len(), 7);
        // sfs.report();
        let dm = sfs.to_dep_manifest(Anchor::Lower).unwrap();
        println!("{:?}", dm);
        assert_eq!(dm.len(), 3);
    }
}
