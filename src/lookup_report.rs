use crate::audit_report::AuditReport;
use crate::dep_manifest::DepManifest;
use crate::dep_spec::DepSpec;
use crate::env_marker::EnvMarkerState;
use crate::package::Package;
use crate::pypi_project::query_pypi_project;
use crate::ureq_client::UreqClient;
use crate::util::logger;
use crate::util::name_to_key;
use crate::util::CacheConfig;
use crate::util::FlagCacheRefresh;
use crate::util::FlagLog;
use crate::util::FlagRetainPassing;
use crate::util::ResultDynError;
use crate::CvssFilter;
use rayon::prelude::*;
use serde::Serialize;
use std::ops::Deref;
use std::sync::Arc;

#[derive(Debug, Serialize)]
pub struct LookupReport(pub AuditReport);

impl LookupReport {
    /// Convert a DepSpec to a list of Packages by querying PyPI if the DepSpec is not a pinned resource.
    fn dep_spec_to_packages(
        client: Arc<dyn UreqClient>,
        ds: &DepSpec,
        limit: Option<usize>,
        cache_config: &CacheConfig,
        log: FlagLog,
    ) -> Option<Vec<Package>> {
        // if a package is pinned (exact) do not query pypi
        match ds.get_exact() {
            Some(version) => Some(vec![Package {
                name: ds.name.clone(),
                key: name_to_key(&ds.name),
                version,
                direct_url: None,
            }]),
            None => {
                query_pypi_project(client, &ds.key, cache_config, log)
                    .ok()
                    .map(|project| {
                        project
                            .get_version_specs(Some(ds), limit) // filter by ds, limit
                            .into_iter()
                            .map(|version| Package {
                                name: ds.name.clone(),
                                key: name_to_key(&ds.name),
                                version,
                                direct_url: None,
                            })
                            .collect()
                    })
            }
        }
    }

    /// Get a LookupReport from a single `DepSpec`.
    #[allow(clippy::too_many_arguments)]
    pub fn from_dep_spec(
        client: Arc<dyn UreqClient>,
        ds: &DepSpec,
        limit: Option<usize>,
        cache_config: &CacheConfig,
        cache_refresh: FlagCacheRefresh,
        log: FlagLog,
        filter_cvss: CvssFilter,
        retain_passing: FlagRetainPassing,
    ) -> ResultDynError<Self> {
        // Versions are sorted when returned here
        let packages: Vec<Package> = match ds.get_exact() {
            Some(version) => vec![Package {
                name: ds.name.clone(),
                key: name_to_key(&ds.name),
                version,
                direct_url: None,
            }],
            None => query_pypi_project(client.clone(), &ds.key, cache_config, log)?
                .get_version_specs(Some(ds), limit) // filter by DepSpec
                .into_iter()
                .map(|version| Package {
                    name: ds.name.clone(),
                    key: name_to_key(&ds.name),
                    version,
                    direct_url: None,
                })
                .collect(),
        };

        logger!(
            log,
            module_path!(),
            "Looking up vulns in {:?} packages",
            packages.len()
        );

        let audit_report = AuditReport::from_packages(
            client,
            &packages,
            cache_refresh,
            cache_config.clone(),
            log,
            filter_cvss,
            retain_passing,
        );
        Ok(LookupReport(audit_report))
    }

    /// Get a LookupReport from a single `DepManifest`.
    #[allow(clippy::too_many_arguments)]
    pub fn from_dep_manifest(
        client: Arc<dyn UreqClient>,
        dep_manifest: &DepManifest,
        env_marker_state: Option<&EnvMarkerState>,
        cache_config: &CacheConfig,
        cache_refresh: FlagCacheRefresh,
        log: FlagLog,
        filter_cvss: CvssFilter,
        retain_passing: FlagRetainPassing,
    ) -> ResultDynError<Self> {
        let mut dep_specs: Vec<DepSpec> = Vec::new();
        for ds in dep_manifest.iter_dep_specs() {
            if ds.env_marker.is_empty() {
                dep_specs.push(ds.clone());
            } else if let Some(ems) = env_marker_state {
                // NOTE: this will take any passing DepSpec that passes with this EMS; it does not limit that only one passes per package as we are iterating over all DepSpec
                if ds.validate_env_marker(ems) {
                    dep_specs.push(ds.clone());
                }
            }
        }

        let mut packages: Vec<Package> = dep_specs
            .par_iter()
            .filter_map(|ds| {
                Self::dep_spec_to_packages(
                    client.clone(),
                    ds,
                    Some(1), // Get only the most recent version
                    cache_config,
                    log,
                )
            })
            .flatten()
            .collect();

        packages.sort();

        let audit_report = AuditReport::from_packages(
            client,
            &packages,
            cache_refresh,
            cache_config.clone(),
            log,
            filter_cvss,
            retain_passing,
        );
        Ok(LookupReport(audit_report))
    }
}

