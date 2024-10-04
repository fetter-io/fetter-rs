use std::cmp;
use std::fs::File;
use std::io;
use std::io::Write;
use std::path::PathBuf;

use crate::osv_query::query_osv_batches;
use crate::package::Package;
use crate::ureq_client::UreqClientLive;

#[derive(Debug)]
pub(crate) struct AuditRecord {
    package: Package,
    vulns: String,
}

#[derive(Debug)]
pub struct AuditReport {
    records: Vec<AuditRecord>,
}

impl AuditReport {
    pub(crate) fn from_packages(packages: &Vec<Package>) -> Self {
        let vulns: Vec<Option<Vec<String>>> =
            query_osv_batches(&UreqClientLive, packages);
        let mut records = Vec::new();
        for (package, vuln_ids) in packages.iter().zip(vulns.iter()) {
            // TODO: look up all vuln_ids with query_osv_vulns
            let vulns = match vuln_ids {
                Some(v) => v.join(", "),
                None => "".to_string(),
            };
            let record = AuditRecord {
                package: package.clone(),
                vulns: vulns,
            };
            records.push(record);
        }
        AuditReport { records }
    }

    fn to_writer<W: Write>(
        &self,
        mut writer: W,
        delimiter: char,
        pad: bool,
    ) -> io::Result<()> {
        let mut package_displays: Vec<String> = Vec::new();
        let mut max_package_width = 0;

        for item in &self.records {
            let package_display = format!("{}", item.package);
            if pad {
                max_package_width = cmp::max(max_package_width, package_display.len());
            }
            package_displays.push(package_display);
        }
        writeln!(
            writer,
            "{:<key_width$}{}{}",
            "Package",
            delimiter,
            "Vulnerabilities",
            key_width = max_package_width,
        )?;

        for (package_display, record) in package_displays.iter().zip(self.records.iter())
        {
            writeln!(
                writer,
                "{:<package_width$}{}{}",
                package_display,
                delimiter,
                record.vulns,
                package_width = max_package_width,
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
