use serde::{Deserialize, Serialize};
// use std::cmp;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;

use crate::dep_manifest::DepManifest;
use crate::dep_spec::DepSpec;
use crate::package::Package;
use crate::path_shared::PathShared;
use crate::table::ColumnFormat;
use crate::table::Rowable;
use crate::table::RowableContext;
use crate::table::Tableable;
use crate::EnvMarkerState;

//------------------------------------------------------------------------------
pub enum ValidationExplain {
    Missing,
    Unrequired,
    Misdefined,
    Undefined,
}

impl fmt::Display for ValidationExplain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            ValidationExplain::Missing => "Missing", // not found
            ValidationExplain::Unrequired => "Unrequired", // found, not specified
            ValidationExplain::Misdefined => "Misdefined", // found, not matched version
            ValidationExplain::Undefined => "Undefined",
        };
        write!(f, "{value}")
    }
}

//------------------------------------------------------------------------------
#[derive(Debug)]
pub struct ValidationFlags {
    pub permit_superset: bool,
    pub permit_subset: bool,
}

#[derive(Debug, PartialEq)]
pub struct ValidationRecord {
    pub package: Option<Package>,
    pub dep_spec: Option<DepSpec>,
    pub sites: Option<Vec<PathShared>>,
}

impl ValidationRecord {
    pub fn new(
        package: Option<Package>,
        dep_spec: Option<DepSpec>,
        sites: Option<Vec<PathShared>>,
    ) -> Self {
        ValidationRecord {
            package,
            dep_spec,
            sites,
        }
    }

    pub fn explain(&self) -> ValidationExplain {
        match (&self.package, &self.dep_spec) {
            (Some(_), Some(_)) => ValidationExplain::Misdefined,
            (None, Some(_)) => ValidationExplain::Missing,
            (Some(_), None) => ValidationExplain::Unrequired,
            (None, None) => ValidationExplain::Undefined,
        }
    }
}

impl Rowable for ValidationRecord {
    fn to_rows(&self, _context: &RowableContext) -> Vec<Vec<String>> {
        // these could be different or configurable
        let dep_missing = "";
        let pkg_missing = "";

        let pkg_display = match &self.package {
            Some(package) => package.to_string(),
            None => pkg_missing.to_string(),
        };
        let dep_display = match &self.dep_spec {
            Some(dep_spec) => dep_spec.to_string(),
            None => dep_missing.to_string(),
        };
        // we reduce this to a string for concise representation
        let sites_display = match &self.sites {
            Some(sites) => sites
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(","),
            None => "".to_string(),
        };
        vec![vec![
            pkg_display,
            dep_display,
            self.explain().to_string(),
            sites_display,
        ]]
    }
}

//------------------------------------------------------------------------------
// A summary of validation results suitable for JSON serialization to naive readers that need labelled fields.
#[derive(Serialize, Deserialize)]
pub(crate) struct ValidationDigestRecord {
    package: Option<String>,
    dependency: Option<String>,
    explain: String,
    sites: Option<Vec<String>>,
}

pub(crate) type ValidationDigest = Vec<ValidationDigestRecord>;

//------------------------------------------------------------------------------
// Complete report of a validation process.
pub struct ValidationReport {
    pub records: Vec<ValidationRecord>,
}

