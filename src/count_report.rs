use std::collections::HashSet;

use crate::eco::scanner_for;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Site;
use crate::scan_fs::ScanFS;
use crate::table::ColumnFormat;
use crate::table::Rowable;
use crate::table::RowableContext;
use crate::table::Tableable;

//------------------------------------------------------------------------------
/// Counts for one ecosystem, or for all ecosystems if `ecosystem` is None.
#[derive(Debug, Clone)]
pub(crate) struct CountRecord {
    ecosystem: Option<Ecosystem>,
    origins: usize,
    sites: usize,
    packages: usize,
}

impl Rowable for CountRecord {
    fn to_rows(&self, _context: &RowableContext) -> Vec<Vec<String>> {
        let label = match self.ecosystem {
            Some(e) => e.to_string(),
            None => "all".to_string(),
        };
        vec![vec![
            label,
            self.origins.to_string(),
            self.sites.to_string(),
            self.packages.to_string(),
        ]]
    }
}

//------------------------------------------------------------------------------
#[derive(Debug)]
pub(crate) struct CountReport {
    records: Vec<CountRecord>,
}

impl CountReport {
    /// One row per enabled (and implemented) ecosystem, including those with zero counts; if there is more than one row, a final "all" row gives totals. Only Sites that contain at least one package are counted.
    pub(crate) fn from_scan_fs(scan_fs: &ScanFS) -> CountReport {
        let sites: HashSet<&Site> = scan_fs.package_to_sites.values().flatten().collect();

        let mut records: Vec<CountRecord> = scan_fs
            .ecosystems()
            .iter()
            .filter(|e| scanner_for(**e).is_some())
            .map(|e| CountRecord {
                ecosystem: Some(*e),
                origins: scan_fs
                    .origin_to_sites
                    .keys()
                    .filter(|o| o.ecosystem == *e)
                    .count(),
                sites: sites.iter().filter(|s| s.ecosystem == *e).count(),
                packages: scan_fs
                    .package_to_sites
                    .keys()
                    .filter(|p| p.ecosystem == *e)
                    .count(),
            })
            .collect();

        if records.len() > 1 {
            records.push(CountRecord {
                ecosystem: None,
                origins: scan_fs.origin_to_sites.len(),
                sites: sites.len(),
                packages: scan_fs.package_to_sites.len(),
            });
        }
        CountReport { records }
    }
}

impl Tableable<CountRecord> for CountReport {
    fn get_header(&self) -> Vec<ColumnFormat> {
        vec![
            ColumnFormat::new("Ecosystem".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Origins".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Sites".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Packages".to_string(), false, "#666666".to_string()),
        ]
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
    use std::io;
    use std::io::BufRead;
    use std::path::PathBuf;
    use tempfile::tempdir;

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
        assert_eq!(
            lines.next().unwrap().unwrap(),
            "Ecosystem,Origins,Sites,Packages"
        );
        assert_eq!(lines.next().unwrap().unwrap(), "python,1,1,3");
        // implemented ecosystems with nothing found still get a row, then totals
        assert_eq!(lines.next().unwrap().unwrap(), "npm,0,0,0");
        assert_eq!(lines.next().unwrap().unwrap(), "cargo,0,0,0");
        assert_eq!(lines.next().unwrap().unwrap(), "all,1,1,3");
        assert!(lines.next().is_none());
    }
}
