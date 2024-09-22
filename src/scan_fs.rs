use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use rayon::prelude::*;

use crate::count_report::CountReport;
use crate::dep_manifest::DepManifest;
use crate::dep_spec::DepOperator;
use crate::dep_spec::DepSpec;
use crate::exe_search::find_exe;
use crate::package::Package;
use crate::path_shared::PathShared;
use crate::scan_report::ScanReport;
use crate::validation_report::ValidationFlags;
use crate::validation_report::ValidationRecord;
use crate::validation_report::ValidationReport;

//------------------------------------------------------------------------------
#[derive(Debug, Copy, Clone)]
pub(crate) enum Anchor {
    Lower,
    Upper,
    Both,
}

//------------------------------------------------------------------------------
/// Given a path to a Python binary, call out to Python to get all known site packages; some site packages may not exist; we do not filter them here. This will include "dist-packages" on Linux.
fn get_site_package_dirs(executable: &Path) -> Vec<PathShared> {
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
                    .map(|line| PathShared::from_str(line.trim()))
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
            let file_path = entry.path();
            if let Some(package) = Package::from_file_path(&file_path) {
                packages.push(package);
            }
        }
    }
    packages
}

//------------------------------------------------------------------------------
// The result of a file-system scan.
pub(crate) struct ScanFS {
    // NOTE: these attributes used by reporters
    /// A mapping of exe path to site packages paths
    pub(crate) exe_to_sites: HashMap<PathBuf, Vec<PathShared>>,
    /// A mapping of Package tp a site package paths
    pub(crate) package_to_sites: HashMap<Package, Vec<PathShared>>,
}

impl ScanFS {
    fn from_exe_to_sites(
        exe_to_sites: HashMap<PathBuf, Vec<PathShared>>,
    ) -> Result<Self, String> {
        // Some site packages will be repeated; let them be processed more than once here, as it seems easier than filtering them out
        let site_to_packages = exe_to_sites
            .par_iter()
            .flat_map(|(_, site_packages)| {
                site_packages.par_iter().map(|site_package_path| {
                    let packages = get_packages(site_package_path.as_path());
                    (site_package_path.clone(), packages)
                })
            })
            .collect::<HashMap<PathShared, Vec<Package>>>();

        let mut package_to_sites: HashMap<Package, Vec<PathShared>> = HashMap::new();
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
    // Given a Vec of PathBuf to executables, use them to collect site packages.
    pub(crate) fn from_exes(exes: Vec<PathBuf>) -> Result<Self, String> {
        let exe_to_sites: HashMap<PathBuf, Vec<PathShared>> = exes
            .into_par_iter()
            .map(|exe| {
                let dirs = get_site_package_dirs(&exe);
                (exe, dirs)
            })
            .collect();
        Self::from_exe_to_sites(exe_to_sites)
    }
    pub(crate) fn from_exe_scan() -> Result<Self, String> {
        // For every unique exe, we hae a list of site packages; some site packages might be associated with more than one exe, meaning that a reverse lookup would have to be site-package to Vec of exe
        let exe_to_sites: HashMap<PathBuf, Vec<PathShared>> = find_exe()
            .into_par_iter()
            .map(|exe| {
                let dirs = get_site_package_dirs(&exe);
                (exe, dirs)
            })
            .collect();
        Self::from_exe_to_sites(exe_to_sites)
    }
    // Alternative constructor from in-memory objects, mostly for testing. Here we provide notional exe and site paths, and focus just on collecting Packages.
    #[allow(dead_code)]
    pub(crate) fn from_exe_site_packages(
        exe: PathBuf,
        site: PathBuf,
        packages: Vec<Package>,
    ) -> Result<Self, String> {
        let mut exe_to_sites = HashMap::new();
        let site_shared = PathShared::from_path_buf(site);

        exe_to_sites.insert(exe.clone(), vec![site_shared.clone()]);

        let mut package_to_sites = HashMap::new();
        for package in packages {
            package_to_sites
                .entry(package)
                .or_insert_with(Vec::new)
                .push(site_shared.clone());
        }
        Ok(ScanFS {
            exe_to_sites,
            package_to_sites,
        })
    }

    //--------------------------------------------------------------------------

    /// Return sorted packages.
    pub(crate) fn get_packages(&self) -> Vec<Package> {
        let mut packages: Vec<Package> = self.package_to_sites.keys().cloned().collect();
        packages.sort();
        packages
    }

    /// The length of the scan is the number of unique packages.
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.package_to_sites.len()
    }