impl ValidationReport {
    pub fn from_components(
        packages: &Vec<Package>, // ordered for reporting
        package_to_sites: &HashMap<Package, Vec<PathShared>>,
        site_to_exe: &HashMap<PathShared, PathBuf>, // only needed if exe_to_ems is Some
        exe_to_ems: &Option<HashMap<PathBuf, EnvMarkerState>>,
        dm: &DepManifest,
        vf: &ValidationFlags,
        ignore: Option<&HashSet<String>>,
    ) -> ValidationReport {
        let mut records: Vec<ValidationRecord> = Vec::new();
        // We collect all DS keys matched to package, regardless of if the version matches; we can then (if we do not permit_subset) find all the DS definitions that were not satisfied
        let mut ds_keys_matched: HashSet<&String> = HashSet::new();

        // iterate over found packages in order for better reporting
        for package in packages {
            if ignore.is_some_and(|i| i.contains(&package.name)) {
                continue;
            }
            if !dm.has_package(package) {
                if !vf.permit_superset {
                    let sites = package_to_sites.get(package).cloned();
                    // Add records if package is not in the DM and do not permit superset
                    records.push(ValidationRecord::new(
                        Some(package.clone()),
                        None,
                        sites,
                    ));
                }
                // else do not add record
            } else if let Some(exe_to_ems) = exe_to_ems {
                // For each package, if the DepManifest has env_marker_active, we have already loaded EnvMarkerState
                for site in package_to_sites.get(package).unwrap() {
                    let exe = site_to_exe.get(site).unwrap();
                    let ems = exe_to_ems.get(exe); // validate() expects Option
                    let (valid, ds) = dm.validate(package, vf.permit_superset, ems);
                    if let Some(ds) = ds {
                        ds_keys_matched.insert(&ds.key);
                    }
                    if !valid {
                        records.push(ValidationRecord::new(
                            Some(package.clone()),
                            ds.cloned(),
                            Some(vec![site.clone()]),
                        ));
                    }
                }
            } else {
                // env_marker_active is False
                let (valid, ds) = dm.validate(package, vf.permit_superset, None);
                if let Some(ds) = ds {
                    ds_keys_matched.insert(&ds.key);
                }
                if !valid {
                    let sites = package_to_sites.get(package).cloned();
                    // ds is an Option type, might be None
                    records.push(ValidationRecord::new(
                        Some(package.clone()),
                        ds.cloned(),
                        sites,
                    ));
                }
            }
        }
        if !vf.permit_subset {
            // find DS in DM that are not in packages; if any DS has env_marker relevant to the known environments, report it.
            // NOTE: this is sorted, but not sorted with the other records
            for key in dm.get_dep_spec_difference(&ds_keys_matched) {
                if let Some(iter) = dm.get_dep_specs(key) {
                    for ds in iter {
                        if ignore.is_some_and(|i| i.contains(&ds.name)) {
                            continue;
                        }
                        // if a DS has an env_marker, that env_marker must be valid for at least one of our exe environents
                        if !ds.env_marker.is_empty() {
                            if let Some(exe_to_ems) = exe_to_ems {
                                if exe_to_ems
                                    .values()
                                    .any(|ems| ds.validate_env_marker(ems))
                                {
                                    records.push(ValidationRecord::new(
                                        None,
                                        Some(ds.clone()),
                                        None,
                                    ));
                                }
                            }
                        } else {
                            records.push(ValidationRecord::new(
                                None,
                                Some(ds.clone()),
                                None,
                            ));
                        }
                    }
                }
            }
        }
        ValidationReport { records }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub(crate) fn to_validation_digest(&self) -> ValidationDigest {
        let mut records: Vec<&ValidationRecord> = self.records.iter().collect();
        records.sort_by_key(|item| &item.package);

        let mut digests: ValidationDigest = Vec::new();
        for record in &records {
            let pkg_display = record.package.as_ref().map(|package| format!("{package}"));
            let dep_display = record
                .dep_spec
                .as_ref()
                .map(|dep_spec| format!("{dep_spec}"));
            let sites = record
                .sites
                .as_ref()
                .map(|sites| sites.iter().map(|s| s.to_string()).collect::<Vec<_>>());
            digests.push(ValidationDigestRecord {
                package: pkg_display,
                dependency: dep_display,
                explain: record.explain().to_string(),
                sites,
            });
        }
        digests
    }
}

impl Tableable<ValidationRecord> for ValidationReport {
    fn get_header(&self) -> Vec<ColumnFormat> {
        vec![
            ColumnFormat::new("Package".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Dependency".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Explain".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Sites".to_string(), true, "#666666".to_string()),
        ]
    }
    fn get_records(&self) -> &Vec<ValidationRecord> {
        &self.records
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dep_manifest::DepManifest;
    use crate::scan_fs::ScanFS;
    use crate::util::FlagLog;
    use std::fs::File;
    use std::io;
    use std::io::BufRead;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn test_to_file_a() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("flask", "1.2", None).unwrap(),
            Package::from_name_version_durl("packaging", "24.1", None).unwrap(),
        ];
        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // hyphen / underscore are normalized
        let dm = DepManifest::try_from_iter(
            ["numpy==2.1.0", "flask>1,<2", "static_frame==2.1.0"].iter(),
        )
        .unwrap();
        let vr1 = sfs.to_validation_report(
            dm.clone(),
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );

        let dir = tempdir().unwrap();
        let fp = dir.path().join("valid.txt");
        let _ = vr1.to_file(&fp, '|');

        let file = File::open(&fp).unwrap();
        let mut lines = io::BufReader::new(file).lines();
        assert_eq!(
            lines.next().unwrap().unwrap(),
            "Package|Dependency|Explain|Sites"
        );
        assert_eq!(
            lines.next().unwrap().unwrap(),
            "numpy-1.19.3|numpy==2.1.0|Misdefined|/usr/lib/python3/site-packages"
        );
        assert_eq!(
            lines.next().unwrap().unwrap(),
            "packaging-24.1||Unrequired|/usr/lib/python3/site-packages"
        );
        assert_eq!(lines.next().unwrap().unwrap(), "static-frame-2.13.0|static_frame==2.1.0|Misdefined|/usr/lib/python3/site-packages");
        assert!(lines.next().is_none());
    }
}
