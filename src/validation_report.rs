use serde::{Deserialize, Serialize};
// use std::cmp;
use std::fmt;


use crate::dep_spec::DepSpec;
use crate::package::Package;
use crate::path_shared::PathShared;
use crate::table::Rowable;
use crate::table::Tableable;


//------------------------------------------------------------------------------
enum ValidationExplain {
    Missing,
    Unrequired,
    Misdefined,
    Undefined,
}

impl fmt::Display for ValidationExplain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            ValidationExplain::Missing => "Missing",       // not found
            ValidationExplain::Unrequired => "Unrequired", // found, not specified
            ValidationExplain::Misdefined => "Misdefined", // found, not matched version
            ValidationExplain::Undefined => "Undefined",
        };
        write!(f, "{}", value)
    }
}

//------------------------------------------------------------------------------
#[derive(Debug)]
pub(crate) struct ValidationFlags {
    pub(crate) permit_superset: bool,
    pub(crate) permit_subset: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) struct ValidationRecord {
    package: Option<Package>,
    dep_spec: Option<DepSpec>,
    sites: Option<Vec<PathShared>>,
}

impl ValidationRecord {
    pub(crate) fn new(
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
}

impl Rowable for ValidationRecord {
    fn to_rows(&self) -> Vec<Vec<String>> {
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
        let explain_display = match (&self.package, &self.dep_spec) {
            (Some(_), Some(_)) => ValidationExplain::Misdefined.to_string(),
            (None, Some(_)) => ValidationExplain::Missing.to_string(),
            (Some(_), None) => ValidationExplain::Unrequired.to_string(),
            (None, None) => ValidationExplain::Undefined.to_string(),
        };
        // we reduce this to a string for concise representation
        let sites_display = match &self.sites {
            Some(sites) => sites
                .iter()
                .map(|s| format!("{}", s.display()))
                .collect::<Vec<_>>()
                .join(","),
            None => "".to_string(),
        };
        return vec![vec![pkg_display, dep_display, explain_display, sites_display]];
    }
}

//------------------------------------------------------------------------------
// A summary of validation results suitable for JSON serialziation to naive readers.
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
    pub(crate) records: Vec<ValidationRecord>,
}

impl ValidationReport {
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    #[allow(dead_code)]
    pub(crate) fn get_package_strings(&self) -> Vec<String> {
        self.records
            .iter()
            .filter_map(|record| record.package.as_ref().map(ToString::to_string))
            .collect()
    }

    // fn to_writer<W: Write>(
    //     &self,
    //     mut writer: W,
    //     delimiter: char,
    //     pad: bool,
    // ) -> io::Result<()> {
    //     let mut package_displays: Vec<String> = Vec::new();
    //     let mut dep_spec_displays: Vec<String> = Vec::new();
    //     let mut explain_displays: Vec<String> = Vec::new();
    //     let mut sites_displays: Vec<String> = Vec::new();

    //     let mut max_package_width = 0;
    //     let mut max_dep_spec_width = 0;
    //     let mut max_explain_width = 0;

    //     if pad {
    //         max_package_width = "Package".len();
    //         max_dep_spec_width = "Dependency".len();
    //         max_explain_width = "Explain".len();
    //     }

    //     let dep_missing = "";
    //     let pkg_missing = "";

    //     let mut records: Vec<&ValidationRecord> = self.records.iter().collect();
    //     records.sort_by_key(|item| &item.package);

    //     for item in &records {
    //         let pkg_display = match &item.package {
    //             Some(package) => format!("{}", package),
    //             None => pkg_missing.to_string(),
    //         };

    //         let dep_display = match &item.dep_spec {
    //             Some(dep_spec) => format!("{}", dep_spec),
    //             None => dep_missing.to_string(),
    //         };

    //         let explain_display = match (&item.package, &item.dep_spec) {
    //             (Some(_), Some(_)) => ValidationExplain::Misdefined.to_string(),
    //             (None, Some(_)) => ValidationExplain::Missing.to_string(),
    //             (Some(_), None) => ValidationExplain::Unrequired.to_string(),
    //             (None, None) => ValidationExplain::Undefined.to_string(),
    //         };

    //         let sites_display = match &item.sites {
    //             // we reduce this to a string for concise representation
    //             Some(sites) => sites
    //                 .iter()
    //                 .map(|s| format!("{}", s.display()))
    //                 .collect::<Vec<_>>()
    //                 .join(","),
    //             None => "".to_string(),
    //         };
    //         sites_displays.push(sites_display);

