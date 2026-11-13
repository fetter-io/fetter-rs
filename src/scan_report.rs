use std::collections::HashMap;

use crate::origin_site::Site;
use crate::package::Package;
use crate::table::ColumnFormat;
use crate::table::Rowable;
use crate::table::RowableContext;
use crate::table::Tableable;

#[derive(Debug, Clone)]
pub(crate) struct ScanRecord {
    package: Package,
    sites: Vec<Site>,
}

impl ScanRecord {
    pub(crate) fn new(package: Package, sites: Vec<Site>) -> Self {
        ScanRecord { package, sites }
    }

    /// Display order: by package, then by sites. Distinct packages can compare equal (e.g. the same name and version with different `direct_url`), so the sites tie-break keeps output deterministic.
    fn cmp_display(a: &Self, b: &Self) -> std::cmp::Ordering {
        a.package
            .cmp(&b.package)
            .then_with(|| a.sites.cmp(&b.sites))
    }
}

impl Rowable for ScanRecord {
    fn to_rows(&self, context: &RowableContext) -> Vec<Vec<String>> {
        let mut rows: Vec<Vec<String>> = Vec::new();
        let pkg_display = self.package.to_string();
        let is_tty = *context == RowableContext::Tty;

        let eco_display = self.package.ecosystem.to_string();
        for (i, path) in self.sites.iter().enumerate() {
            let (e, p) = if i > 0 && is_tty {
                ("".to_string(), "".to_string())
            } else {
                (eco_display.clone(), pkg_display.clone())
            };
            rows.push(vec![e, p, path.to_string()]);
        }
        rows
    }
}

#[derive(Debug)]
pub struct ScanReport {
    records: Vec<ScanRecord>,
}

impl ScanReport {
    pub(crate) fn from_package_to_sites(
        package_to_sites: &HashMap<Package, Vec<Site>>,
    ) -> Self {
        let mut records = Vec::new();
        for (package, sites) in package_to_sites {
            let record = ScanRecord::new(package.clone(), sites.clone());
            records.push(record);
        }
        records.sort_by(ScanRecord::cmp_display);
        ScanReport { records }
    }

    // Alternative constructor when we want to report on a subset of all packages.
    pub(crate) fn from_packages(
        packages: &Vec<Package>,
        package_to_sites: &HashMap<Package, Vec<Site>>,
    ) -> Self {
        let mut records = Vec::new();
        for package in packages {
            let sites = package_to_sites.get(package).unwrap();
            let record = ScanRecord::new(package.clone(), sites.clone());
            records.push(record);
        }
        records.sort_by(ScanRecord::cmp_display);
        ScanReport { records }
    }
}

impl Tableable<ScanRecord> for ScanReport {
    fn get_header(&self) -> Vec<ColumnFormat> {
        vec![
            ColumnFormat::new("Ecosystem".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Package".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Site".to_string(), true, "#666666".to_string()),
        ]
    }
    fn get_records(&self) -> &Vec<ScanRecord> {
        &self.records
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan_fs::ScanFS;
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
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        let sr1 = sfs.to_scan_report();

        let dir = tempdir().unwrap();
        let fp = dir.path().join("scan.txt");
        let _ = sr1.to_file(&fp, '|');

        let file = File::open(&fp).unwrap();
        let mut lines = io::BufReader::new(file).lines();

        assert_eq!(lines.next().unwrap().unwrap(), "Ecosystem|Package|Site");
        assert_eq!(
            lines.next().unwrap().unwrap(),
            "python|flask-1.2|/usr/lib/python3/site-packages"
        );
        assert_eq!(
            lines.next().unwrap().unwrap(),
            "python|numpy-1.19.3|/usr/lib/python3/site-packages"
        );
        assert_eq!(
            lines.next().unwrap().unwrap(),
            "python|packaging-24.1|/usr/lib/python3/site-packages"
        );
        assert_eq!(
            lines.next().unwrap().unwrap(),
            "python|static-frame-2.13.0|/usr/lib/python3/site-packages"
        );
        assert!(lines.next().is_none());
    }
}
