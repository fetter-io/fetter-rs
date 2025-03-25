use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::osv_query::query_osv_batches;
use crate::osv_vulns::query_osv_vulns;

use crate::osv_vulns::OSVVulnInfo;
use crate::package::Package;
use crate::table::ColumnFormat;
use crate::table::Rowable;
use crate::table::RowableContext;
use crate::table::Tableable;
use crate::ureq_client::UreqClient;

//------------------------------------------------------------------------------
#[derive(Debug, Serialize)]
pub(crate) struct AuditRecord {
    package: Package,
    vuln_ids: Vec<String>,
    vuln_infos: HashMap<String, OSVVulnInfo>,
}

impl Rowable for AuditRecord {
    fn to_rows(&self, context: &RowableContext) -> Vec<Vec<String>> {
        let is_tty = *context == RowableContext::Tty;

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

            if let Some(vuln_info) = self.vuln_infos.get(vuln_id) {
                rows.push(vec![
                    package_display(),
                    vuln_id.clone(),
                    "URL".to_string(),
                    vuln_info.get_url(),
                ]);
                if let Some(summary) = &vuln_info.summary {
                    rows.push(vec![
                        package_display(),
                        vuln_display(),
                        "Summary".to_string(),
                        summary.clone(),
                    ]);
                }
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
// Complete report of a validation process.
#[derive(Debug, Serialize)]
pub struct AuditReport {
    records: Vec<AuditRecord>,
}

/// An AuditReport, for all provided packages, looks up and display any vulnerabilities in the OSV DB
impl AuditReport {
    pub(crate) fn from_packages(
        client: Arc<dyn UreqClient>,
        packages: &[Package],
    ) -> Self {
        let vulns: Vec<Option<Vec<String>>> = query_osv_batches(client.clone(), packages);
        let mut records = Vec::new();
        for (package, vuln_ids) in packages.iter().zip(vulns.iter()) {
            if let Some(vuln_ids) = vuln_ids {
                let vuln_infos: HashMap<String, OSVVulnInfo> =
                    query_osv_vulns(client.clone(), vuln_ids);

                let record = AuditRecord {
                    package: package.clone(),
                    vuln_ids: vuln_ids.clone(),
                    vuln_infos, // move
                };
                records.push(record);
            }
        }
        AuditReport { records }
    }

    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }
}

impl Tableable<AuditRecord> for AuditReport {
    fn get_header(&self) -> Vec<ColumnFormat> {
        vec![
            ColumnFormat::new("Package".to_string(), false, "#666666".to_string()),
            ColumnFormat::new(
                "Vulnerabilities".to_string(),
                false,
                "#666666".to_string(),
            ),
            ColumnFormat::new("Attribute".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Value".to_string(), true, "#666666".to_string()),
        ]
    }
    fn get_records(&self) -> &Vec<AuditRecord> {
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
    use tempfile::tempdir;

    use crate::table::Tableable;
    use crate::ureq_client::UreqClientMock;

    #[test]
    fn test_audit_report() {
        let mock_get = r#"
        {"id":"GHSA-48cq-79qq-6f7x","summary":"Gradio applications running locally vulnerable to 3rd party websites accessing routes and uploading files","details":" Impact\nThis CVE covers the ability of 3rd party websites to access routes and upload files to users running Gradio applications locally.  For example, the malicious owners of [www.dontvisitme.com](http://www.dontvisitme.com/) could put a script on their website that uploads a large file to http://localhost:7860/upload and anyone who visits their website and has a Gradio app will now have that large file uploaded on their computer\n\n### Patches\nYes, the problem has been patched in Gradio version 4.19.2 or higher. We have no knowledge of this exploit being used against users of Gradio applications, but we encourage all users to upgrade to Gradio 4.19.2 or higher.\n\nFixed in: https://github.com/gradio-app/gradio/commit/84802ee6a4806c25287344dce581f9548a99834a\nCVE: https://nvd.nist.gov/vuln/detail/CVE-2024-1727","aliases":["CVE-2024-1727"],"modified":"2024-05-21T15:12:35.101662Z","published":"2024-05-21T14:43:50Z","database_specific":{"github_reviewed_at":"2024-05-21T14:43:50Z","github_reviewed":true,"severity":"MODERATE","cwe_ids":["CWE-352"],"nvd_published_at":null},"references":[{"type":"WEB","url":"https://github.com/gradio-app/gradio/security/advisories/GHSA-48cq-79qq-6f7x"},{"type":"ADVISORY","url":"https://nvd.nist.gov/vuln/detail/CVE-2024-1727"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/pull/7503"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/commit/84802ee6a4806c25287344dce581f9548a99834a"},{"type":"PACKAGE","url":"https://github.com/gradio-app/gradio"},{"type":"WEB","url":"https://huntr.com/bounties/a94d55fb-0770-4cbe-9b20-97a978a2ffff"}],"affected":[{"package":{"name":"gradio","ecosystem":"PyPI","purl":"pkg:pypi/gradio"},"ranges":[{"type":"ECOSYSTEM","events":[{"introduced":"0"},{"fixed":"4.19.2"}]}],"versions":["4.18.0","4.19.0","4.19.1","4.2.0","4.3.0","4.4.0","4.4.1","4.5.0","4.7.0","4.7.1","4.8.0","4.9.0","4.9.1"],"database_specific":{"source":"https://github.com/github/advisory-database/blob/main/advisories/github-reviewed/2024/05/GHSA-48cq-79qq-6f7x/GHSA-48cq-79qq-6f7x.json"}}],"schema_version":"1.6.0","severity":[{"type":"CVSS_V3","score":"CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L"}]}"#;

        let client = Arc::new(UreqClientMock {
            mock_post : Some("{\"results\":[{\"vulns\":[{\"id\":\"GHSA-48cq-79qq-6f7x\",\"modified\":\"2024-05-21T14:58:25.710902Z\"}]}]}".to_string()),
            mock_get : Some(mock_get.to_string()),
        });

        let packages =
            vec![Package::from_name_version_durl("gradio", "4.0.0", None).unwrap()];

        // client is Arc
        let ar = AuditReport::from_packages(client.clone(), &packages);

        let dir = tempdir().unwrap();
        let fp = dir.path().join("report.txt");
        let _ = ar.to_file(&fp, ',');

        let file = File::open(&fp).unwrap();
        let mut lines = io::BufReader::new(file).lines();
        assert_eq!(
            lines.next().unwrap().unwrap(),
            "Package,Vulnerabilities,Attribute,Value"
        );
        assert_eq!(lines.next().unwrap().unwrap(), "gradio-4.0.0,GHSA-48cq-79qq-6f7x,URL,https://osv.dev/vulnerability/GHSA-48cq-79qq-6f7x");
        assert_eq!(lines.next().unwrap().unwrap(), "gradio-4.0.0,GHSA-48cq-79qq-6f7x,Summary,Gradio applications running locally vulnerable to 3rd party websites accessing routes and uploading files");
        assert_eq!(lines.next().unwrap().unwrap(), "gradio-4.0.0,GHSA-48cq-79qq-6f7x,Reference,https://nvd.nist.gov/vuln/detail/CVE-2024-1727");
        assert_eq!(lines.next().unwrap().unwrap(), "gradio-4.0.0,GHSA-48cq-79qq-6f7x,Severity,CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L");
    }

    #[test]
    fn test_audit_report_serialization() {
        let mock_get = r#"
        {"id":"GHSA-48cq-79qq-6f7x","summary":"Gradio applications running locally vulnerable to 3rd party websites accessing routes and uploading files","details":" Impact\nThis CVE covers the ability of 3rd party websites to access routes and upload files to users running Gradio applications locally.  For example, the malicious owners of [www.dontvisitme.com](http://www.dontvisitme.com/) could put a script on their website that uploads a large file to http://localhost:7860/upload and anyone who visits their website and has a Gradio app will now have that large file uploaded on their computer\n\n### Patches\nYes, the problem has been patched in Gradio version 4.19.2 or higher. We have no knowledge of this exploit being used against users of Gradio applications, but we encourage all users to upgrade to Gradio 4.19.2 or higher.\n\nFixed in: https://github.com/gradio-app/gradio/commit/84802ee6a4806c25287344dce581f9548a99834a\nCVE: https://nvd.nist.gov/vuln/detail/CVE-2024-1727","aliases":["CVE-2024-1727"],"modified":"2024-05-21T15:12:35.101662Z","published":"2024-05-21T14:43:50Z","database_specific":{"github_reviewed_at":"2024-05-21T14:43:50Z","github_reviewed":true,"severity":"MODERATE","cwe_ids":["CWE-352"],"nvd_published_at":null},"references":[{"type":"WEB","url":"https://github.com/gradio-app/gradio/security/advisories/GHSA-48cq-79qq-6f7x"},{"type":"ADVISORY","url":"https://nvd.nist.gov/vuln/detail/CVE-2024-1727"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/pull/7503"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/commit/84802ee6a4806c25287344dce581f9548a99834a"},{"type":"PACKAGE","url":"https://github.com/gradio-app/gradio"},{"type":"WEB","url":"https://huntr.com/bounties/a94d55fb-0770-4cbe-9b20-97a978a2ffff"}],"affected":[{"package":{"name":"gradio","ecosystem":"PyPI","purl":"pkg:pypi/gradio"},"ranges":[{"type":"ECOSYSTEM","events":[{"introduced":"0"},{"fixed":"4.19.2"}]}],"versions":["4.18.0","4.19.0","4.19.1","4.2.0","4.3.0","4.4.0","4.4.1","4.5.0","4.7.0","4.7.1","4.8.0","4.9.0","4.9.1"],"database_specific":{"source":"https://github.com/github/advisory-database/blob/main/advisories/github-reviewed/2024/05/GHSA-48cq-79qq-6f7x/GHSA-48cq-79qq-6f7x.json"}}],"schema_version":"1.6.0","severity":[{"type":"CVSS_V3","score":"CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L"}]}"#;

        let client = UreqClientMock {
            mock_post : Some("{\"results\":[{\"vulns\":[{\"id\":\"GHSA-48cq-79qq-6f7x\",\"modified\":\"2024-05-21T14:58:25.710902Z\"}]}]}".to_string()),
            mock_get : Some(mock_get.to_string()),
        };

        let packages =
            vec![Package::from_name_version_durl("gradio", "4.0.0", None).unwrap()];

        let ar = AuditReport::from_packages(&client, &packages);
        let ar_json = serde_json::to_string_pretty(&ar).unwrap();
        let expected_json = r#"{"records":[{"package":{"name":"gradio","version":"4.0.0","key":"gradio","direct_url":null},"vuln_ids":["GHSA-48cq-79qq-6f7x"],"vuln_infos":{"GHSA-48cq-79qq-6f7x":{"id":"GHSA-48cq-79qq-6f7x","summary":"Gradio applications running locally vulnerable to 3rd party websites accessing routes and uploading files","references":[{"type":"WEB","url":"https://github.com/gradio-app/gradio/security/advisories/GHSA-48cq-79qq-6f7x"},{"type":"ADVISORY","url":"https://nvd.nist.gov/vuln/detail/CVE-2024-1727"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/pull/7503"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/commit/84802ee6a4806c25287344dce581f9548a99834a"},{"type":"PACKAGE","url":"https://github.com/gradio-app/gradio"},{"type":"WEB","url":"https://huntr.com/bounties/a94d55fb-0770-4cbe-9b20-97a978a2ffff"}],"severity":[{"type":"CVSS_V3","score":"CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L"}]}}}]}"#;

        println!("Generated JSON: {}", ar_json);
        let expected_json: serde_json::Value =
            serde_json::from_str(expected_json).unwrap();
        let actual_json: serde_json::Value = serde_json::from_str(&ar_json).unwrap();

        assert_eq!(
            actual_json, expected_json,
            "AuditDigest JSON output mismatch!"
        );
    }
}
