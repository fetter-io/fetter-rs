use std::collections::HashMap;

use crate::osv_query::query_osv_batches;
use crate::osv_vulns::query_osv_vulns;

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

// impl AuditRecord {
//     pub(crate) fn new(key: String, value: usize) -> Self {
//         AuditRecord { key, value }
//     }
// }

impl Rowable for AuditRecord {
    fn to_rows(&self, _context: &RowableContext) -> Vec<Vec<String>> {
        let mut rows = Vec::new();
        for (i, vuln_id) in self.vuln_ids.iter().enumerate() {
            if i == 0 {
                rows.push(vec![self.package.to_string(), vuln_id.clone()])
            } else {
                rows.push(vec!["".to_string(), vuln_id.clone()])
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
        vec!["Package".to_string(), "Vulnerabilities".to_string()]
    }
    fn get_records(&self) -> &Vec<AuditRecord> {
        &self.records
    }
}
