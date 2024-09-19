use std::cmp;
// use std::fmt;
use std::fs::File;
use std::io;
use std::io::Write;
use std::path::PathBuf;

use crate::dep_spec::DepSpec;
use crate::package::Package;

#[derive(Debug)]
pub(crate) struct ValidationFlags {
    pub(crate) permit_unspecified: bool,
    pub(crate) report_sites: bool,
}

#[derive(Debug)]
pub(crate) struct ValidationRecord {
    package: Package,
    dep_spec: Option<DepSpec>,
    sites: Option<Vec<PathBuf>>,
}

impl ValidationRecord {
    pub(crate) fn new(
        package: Package,
        dep_spec: Option<DepSpec>,
        sites: Option<Vec<PathBuf>>,
    ) -> Self {
        ValidationRecord {
            package,
            dep_spec,
            sites,
        }
    }
}

#[derive(Debug)]
pub struct ValidationReport {
    pub(crate) records: Vec<ValidationRecord>,
    pub(crate) flags: ValidationFlags,
}

impl ValidationReport {
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    #[allow(dead_code)]
    pub(crate) fn get_package_strings(&self) -> Vec<String> {
        self.records
            .iter()
            .map(|record| record.package.to_string())
            .collect()
    }

    fn to_writer<W: Write>(&self, mut writer: W, delimiter: char) -> io::Result<()> {
        let mut package_displays: Vec<String> = Vec::new();
        let mut dep_spec_displays: Vec<String> = Vec::new();
        let mut site_displays: Vec<String> = Vec::new();

        let mut max_package_width = 0;
        let mut max_dep_spec_width = 0;

        let dep_missing = '-';

        let mut records: Vec<&ValidationRecord> = self.records.iter().collect();
        records.sort_by_key(|item| &item.package);

        for item in &records {
            let pkg_display = format!("{}", item.package);

            let dep_display = match &item.dep_spec {
                Some(dep_spec) => format!("{}", dep_spec),
                None => dep_missing.to_string(),
            };

            if self.flags.report_sites {
                let site_display = match &item.sites {
                    Some(sites) => sites
                        .iter()
                        .map(|s| format!("{:?}", s))
                        .collect::<Vec<_>>()
                        .join(","),
                    None => "".to_string(),
                };
                site_displays.push(site_display);
            }

            max_package_width = cmp::max(max_package_width, pkg_display.len());
            max_dep_spec_width = cmp::max(max_dep_spec_width, dep_display.len());

            package_displays.push(pkg_display);
            dep_spec_displays.push(dep_display);
        }
        // TODO: show sites
        writeln!(
            writer,
            "{:<package_width$}{}{:<dep_spec_width$}",
            "Package",
            delimiter,
            "Dependency",
            package_width = max_package_width,
            dep_spec_width = max_dep_spec_width
        )?;

        for (pkg_display, dep_display) in
            package_displays.iter().zip(dep_spec_displays.iter())
        {
            writeln!(
                writer,
                "{:<package_width$}{}{:<dep_spec_width$}",
                pkg_display,
                delimiter,
                dep_display,
                package_width = max_package_width,
                dep_spec_width = max_dep_spec_width
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
}
