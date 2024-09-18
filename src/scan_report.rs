use std::cmp;
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::io::Write;
use std::path::PathBuf;

use crate::package::Package;

#[derive(Debug)]
pub(crate) struct ScanRecord {
    package: Package,
    sites: Vec<PathBuf>,
}

impl ScanRecord {
    pub(crate) fn new(package: Package, sites: Vec<PathBuf>) -> Self {
        ScanRecord { package, sites }
    }
}

#[derive(Debug)]
pub struct ScanReport {
    records: Vec<ScanRecord>,
}

impl ScanReport {
    pub(crate) fn from_package_to_sites(package_to_sites: &HashMap<Package, Vec<PathBuf>>) -> Self {
        let mut records = Vec::new();
        for (package, sites) in package_to_sites {
            let record = ScanRecord::new(package.clone(), sites.clone());
            records.push(record);
        }
        ScanReport { records }
    }

    fn to_writer<W: Write>(
        &self,
        mut writer: W,
        delimiter: char,
        repeat_package: bool,
    ) -> io::Result<()> {
        let mut package_displays: Vec<String> = Vec::new();
        let mut max_package_width = 0;

        let mut records: Vec<&ScanRecord> = self.records.iter().collect();
        records.sort_by_key(|item| &item.package);

        for item in &records {
            let pkg_display = format!("{}", item.package);
            max_package_width = cmp::max(max_package_width, pkg_display.len());
            package_displays.push(pkg_display);
        }
        writeln!(
            writer,
            "{:<package_width$}{}{}",
            "Package",
            delimiter,
            "Site",
            package_width = max_package_width,
        )?;

        for (pkg_display, record) in package_displays.iter().zip(records.iter()) {
            for (index, site) in record.sites.iter().enumerate() {
                writeln!(
                    writer,
                    "{:<package_width$}{}{}",
                    if index == 0 || repeat_package {
                        pkg_display
                    } else {
                        ""
                    },
                    delimiter,
                    site.display(),
                    package_width = max_package_width,
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn to_file(&self, file_path: &PathBuf, delimiter: char) -> io::Result<()> {
        let file = File::create(file_path)?;
        self.to_writer(file, delimiter, true)
    }

    pub(crate) fn to_stdout(&self) {
        let stdout = io::stdout();
        let handle = stdout.lock();
        self.to_writer(handle, ' ', false).unwrap();
    }
}

// TODO: need tests
