use std::cmp;
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::io::Write;
use std::path::PathBuf;

use crate::package::Package;
use crate::path_shared::PathShared;

#[derive(Debug)]
pub(crate) struct ScanRecord {
    package: Package,
    sites: Vec<PathShared>,
}

impl ScanRecord {
    pub(crate) fn new(package: Package, sites: Vec<PathShared>) -> Self {
        ScanRecord { package, sites }
    }
}

#[derive(Debug)]
pub struct ScanReport {
    records: Vec<ScanRecord>,
}

impl ScanReport {
    pub(crate) fn from_package_to_sites(
        package_to_sites: &HashMap<Package, Vec<PathShared>>,
    ) -> Self {
        let mut records = Vec::new();
        for (package, sites) in package_to_sites {
            let record = ScanRecord::new(package.clone(), sites.clone());
            records.push(record);
        }
        ScanReport { records }
    }

    // Alternative constructor when we want to report on a subset of all packages.
    pub(crate) fn from_packages(
        packages: &Vec<Package>,
        package_to_sites: &HashMap<Package, Vec<PathShared>>,
    ) -> Self {
        let mut records = Vec::new();
        for package in packages {
            let sites = package_to_sites.get(package).unwrap();
            let record = ScanRecord::new(package.clone(), sites.clone());
            records.push(record);
        }
        ScanReport { records }
    }

    //--------------------------------------------------------------------------

    fn to_writer<W: Write>(
        &self,
        mut writer: W,
        delimiter: char,
        repeat_package: bool,
        pad: bool,
    ) -> io::Result<()> {
        let mut package_displays: Vec<String> = Vec::new();
        let mut max_package_width = 0;

        let mut records: Vec<&ScanRecord> = self.records.iter().collect();
        records.sort_by_key(|item| &item.package);

        for item in &records {
            let pkg_display = format!("{}", item.package);
            if pad {
                max_package_width = cmp::max(max_package_width, pkg_display.len());
            }
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
        self.to_writer(file, delimiter, true, false)
    }

    pub(crate) fn to_stdout(&self) {
        let stdout = io::stdout();
        let handle = stdout.lock();
        self.to_writer(handle, ' ', false, true).unwrap();
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScanFS;
    use std::io::BufRead;
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
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        let sr1 = sfs.to_scan_report();

        let dir = tempdir().unwrap();
        let fp = dir.path().join("scan.txt");
        let _ = sr1.to_file(&fp, '|');

        let file = File::open(&fp).unwrap();
        let mut lines = io::BufReader::new(file).lines();
        assert_eq!(lines.next().unwrap().unwrap(), "Package|Site");
        assert_eq!(
            lines.next().unwrap().unwrap(),
            "flask-1.2|/usr/lib/python3/site-packages"
        );
        assert_eq!(
            lines.next().unwrap().unwrap(),
            "numpy-1.19.3|/usr/lib/python3/site-packages"
        );
        assert_eq!(
            lines.next().unwrap().unwrap(),
            "packaging-24.1|/usr/lib/python3/site-packages"
        );
        assert_eq!(
            lines.next().unwrap().unwrap(),
            "static-frame-2.13.0|/usr/lib/python3/site-packages"
        );

        assert!(lines.next().is_none());
    }
}