    /// Validate this scan against the provided DepManifest.
    pub(crate) fn to_validation_report(
        &self,
        dm: DepManifest,
        vf: ValidationFlags,
    ) -> ValidationReport {
        let mut records: Vec<ValidationRecord> = Vec::new();

        let mut ds_keys_matched: HashSet<&String> = HashSet::new();

        // iterate over found packages in order for better reporting
        for package in self.get_packages() {
            let ds = dm.get_dep_spec(&package.key);
            // package is valid if ds exists and version is valid, or it does not exist and permit_superset is true
            let package_valid = match ds {
                Some(ds) => {
                    ds_keys_matched.insert(&ds.key);
                    ds.validate_version(&package.version) && ds.validate_url(&package)
                }
                None => vf.permit_superset, // we do not have a matching DepSpec
            };
            if !package_valid {
                // sites might be None
                let sites: Option<Vec<PathShared>> = match vf.report_sites {
                    true => Some(self.package_to_sites.get(&package).unwrap().clone()),
                    false => None,
                };
                // ds  is an Option type, might be None
                records.push(ValidationRecord::new(
                    Some(package.clone()),
                    ds.cloned(),
                    sites,
                ));
            }
        }
        if !vf.permit_subset {
            for key in dm.get_dep_spec_difference(&ds_keys_matched) {
                records.push(ValidationRecord::new(
                    None,
                    dm.get_dep_spec(key).cloned(),
                    None,
                ));
            }
        }
        ValidationReport {
            records: records,
            flags: vf,
        }
    }

    pub(crate) fn to_dep_manifest(&self, anchor: Anchor) -> Result<DepManifest, String> {
        let mut package_name_to_package: HashMap<String, Vec<Package>> = HashMap::new();

        for package in self.package_to_sites.keys() {
            package_name_to_package
                .entry(package.name.clone())
                .or_insert_with(Vec::new)
                .push(package.clone());
        }
        let names: Vec<String> = package_name_to_package.keys().cloned().collect();
        let mut dep_specs: Vec<DepSpec> = Vec::new();
        for name in names {
            let packages = match package_name_to_package.get_mut(&name) {
                Some(packages) => packages,
                None => continue,
            };
            packages.sort();

            let pkg_min = match packages.first() {
                Some(pkg) => pkg,
                None => continue,
            };
            let pkg_max = match packages.last() {
                Some(pkg) => pkg,
                None => continue,
            };

            let ds = match anchor {
                Anchor::Lower => {
                    DepSpec::from_package(pkg_min, DepOperator::GreaterThanOrEq)
                }
                Anchor::Upper => {
                    DepSpec::from_package(pkg_max, DepOperator::LessThanOrEq)
                }
                Anchor::Both => return Err("Not implemented".to_string()),
            };
            if let Ok(dep_spec) = ds {
                dep_specs.push(dep_spec);
            }
        }
        DepManifest::from_dep_specs(&dep_specs)
    }

    pub(crate) fn to_scan_report(&self) -> ScanReport {
        ScanReport::from_package_to_sites(&self.package_to_sites)
    }

    pub(crate) fn to_count_report(&self) -> CountReport {
        CountReport::from_scan_fs(&self)
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
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
        let fp_dir = tempdir().unwrap();
        let fp_exe = fp_dir.path().join("python");
        let _ = File::create(fp_exe.clone()).unwrap();

        let fp_sp = fp_dir.path().join("site-packages");
        fs::create_dir(fp_sp.clone()).unwrap();

        let fp_p1 = fp_sp.join("numpy-1.19.1.dist-info");
        fs::create_dir(&fp_p1).unwrap();

        let fp_p2 = fp_sp.join("foo-3.0.dist-info");
        fs::create_dir(&fp_p2).unwrap();

        let mut exe_to_sites = HashMap::<PathBuf, Vec<PathShared>>::new();
        exe_to_sites.insert(
            fp_exe.clone(),
            vec![PathShared::from_path_buf(fp_sp.to_path_buf())],
        );
        let sfs = ScanFS::from_exe_to_sites(exe_to_sites).unwrap();
        assert_eq!(sfs.len(), 2);

        let dm1 = DepManifest::from_iter(vec!["numpy >= 1.19", "foo==3"]).unwrap();
        assert_eq!(dm1.len(), 2);
        let invalid1 = sfs.to_validation_report(
            dm1,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
                report_sites: false,
            },
        );
        assert_eq!(invalid1.len(), 0);

