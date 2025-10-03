use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;

use crate::cli::CvssFilter;
use crate::osv_query::query_osv_batches;
use crate::osv_vulns::query_osv_vulns;
use crate::util::logger;
use crate::util::DURATION_0;

use crate::osv_vulns::VulnInfo;
use crate::package::Package;
use crate::table::ColumnFormat;
use crate::table::Rowable;
use crate::table::RowableContext;
use crate::table::Tableable;
use crate::ureq_client::UreqClient;
use crate::util::CacheConfig;
use crate::util::FlagCacheRefresh;
use crate::util::FlagLog;

//------------------------------------------------------------------------------
#[derive(Debug, Serialize)]
pub struct AuditRecord {
    pub package: Package,
    pub vuln_ids: Vec<String>,
    pub vuln_infos: HashMap<String, VulnInfo>,
}

impl AuditRecord {
    /// Among all the vuln_ids, vuln_infos, remove all references for scores less than min_score. This updates in-place vuln_infos and vuln_ids. This might leave the AuditRecord with no vulnerabiltities.
    fn filter_by_cvss_threshold(&mut self, min_score: f64) {
        self.vuln_infos.retain(|_vuln_id, vuln_info| {
            if let Some(cvss_details) = &vuln_info.cvss_details {
                cvss_details.has_score_gte(min_score)
            } else {
                false
            }
        });
        // Update vuln_ids to only include the remaining vulnerabilities
        self.vuln_ids
            .retain(|vuln_id| self.vuln_infos.contains_key(vuln_id));
    }
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

