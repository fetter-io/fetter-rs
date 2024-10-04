use std::collections::HashSet;

use crate::path_shared::PathShared;
use crate::scan_fs::ScanFS;
use crate::table::Rowable;
use crate::table::Tableable;

#[derive(Debug, Clone)]
pub(crate) struct CountRecord {
    key: String,
    value: usize,
}

impl CountRecord {
    pub(crate) fn new(key: String, value: usize) -> Self {
        CountRecord { key, value }
    }
}

impl Rowable for CountRecord {
    fn to_row(&self) -> Vec<String> {
        vec![self.key.clone(), self.value.to_string()]
    }
}

#[derive(Debug)]
pub struct CountReport {
    records: Vec<CountRecord>,
}

impl CountReport {
    pub(crate) fn from_scan_fs(scan_fs: &ScanFS) -> CountReport {
        // discover unique packages
        let mut site_packages: HashSet<&PathShared> = HashSet::new();
        for package in scan_fs.package_to_sites.keys() {
            if let Some(site_paths) = scan_fs.package_to_sites.get(&package) {
                for path in site_paths {
                    site_packages.insert(path);
                }
            }
        }
        let mut records: Vec<CountRecord> = Vec::new();
        records.push(CountRecord::new(
            "Executables".to_string(),
            scan_fs.exe_to_sites.len(),
        ));
        records.push(CountRecord::new("Sites".to_string(), site_packages.len()));
        records.push(CountRecord::new(
            "Packages".to_string(),
            scan_fs.package_to_sites.len(),
        ));
        CountReport { records }
    }
}

impl Tableable<CountRecord> for CountReport {
    fn get_header(&self) -> Vec<String> {
        vec!["".to_string(), "Count".to_string()]
    }
    fn get_records(&self) -> &Vec<CountRecord> {
        &self.records
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::Package;
    use std::fs::File;
    use std::io::BufRead;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use std::io;

    #[test]
    fn test_from_scan_fs() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("requests", "0.7.6", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();
        let cr = CountReport::from_scan_fs(&sfs);

        let dir = tempdir().unwrap();
        let fp = dir.path().join("report.txt");
        let _ = cr.to_file(&fp, ',');

        let file = File::open(&fp).unwrap();
        let mut lines = io::BufReader::new(file).lines();
        assert_eq!(lines.next().unwrap().unwrap(), ",Count");
        assert_eq!(lines.next().unwrap().unwrap(), "Executables,1");
        assert_eq!(lines.next().unwrap().unwrap(), "Sites,1");
        assert_eq!(lines.next().unwrap().unwrap(), "Packages,3");
    }
}