        let dm2 = DepManifest::from_iter(vec!["numpy >= 2", "foo==3"]).unwrap();
        let invalid2 = sfs.to_validation_report(
            dm2,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
                report_sites: false,
            },
        );
        assert_eq!(invalid2.len(), 1);
    }
    //--------------------------------------------------------------------------
    #[test]
    fn from_exe_site_packages_a() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3.8/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("numpy", "1.20.1", None).unwrap(),
            Package::from_name_version_durl("numpy", "2.1.1", None).unwrap(),
            Package::from_name_version_durl("requests", "0.7.6", None).unwrap(),
            Package::from_name_version_durl("requests", "2.32.3", None).unwrap(),
            Package::from_name_version_durl("flask", "3.0.3", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();
        assert_eq!(sfs.len(), 7);
        // sfs.report();
        let dm = sfs.to_dep_manifest(Anchor::Lower).unwrap();
        println!("{:?}", dm);
        assert_eq!(dm.len(), 3);
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_validation_a() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("requests", "0.7.6", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let dm = DepManifest::from_iter(
            vec!["numpy>1.19", "requests==0.7.6", "flask> 1"].iter(),
        )
        .unwrap();

        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();
        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
                report_sites: false,
            },
        );
        assert_eq!(vr.len(), 0);
    }
    #[test]
    fn test_validation_b() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("requests", "0.7.6", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let dm = DepManifest::from_iter(
            vec!["numpy>1.19", "requests==0.7.6", "flask> 2"].iter(),
        )
        .unwrap();

        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();
        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
                report_sites: false,
            },
        );

        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(json, r#"[["flask-1.1.3","flask>2","Invalid",null]]"#);
    }
    #[test]
    fn test_validation_c() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("requests", "0.7.6", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let dm = DepManifest::from_iter(
            vec!["numpy>2", "requests==0.7.1", "flask> 2,<3"].iter(),
        )
        .unwrap();

        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();
        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
                report_sites: false,
            },
        );

        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[["flask-1.1.3","flask>2,<3","Invalid",null],["numpy-1.19.3","numpy>2","Invalid",null],["requests-0.7.6","requests==0.7.1","Invalid",null]]"#
        );
    }

    #[test]
    fn test_validation_d() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("requests", "0.7.6", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let dm = DepManifest::from_iter(vec!["numpy>2", "flask> 2,<3"].iter()).unwrap();

        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: true,
                permit_subset: false,
                report_sites: false,
            },
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[["flask-1.1.3","flask>2,<3","Invalid",null],["numpy-1.19.3","numpy>2","Invalid",null]]"#
        );
    }
    #[test]
    fn test_validation_e() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // hyphen / underscore are normalized
        let dm = DepManifest::from_iter(
            vec!["numpy==1.19.3", "flask>1,<2", "static_frame==2.13.0"].iter(),
        )
        .unwrap();
        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
                report_sites: false,
            },
        );
        assert_eq!(vr.len(), 0);
    }
    #[test]
    fn test_validation_f() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
        ];
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // hyphen / underscore are normalized
        let dm = DepManifest::from_iter(
            vec!["numpy==1.19.3", "flask>1,<2", "static_frame==2.13.0"].iter(),
        )
        .unwrap();
        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
                report_sites: false,
            },
        );
        assert_eq!(vr.len(), 1);
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(json, r#"[[null,"flask>1,<2","Missing",null]]"#);
    }
    #[test]
    fn test_validation_g() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
        ];
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();
        let dm = DepManifest::from_iter(vec!["numpy==1.19.3"].iter()).unwrap();
        let vr1 = sfs.to_validation_report(
            dm.clone(),
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
                report_sites: false,
            },
        );
        assert_eq!(vr1.len(), 1);
        let json = serde_json::to_string(&vr1.to_validation_digest()).unwrap();
        assert_eq!(json, r#"[["static-frame-2.13.0",null,"Disallowed",null]]"#);

        let vr2 = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: true,
                permit_subset: false,
                report_sites: false,
            },
        );
        assert_eq!(vr2.len(), 0);
    }
    #[test]
    fn test_validation_h() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
        ];
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // hyphen / underscore are normalized
        let dm = DepManifest::from_iter(
            vec!["numpy==1.19.3", "flask>1,<2", "static_frame==2.13.0"].iter(),
        )
        .unwrap();
        let vr1 = sfs.to_validation_report(
            dm.clone(),
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
                report_sites: false,
            },
        );
        let json = serde_json::to_string(&vr1.to_validation_digest()).unwrap();
        assert_eq!(json, r#"[[null,"flask>1,<2","Missing",null]]"#);

        let vr2 = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: true,
                report_sites: false,
            },
        );
        assert_eq!(vr2.len(), 0);
    }
}