    //         if pad {
    //             max_package_width = cmp::max(max_package_width, pkg_display.len());
    //             max_dep_spec_width = cmp::max(max_dep_spec_width, dep_display.len());
    //             max_explain_width = cmp::max(max_explain_width, explain_display.len());
    //         }

    //         package_displays.push(pkg_display);
    //         dep_spec_displays.push(dep_display);
    //         explain_displays.push(explain_display);
    //     }
    //     writeln!(
    //         writer,
    //         "{:<package_width$}{}{:<dep_spec_width$}{}{:<explain_width$}{}{}",
    //         "Package",
    //         delimiter,
    //         "Dependency",
    //         delimiter,
    //         "Explain",
    //         delimiter,
    //         "Sites",
    //         package_width = max_package_width,
    //         dep_spec_width = max_dep_spec_width,
    //         explain_width = max_explain_width,
    //     )?;

    //     for (pkg_display, (dep_display, (explain_display, sites_display))) in
    //         package_displays.iter().zip(
    //             dep_spec_displays
    //                 .iter()
    //                 .zip(explain_displays.iter().zip(sites_displays)),
    //         )
    //     {
    //         writeln!(
    //             writer,
    //             "{:<package_width$}{}{:<dep_spec_width$}{}{:<explain_width$}{}{}",
    //             pkg_display,
    //             delimiter,
    //             dep_display,
    //             delimiter,
    //             explain_display,
    //             delimiter,
    //             sites_display,
    //             package_width = max_package_width,
    //             dep_spec_width = max_dep_spec_width,
    //             explain_width = max_explain_width,
    //         )?;
    //     }
    //     Ok(())
    // }

    // pub(crate) fn to_file(&self, file_path: &PathBuf, delimiter: char) -> io::Result<()> {
    //     let file = File::create(file_path)?;
    //     self.to_writer(file, delimiter, false)
    // }

    // pub(crate) fn to_stdout(&self) {
    //     let stdout = io::stdout();
    //     let handle = stdout.lock();
    //     self.to_writer(handle, ' ', true).unwrap();
    // }

    pub(crate) fn to_validation_digest(&self) -> ValidationDigest {
        let mut records: Vec<&ValidationRecord> = self.records.iter().collect();
        records.sort_by_key(|item| &item.package);

        let mut digests: ValidationDigest = Vec::new();
        for item in &records {
            let pkg_display = match &item.package {
                Some(package) => Some(format!("{}", package)),
                None => None,
            };
            let dep_display = match &item.dep_spec {
                Some(dep_spec) => Some(format!("{}", dep_spec)),
                None => None,
            };
            let sites_display = match &item.sites {
                // we leave this as a Vec for JSON encoding as an array
                Some(sites) => Some(
                    sites
                        .iter()
                        .map(|s| format!("{}", s.display()))
                        .collect::<Vec<_>>(),
                ),
                None => None,
            };
            let explain = match (&pkg_display, &dep_display) {
                (Some(_), Some(_)) => ValidationExplain::Misdefined.to_string(),
                (None, Some(_)) => ValidationExplain::Missing.to_string(),
                (Some(_), None) => ValidationExplain::Unrequired.to_string(),
                (None, None) => ValidationExplain::Undefined.to_string(),
            };

            digests.push(ValidationDigestRecord {
                package: pkg_display,
                dependency: dep_display,
                explain: explain,
                sites: sites_display,
            });
        }
        digests
    }
}

impl Tableable<ValidationRecord> for ValidationReport {
    fn get_header(&self) -> Vec<String> {
        vec!["Package".to_string(), "Dependency".to_string(), "Explain".to_string(), "Sites".to_string()]
    }
    fn get_records(&self) -> &Vec<ValidationRecord> {
        &self.records
    }
}


//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::DepManifest;
    use crate::ScanFS;
    use std::io::BufRead;
    use tempfile::tempdir;
    use std::fs::File;
    use std::io;
    use std::path::PathBuf;

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
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // hyphen / underscore are normalized
        let dm = DepManifest::from_iter(
            vec!["numpy==2.1.0", "flask>1,<2", "static_frame==2.1.0"].iter(),
        )
        .unwrap();
        let vr1 = sfs.to_validation_report(
            dm.clone(),
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
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