                if let Some(cvss_details) = &vuln_info.cvss_details {
                    rows.push(vec![
                        package_display(),
                        vuln_display(),
                        "Severity".to_string(),
                        cvss_details.get_prime(), // gets a display
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
    pub records: Vec<AuditRecord>,
}

/// An AuditReport, for all provided packages, looks up package vulnerabilities and details in the OSV DB. There are two types of caching. (1) A cache on the vulnerabilities found for the provided set of packages. This is a time-cache that is invalidated after `duration`. (2) A cache of the details of each vulnerability. As these are unlikely to change often, these are fetched once and only removed if `cache_refresh` is set.
impl AuditReport {
    pub fn from_packages(
        client: Arc<dyn UreqClient>,
        packages: &[Package],
        cache_refresh: FlagCacheRefresh,
        mut cache: CacheConfig,
        log: FlagLog,
        filter_cvss: CvssFilter,
    ) -> Self {
        if packages.is_empty() {
            return AuditReport {
                records: Vec::new(),
            };
        }
        // if cache_refresh is false, force no usage of any caching
        if bool::from(cache_refresh) {
            cache.duration = DURATION_0;
        }
        let vulns: Vec<Option<Vec<String>>> = match query_osv_batches(
            client.clone(),
            packages,
            cache.duration,
            cache.dir,
            log,
        ) {
            Ok(vulns) => vulns,
            Err(e) => {
                logger!(log, module_path!(), "Failed to query OSV batches: {}", e);
                return AuditReport {
                    records: Vec::new(),
                };
            }
        };
        logger!(log, module_path!(), "Completed query_osv_batch");

        let mut records = Vec::new();
        for (package, vuln_ids) in packages.iter().zip(vulns.iter()) {
            if let Some(vuln_ids) = vuln_ids {
                let vuln_infos: HashMap<String, VulnInfo> = query_osv_vulns(
                    client.clone(),
                    vuln_ids,
                    cache_refresh,
                    cache.dir,
                    log,
                );

                let record = AuditRecord {
                    package: package.clone(),
                    vuln_ids: vuln_ids.clone(),
                    vuln_infos, // move
                };
                records.push(record);
            }
        }
        Self::apply_cvss_filter(AuditReport { records }, filter_cvss)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    fn apply_cvss_filter(
        mut report: AuditReport,
        filter_cvss: CvssFilter,
    ) -> AuditReport {
        let threshold: Option<f64> = match filter_cvss {
            CvssFilter::All => None,
            CvssFilter::MaxOnly => report.find_max_cvss_score(),
            CvssFilter::Threshold(min) => Some(min),
        };

        if let Some(min) = threshold {
            for record in &mut report.records {
                record.filter_by_cvss_threshold(min);
            }
            // after removing filtered vulns from records, we then need to remove those records
            report.records.retain(|r| !r.vuln_infos.is_empty());
        }
        report
    }

    fn find_max_cvss_score(&self) -> Option<f64> {
        let mut max_score = None;
        for record in &self.records {
            for vuln_info in record.vuln_infos.values() {
                if let Some(cvss_details) = &vuln_info.cvss_details {
                    if let Some(score) = cvss_details.get_max_score() {
                        match max_score {
                            None => max_score = Some(score),
                            Some(current_max) => {
                                if score > current_max {
                                    max_score = Some(score);
                                }
                            }
                        }
                    }
                }
            }
        }
        max_score
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
    use crate::util::path_cache;
    use std::fs::File;
    use std::io;
    use std::io::BufRead;
    use tempfile::tempdir;

    use crate::table::Tableable;
    use crate::ureq_client::UreqClientMock;

    #[test]
    fn test_audit_report_a() {
        let mock_get = r#"
        {"id":"GHSA-48cq-79qq-6f7x","summary":"Gradio applications running locally vulnerable to 3rd party websites accessing routes and uploading files","details":" Impact\nThis CVE covers the ability of 3rd party websites to access routes and upload files to users running Gradio applications locally.  For example, the malicious owners of [www.dontvisitme.com](http://www.dontvisitme.com/) could put a script on their website that uploads a large file to http://localhost:7860/upload and anyone who visits their website and has a Gradio app will now have that large file uploaded on their computer\n\n### Patches\nYes, the problem has been patched in Gradio version 4.19.2 or higher. We have no knowledge of this exploit being used against users of Gradio applications, but we encourage all users to upgrade to Gradio 4.19.2 or higher.\n\nFixed in: https://github.com/gradio-app/gradio/commit/84802ee6a4806c25287344dce581f9548a99834a\nCVE: https://nvd.nist.gov/vuln/detail/CVE-2024-1727","aliases":["CVE-2024-1727"],"modified":"2024-05-21T15:12:35.101662Z","published":"2024-05-21T14:43:50Z","database_specific":{"github_reviewed_at":"2024-05-21T14:43:50Z","github_reviewed":true,"severity":"MODERATE","cwe_ids":["CWE-352"],"nvd_published_at":null},"references":[{"type":"WEB","url":"https://github.com/gradio-app/gradio/security/advisories/GHSA-48cq-79qq-6f7x"},{"type":"ADVISORY","url":"https://nvd.nist.gov/vuln/detail/CVE-2024-1727"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/pull/7503"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/commit/84802ee6a4806c25287344dce581f9548a99834a"},{"type":"PACKAGE","url":"https://github.com/gradio-app/gradio"},{"type":"WEB","url":"https://huntr.com/bounties/a94d55fb-0770-4cbe-9b20-97a978a2ffff"}],"affected":[{"package":{"name":"gradio","ecosystem":"PyPI","purl":"pkg:pypi/gradio"},"ranges":[{"type":"ECOSYSTEM","events":[{"introduced":"0"},{"fixed":"4.19.2"}]}],"versions":["4.18.0","4.19.0","4.19.1","4.2.0","4.3.0","4.4.0","4.4.1","4.5.0","4.7.0","4.7.1","4.8.0","4.9.0","4.9.1"],"database_specific":{"source":"https://github.com/github/advisory-database/blob/main/advisories/github-reviewed/2024/05/GHSA-48cq-79qq-6f7x/GHSA-48cq-79qq-6f7x.json"}}],"schema_version":"1.6.0","severity":[{"type":"CVSS_V3","score":"CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L"}]}"#;

        let client = Arc::new(UreqClientMock {
            mock_post : Some("{\"results\":[{\"vulns\":[{\"id\":\"GHSA-48cq-79qq-6f7x\",\"modified\":\"2024-05-21T14:58:25.710902Z\"}]}]}".to_string()),
            mock_get : Some(mock_get.to_string()),
        });

        let packages =
            vec![Package::from_name_version_durl("gradio", "4.0.0", None).unwrap()];
        let cache_dir = path_cache(true).unwrap();

        // client is Arc
        let cache = CacheConfig::new(DURATION_0, &cache_dir);
        let ar = AuditReport::from_packages(
            client.clone(),
            &packages,
            FlagCacheRefresh(true),
            cache,
            FlagLog(false),
            CvssFilter::All,
        );

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
        assert_eq!(lines.next().unwrap().unwrap(), "gradio-4.0.0,GHSA-48cq-79qq-6f7x,Severity,CVSS 4.3 (Medium): CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L");
    }

    #[test]
    fn test_audit_report_b() {
        let client = Arc::new(UreqClientMock {
            mock_post: Some(String::new()),
            mock_get: Some(String::new()),
        });

        let packages: Vec<Package> = vec![];
        let cache_dir = path_cache(true).unwrap();
        // client is Arc
        let cache = CacheConfig::new(DURATION_0, &cache_dir);
        let ar = AuditReport::from_packages(
            client.clone(),
            &packages,
            FlagCacheRefresh(true),
            cache,
            FlagLog(false),
            CvssFilter::All,
        );
        assert!(ar.get_records().is_empty());
    }

    #[test]
    fn test_audit_report_serialization() {
        let mock_get = r#"
        {"id":"GHSA-48cq-79qq-6f7x","summary":"Gradio applications running locally vulnerable to 3rd party websites accessing routes and uploading files","details":" Impact\nThis CVE covers the ability of 3rd party websites to access routes and upload files to users running Gradio applications locally.  For example, the malicious owners of [www.dontvisitme.com](http://www.dontvisitme.com/) could put a script on their website that uploads a large file to http://localhost:7860/upload and anyone who visits their website and has a Gradio app will now have that large file uploaded on their computer\n\n### Patches\nYes, the problem has been patched in Gradio version 4.19.2 or higher. We have no knowledge of this exploit being used against users of Gradio applications, but we encourage all users to upgrade to Gradio 4.19.2 or higher.\n\nFixed in: https://github.com/gradio-app/gradio/commit/84802ee6a4806c25287344dce581f9548a99834a\nCVE: https://nvd.nist.gov/vuln/detail/CVE-2024-1727","aliases":["CVE-2024-1727"],"modified":"2024-05-21T15:12:35.101662Z","published":"2024-05-21T14:43:50Z","database_specific":{"github_reviewed_at":"2024-05-21T14:43:50Z","github_reviewed":true,"severity":"MODERATE","cwe_ids":["CWE-352"],"nvd_published_at":null},"references":[{"type":"WEB","url":"https://github.com/gradio-app/gradio/security/advisories/GHSA-48cq-79qq-6f7x"},{"type":"ADVISORY","url":"https://nvd.nist.gov/vuln/detail/CVE-2024-1727"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/pull/7503"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/commit/84802ee6a4806c25287344dce581f9548a99834a"},{"type":"PACKAGE","url":"https://github.com/gradio-app/gradio"},{"type":"WEB","url":"https://huntr.com/bounties/a94d55fb-0770-4cbe-9b20-97a978a2ffff"}],"affected":[{"package":{"name":"gradio","ecosystem":"PyPI","purl":"pkg:pypi/gradio"},"ranges":[{"type":"ECOSYSTEM","events":[{"introduced":"0"},{"fixed":"4.19.2"}]}],"versions":["4.18.0","4.19.0","4.19.1","4.2.0","4.3.0","4.4.0","4.4.1","4.5.0","4.7.0","4.7.1","4.8.0","4.9.0","4.9.1"],"database_specific":{"source":"https://github.com/github/advisory-database/blob/main/advisories/github-reviewed/2024/05/GHSA-48cq-79qq-6f7x/GHSA-48cq-79qq-6f7x.json"}}],"schema_version":"1.6.0","severity":[{"type":"CVSS_V3","score":"CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L"}]}"#;

        let client = Arc::new(UreqClientMock {
            mock_post : Some("{\"results\":[{\"vulns\":[{\"id\":\"GHSA-48cq-79qq-6f7x\",\"modified\":\"2024-05-21T14:58:25.710902Z\"}]}]}".to_string()),
            mock_get : Some(mock_get.to_string()),
        });

        let packages =
            vec![Package::from_name_version_durl("gradio", "4.0.0", None).unwrap()];

        let cache_dir = path_cache(true).unwrap();
        let cache = CacheConfig::new(DURATION_0, &cache_dir);
        let ar = AuditReport::from_packages(
            client,
            &packages,
            FlagCacheRefresh(true),
            cache,
            FlagLog(false),
            CvssFilter::All,
        );
        let ar_json = serde_json::to_string_pretty(&ar).unwrap();
        let expected_json = r#"{"records":[{"package":{"name":"gradio","version":"4.0.0","key":"gradio","direct_url":null},"vuln_ids":["GHSA-48cq-79qq-6f7x"],"vuln_infos":{"GHSA-48cq-79qq-6f7x":{"id":"GHSA-48cq-79qq-6f7x","summary":"Gradio applications running locally vulnerable to 3rd party websites accessing routes and uploading files","references":[{"type":"WEB","url":"https://github.com/gradio-app/gradio/security/advisories/GHSA-48cq-79qq-6f7x"},{"type":"ADVISORY","url":"https://nvd.nist.gov/vuln/detail/CVE-2024-1727"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/pull/7503"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/commit/84802ee6a4806c25287344dce581f9548a99834a"},{"type":"PACKAGE","url":"https://github.com/gradio-app/gradio"},{"type":"WEB","url":"https://huntr.com/bounties/a94d55fb-0770-4cbe-9b20-97a978a2ffff"}],"cvss_details":[{"version":"V3_1","vector":"CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L","score":4.3,"severity":"medium"}]}}}]}"#;

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
