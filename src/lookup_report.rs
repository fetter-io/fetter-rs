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
use crate::util::ResultDynError;
use crate::CvssFilter;
use serde::Serialize;
use std::ops::Deref;
use std::sync::Arc;

#[derive(Debug, Serialize)]
pub struct LookupReport(pub AuditReport);

impl LookupReport {
    /// Get a LookupReport from a single `DepSpec`.
    pub fn from_dep_spec(
        client: Arc<dyn UreqClient>,
        ds: &DepSpec,
        limit: Option<usize>,
        cache_config: &CacheConfig,
        cache_refresh: FlagCacheRefresh,
        log: FlagLog,
        filter_cvss: CvssFilter,
    ) -> Self {
        // TODO: need to handle error here
        let pypi_project = query_pypi_project(client.clone(), &ds.key, cache_config, log);

        if let Some(ref project) = pypi_project {
            logger!(
                log,
                module_path!(),
                "Found {:?} releases in PyPI",
                project.get_releases_count()
            );
        }

        // convert VersionSpecs to Packages
        let packages: Vec<Package> = match pypi_project {
            Some(project) => project
                .get_version_specs(Some(ds), limit)
                .into_iter()
                .map(|version| Package {
                    name: ds.name.clone(),
                    key: name_to_key(&ds.name),
                    version,
                    direct_url: None,
                })
                .collect(),
            None => Vec::new(),
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
            true,
        );
        LookupReport(audit_report)
    }

    pub fn from_dep_manifest(
        client: Arc<dyn UreqClient>,
        dep_manifest: &DepManifest,
        env_marker_state: Option<&EnvMarkerState>,
        cache_config: &CacheConfig,
        cache_refresh: FlagCacheRefresh,
        log: FlagLog,
        filter_cvss: CvssFilter,
    ) -> ResultDynError<Self> {
        // Iterate through all dep_specs and collect them, flattening OOM variants
        let mut dep_specs: Vec<DepSpec> = Vec::new();

        for ds in dep_manifest.iter_dep_specs() {
            if ds.env_marker.is_empty() {
                dep_specs.push(ds.clone());
            } else if let Some(ems) = env_marker_state {
                if ds.validate_env_marker(ems) {
                    dep_specs.push(ds.clone());
                }
            }
        }
        let mut packages: Vec<Package> = Vec::new();
        for ds in dep_specs {
            let pypi_project =
                query_pypi_project(client.clone(), &ds.key, cache_config, log);

            // get one most-recent package that match DepSpec (ds) constraints
            if let Some(project) = pypi_project {
                packages.extend(
                    project
                        .get_version_specs(Some(&ds), Some(1))
                        .into_iter()
                        .map(|version| Package {
                            name: ds.name.clone(),
                            key: name_to_key(&ds.name),
                            version,
                            direct_url: None,
                        }),
                );
            }
        }

        let audit_report = AuditReport::from_packages(
            client,
            &packages,
            cache_refresh,
            cache_config.clone(),
            log,
            filter_cvss,
            true,
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
        );

        assert!(result.is_ok());
        let lookup_report = result.unwrap();
        let records = lookup_report.get_records();
        assert_eq!(records.len(), 0);
    }
}
