use std::collections::HashMap;

use crate::osv_query::query_osv_batches;
use crate::osv_vulns::query_osv_vulns;

use crate::osv_vulns::get_osv_url;
use crate::osv_vulns::OSVVulnInfo;
use crate::package::Package;
use crate::table::Rowable;
use crate::table::RowableContext;
use crate::table::Tableable;
use crate::ureq_client::UreqClientLive;
//------------------------------------------------------------------------------
#[derive(Debug)]
pub(crate) struct AuditRecord {
    package: Package,
    vuln_ids: Vec<String>,
    vuln_infos: HashMap<String, OSVVulnInfo>,
}

impl Rowable for AuditRecord {
    fn to_rows(&self, context: &RowableContext) -> Vec<Vec<String>> {
        let is_tty = *context == RowableContext::TTY;

        let mut rows = Vec::new();
        let mut package_set = false;
        let mut package_display = || {
            if !is_tty || !package_set {
                package_set = true;
                self.package.to_string()
            } else {
                "".to_string()
            }
        };
        for vuln_id in self.vuln_ids.iter() {
            let vuln_display = || {
                if is_tty {
                    "".to_string()
                } else {
                    vuln_id.clone()
                }
            };
            rows.push(vec![
                package_display(),
                vuln_id.clone(),
                "URL".to_string(),
                get_osv_url(vuln_id),
            ]);

            if let Some(vuln_info) = self.vuln_infos.get(vuln_id) {
                rows.push(vec![
                    package_display(),
                    vuln_display(),
                    "Summary".to_string(),
                    vuln_info.summary.chars().take(60).collect(), // TEMP!
                ]);

                rows.push(vec![
                    package_display(),
                    vuln_display(),
                    "Reference".to_string(),
                    vuln_info.references.get_prime(),
                ]);

                if let Some(severity) = &vuln_info.severity {
                    rows.push(vec![
                        package_display(),
                        vuln_display(),
                        "Severity".to_string(),
                        severity.get_prime(),
                    ]);
                }
            }
        }

        rows
    }
}

//------------------------------------------------------------------------------
#[derive(Debug)]
pub struct AuditReport {
    records: Vec<AuditRecord>,
}

/// An AuditReport, for all provided packages, looks up and display any vulnerabilities in the OSV DB
impl AuditReport {
    pub(crate) fn from_packages(packages: &Vec<Package>) -> Self {
        let client = UreqClientLive;
        let vulns: Vec<Option<Vec<String>>> = query_osv_batches(&client, packages);
        let mut records = Vec::new();
        for (package, vuln_ids) in packages.iter().zip(vulns.iter()) {
            if let Some(vuln_ids) = vuln_ids {
                let vuln_infos: HashMap<String, OSVVulnInfo> =
                    query_osv_vulns(&client, vuln_ids);

                let record = AuditRecord {
                    package: package.clone(),
                    vuln_ids: vuln_ids.clone(),
                    vuln_infos: vuln_infos, // move
                };
                records.push(record);
            }
        }
        AuditReport { records }
    }
}

impl Tableable<AuditRecord> for AuditReport {
    fn get_header(&self) -> Vec<String> {
        vec![
            "Package".to_string(),
            "Vulnerabilities".to_string(),
            "Attribute".to_string(),
            "Value".to_string(),
        ]
    }
    fn get_records(&self) -> &Vec<AuditRecord> {
        &self.records
    }
}