impl Deref for LookupReport {
    type Target = AuditReport;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dep_spec::DepSpec;
    use crate::table::Tableable;
    use crate::ureq_client::UreqClientLive;
    use crate::util::path_cache;
    use crate::util::DURATION_0;
    use std::sync::Arc;

    #[test]
    #[ignore]
    fn test_from_dep_manifest_live() {
        // Create a synthetic DepManifest with some sample packages
        let dep_specs = vec![
            DepSpec::from_string("numpy==1.20.0").unwrap(),
            DepSpec::from_string("pandas>=2.0.0").unwrap(),
            DepSpec::from_string("requests>=1,<2").unwrap(),
        ];

        let dep_manifest = DepManifest::from_dep_specs(&dep_specs).unwrap();

        let client = Arc::new(UreqClientLive) as Arc<dyn UreqClient>;
        let cache_dir = path_cache(true).unwrap();
        let cache_config = CacheConfig::new(DURATION_0, cache_dir);
        let cache_refresh = FlagCacheRefresh(false);
        let retain_passing = FlagRetainPassing(false);
        let log = FlagLog(false);
        let filter_cvss = CvssFilter::All;

        let result = LookupReport::from_dep_manifest(
            client,
            &dep_manifest,
            None,
            &cache_config,
            cache_refresh,
            log,
            filter_cvss,
            retain_passing,
        );

        assert!(result.is_ok());
        let lookup_report = result.unwrap();

        // Verify we got some packages back
        let records = lookup_report.get_records();

        for record in records {
            println!(
                "Package: {}, Vuln IDs: {:?}",
                record.package, record.vuln_ids
            );
        }
    }

    #[test]
    fn test_from_dep_manifest_empty() {
        let dep_specs: Vec<DepSpec> = vec![];
        let dep_manifest = DepManifest::from_dep_specs(&dep_specs).unwrap();

        let client = Arc::new(UreqClientLive) as Arc<dyn UreqClient>;
        let cache_dir = path_cache(true).unwrap();
        let cache_config = CacheConfig::new(DURATION_0, cache_dir);
        let cache_refresh = FlagCacheRefresh(false);
        let log = FlagLog(false);
        let filter_cvss = CvssFilter::All;

        let result = LookupReport::from_dep_manifest(
            client,
            &dep_manifest,
            None,
            &cache_config,
            cache_refresh,
            log,
            filter_cvss,
            FlagRetainPassing(false),
        );

        assert!(result.is_ok());
        let lookup_report = result.unwrap();
        let records = lookup_report.get_records();
        assert_eq!(records.len(), 0);
    }

