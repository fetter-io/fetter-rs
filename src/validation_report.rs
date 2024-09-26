use std::cmp;
use std::fmt;
use std::fs::File;
use std::io;
use std::io::Write;
use std::path::PathBuf;

use crate::dep_spec::DepSpec;
use crate::package::Package;
use crate::path_shared::PathShared;

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

//------------------------------------------------------------------------------
enum ValidationExplain {
    Missing,
    Disallowed,
    Invalid,
    Undefined,
}

impl fmt::Display for ValidationExplain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            ValidationExplain::Missing => "Missing", // package not found
            ValidationExplain::Disallowed => "Disallowed", // package found, not specified
            ValidationExplain::Invalid => "Invalid", // package found, not matched version
            ValidationExplain::Undefined => "Undefined",
        };
        write!(f, "{}", value)
    }
}

// A summary of validation results suitable for JSON serialziation to naive readers
pub type ValidationDigest =
    Vec<(Option<String>, Option<String>, String, Option<Vec<String>>)>;

//------------------------------------------------------------------------------
// Complete report of a validation process.
// #[derive(Debug)]
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

    fn to_writer<W: Write>(&self, mut writer: W, delimiter: char) -> io::Result<()> {
        let mut package_displays: Vec<String> = Vec::new();
        let mut dep_spec_displays: Vec<String> = Vec::new();
        let mut explain_displays: Vec<String> = Vec::new();
        let mut sites_displays: Vec<String> = Vec::new();

        let mut max_package_width = "Package".len();
        let mut max_dep_spec_width = "Dependency".len();
        let mut max_explain_width = "Explain".len();

        let dep_missing = "";
        let pkg_missing = "";

        let mut records: Vec<&ValidationRecord> = self.records.iter().collect();
        records.sort_by_key(|item| &item.package);

        for item in &records {
            let pkg_display = match &item.package {
                Some(package) => format!("{}", package),
                None => pkg_missing.to_string(),
            };

            let dep_display = match &item.dep_spec {
                Some(dep_spec) => format!("{}", dep_spec),
                None => dep_missing.to_string(),
            };

            let explain_display = match (&item.package, &item.dep_spec) {
                (Some(_), Some(_)) => ValidationExplain::Invalid.to_string(),
                (None, Some(_)) => ValidationExplain::Missing.to_string(),
                (Some(_), None) => ValidationExplain::Disallowed.to_string(),
                (None, None) => ValidationExplain::Undefined.to_string(),
            };

            let sites_display = match &item.sites {
                Some(sites) => sites
                    .iter()
                    .map(|s| format!("{:?}", s))
                    .collect::<Vec<_>>()
                    .join(","),
                None => "".to_string(),
            };
            sites_displays.push(sites_display);

            max_package_width = cmp::max(max_package_width, pkg_display.len());
            max_dep_spec_width = cmp::max(max_dep_spec_width, dep_display.len());
            max_explain_width = cmp::max(max_explain_width, explain_display.len());

            package_displays.push(pkg_display);
            dep_spec_displays.push(dep_display);
            explain_displays.push(explain_display);
        }
        // TODO: show sites
        writeln!(
            writer,
            "{:<package_width$}{}{:<dep_spec_width$}{}{:<explain_width$}",
            "Package",
            delimiter,
            "Dependency",
            delimiter,
            "Explain",
            package_width = max_package_width,
            dep_spec_width = max_dep_spec_width,
            explain_width = max_explain_width,
        )?;

        for (pkg_display, (dep_display, explain_display)) in package_displays
            .iter()
            .zip(dep_spec_displays.iter().zip(explain_displays))
        {
            writeln!(
                writer,
                "{:<package_width$}{}{:<dep_spec_width$}{}{:<explain_width$}",
                pkg_display,
                delimiter,
                dep_display,
                delimiter,
                explain_display,
                package_width = max_package_width,
                dep_spec_width = max_dep_spec_width,
                explain_width = max_explain_width,
            )?;
        }
        Ok(())
    }

    pub(crate) fn to_file(&self, file_path: &PathBuf, delimiter: char) -> io::Result<()> {
        let file = File::create(file_path)?;
        self.to_writer(file, delimiter)
    }

    pub(crate) fn to_stdout(&self) {
        let stdout = io::stdout();
        let handle = stdout.lock();
        self.to_writer(handle, ' ').unwrap();
    }

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
                Some(sites) => Some(
                    sites
                        .iter()
                        .map(|s| format!("{}", s.display()))
                        .collect::<Vec<_>>(),
                ),
                None => None,
            };
            let explain = match (&pkg_display, &dep_display) {
                (Some(_), Some(_)) => ValidationExplain::Invalid.to_string(),
                (None, Some(_)) => ValidationExplain::Missing.to_string(),
                (Some(_), None) => ValidationExplain::Disallowed.to_string(),
                (None, None) => ValidationExplain::Undefined.to_string(),
            };

            digests.push((pkg_display, dep_display, explain, sites_display));
        }
        digests
    }
}
