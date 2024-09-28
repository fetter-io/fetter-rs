use std::cmp;
use std::collections::HashSet;
use std::fs::File;
use std::io;
use std::io::Write;
use std::path::PathBuf;

use crate::path_shared::PathShared;
use crate::scan_fs::ScanFS;

#[derive(Debug)]
pub(crate) struct CountRecord {
    key: String,
    value: usize,
}

impl CountRecord {
    pub(crate) fn new(key: String, value: usize) -> Self {
        CountRecord { key, value }
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
        // CountReport::new(records)
        CountReport { records }
    }

    fn to_writer<W: Write>(
        &self,
        mut writer: W,
        delimiter: char,
        pad: bool,
    ) -> io::Result<()> {
        let mut key_displays: Vec<String> = Vec::new();
        let mut max_key_width = 0;

        for item in self.records.iter() {
            let key_display = format!("{}", item.key);
            if pad {
                max_key_width = cmp::max(max_key_width, key_display.len());
            }
            key_displays.push(key_display);
        }
        writeln!(
            writer,
            "{:<key_width$}{}{}",
            "", // no header for key
            delimiter,
            "Count",
            key_width = max_key_width,
        )?;

        for (key_display, record) in key_displays.iter().zip(self.records.iter()) {
            writeln!(
                writer,
                "{:<key_width$}{}{}",
                key_display,
                delimiter,
                record.value,
                key_width = max_key_width,
            )?;
        }
        Ok(())
    }

    pub(crate) fn to_file(&self, file_path: &PathBuf, delimiter: char) -> io::Result<()> {
        let file = File::create(file_path)?;
        self.to_writer(file, delimiter, false)
    }

    pub(crate) fn to_stdout(&self) {
        let stdout = io::stdout();
        let handle = stdout.lock();
        self.to_writer(handle, ' ', true).unwrap();
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::Package;
    use std::io::BufRead;
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
        assert_eq!(lines.next().unwrap().unwrap(), ",Count");
        assert_eq!(lines.next().unwrap().unwrap(), "Executables,1");
        assert_eq!(lines.next().unwrap().unwrap(), "Sites,1");
        assert_eq!(lines.next().unwrap().unwrap(), "Packages,3");
    }
}