    #[test]
    fn test_from_dep_spec_mock() {
        use crate::ureq_client::UreqClientMock;
        use std::collections::HashMap;

        // Sample PyPI JSON for gradio package
        let pypi_json = r#"{"info":{"author":"Abubakar Abid","name":"gradio","project_urls":{"Homepage":"https://github.com/gradio-app/gradio"}},"releases":{"4.0.0":[{"filename":"gradio-4.0.0-py3-none-any.whl"}],"4.18.0":[{"filename":"gradio-4.18.0-py3-none-any.whl"}],"4.19.2":[{"filename":"gradio-4.19.2-py3-none-any.whl"}]}}"#;

        // Sample OSV batch query response
        let osv_batch_json = r#"{"results":[{"vulns":[{"id":"GHSA-48cq-79qq-6f7x","modified":"2024-05-21T14:58:25.710902Z"}]}]}"#;

        // Sample OSV vuln detail response
        let osv_vuln_json = r#"{"id":"GHSA-48cq-79qq-6f7x","summary":"Gradio applications running locally vulnerable to 3rd party websites accessing routes and uploading files","references":[{"type":"ADVISORY","url":"https://nvd.nist.gov/vuln/detail/CVE-2024-1727"}],"severity":[{"type":"CVSS_V3","score":"CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L"}]}"#;

        let mut mock_get_map = HashMap::new();
        mock_get_map.insert("https://pypi.org".to_string(), pypi_json.to_string());
        mock_get_map.insert("https://api.osv.dev".to_string(), osv_vuln_json.to_string());

        let mut mock_post_map = HashMap::new();
        mock_post_map.insert(
            "https://api.osv.dev".to_string(),
            osv_batch_json.to_string(),
        );

        let client = Arc::new(UreqClientMock {
            mock_get: Some(mock_get_map),
            mock_post: Some(mock_post_map),
        }) as Arc<dyn UreqClient>;

        let dep_spec = DepSpec::from_string("gradio>=4.0.0").unwrap();
        let cache_dir = path_cache(true).unwrap();
        let cache_config = CacheConfig::new(DURATION_0, cache_dir);

        let result = LookupReport::from_dep_spec(
            client,
            &dep_spec,
            Some(3), // limit to 3 most recent versions
            &cache_config,
            FlagCacheRefresh(true),
            FlagLog(false),
            CvssFilter::All,
            FlagRetainPassing(false),
        );

        assert!(result.is_ok());
        let lookup_report = result.unwrap();
        let records = lookup_report.get_records();

        // We should get 3 versions (4.0.0, 4.18.0, 4.19.2) but only one has vulns
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].package.name, "gradio");
        assert_eq!(records[0].vuln_ids, vec!["GHSA-48cq-79qq-6f7x"]);
    }

    #[test]
    fn test_from_dep_manifest_mock() {
        use crate::ureq_client::UreqClientMock;
        use std::collections::HashMap;

        // Sample PyPI JSON for two packages
        let pypi_gradio = r#"{"info":{"author":"Abubakar Abid","name":"gradio","project_urls":{"Homepage":"https://github.com/gradio-app/gradio"}},"releases":{"4.0.0":[{"filename":"gradio-4.0.0-py3-none-any.whl"}]}}"#;

        let pypi_numpy = r#"{"info":{"author":"NumPy Developers","name":"numpy","project_urls":{"Homepage":"https://numpy.org"}},"releases":{"1.19.1":[{"filename":"numpy-1.19.1-py3-none-any.whl"}]}}"#;

        // OSV batch response with gradio having vuln, numpy clean
        let osv_batch_json = r#"{"results":[{"vulns":[{"id":"GHSA-48cq-79qq-6f7x","modified":"2024-05-21T14:58:25.710902Z"}]},{"vulns":null}]}"#;

        // OSV vuln detail for gradio
        let osv_vuln_json = r#"{"id":"GHSA-48cq-79qq-6f7x","summary":"Gradio vulnerability","references":[{"type":"ADVISORY","url":"https://nvd.nist.gov/vuln/detail/CVE-2024-1727"}],"severity":[{"type":"CVSS_V3","score":"CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L"}]}"#;

        let mut mock_get_map = HashMap::new();
        mock_get_map.insert(
            "https://pypi.org/pypi/gradio".to_string(),
            pypi_gradio.to_string(),
        );
        mock_get_map.insert(
            "https://pypi.org/pypi/numpy".to_string(),
            pypi_numpy.to_string(),
        );
        mock_get_map.insert("https://api.osv.dev".to_string(), osv_vuln_json.to_string());

        let mut mock_post_map = HashMap::new();
        mock_post_map.insert(
            "https://api.osv.dev".to_string(),
            osv_batch_json.to_string(),
        );

        let client = Arc::new(UreqClientMock {
            mock_get: Some(mock_get_map),
            mock_post: Some(mock_post_map),
        }) as Arc<dyn UreqClient>;

        let dep_specs = vec![
            DepSpec::from_string("gradio==4.0.0").unwrap(),
            DepSpec::from_string("numpy==1.19.1").unwrap(),
        ];
        let dep_manifest = DepManifest::from_dep_specs(&dep_specs).unwrap();

        let cache_dir = path_cache(true).unwrap();
        let cache_config = CacheConfig::new(DURATION_0, cache_dir);

        let result = LookupReport::from_dep_manifest(
            client,
            &dep_manifest,
            None,
            &cache_config,
            FlagCacheRefresh(true),
            FlagLog(false),
            CvssFilter::All,
            FlagRetainPassing(false),
        );

        assert!(result.is_ok());
        let lookup_report = result.unwrap();
        let records = lookup_report.get_records();

        // Only gradio should appear in records since numpy is clean and retain_passing=false
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].package.name, "gradio");
        assert_eq!(records[0].package.version.to_string(), "4.0.0");
        assert_eq!(records[0].vuln_ids, vec!["GHSA-48cq-79qq-6f7x"]);
    }
}
