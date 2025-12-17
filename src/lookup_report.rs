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
        let pypi_project =
            query_pypi_project(client.clone(), &ds.key, cache_config, log)?;

        let packages: Vec<Package> = pypi_project
            .get_version_specs(Some(ds), limit)
            .into_iter()
            .map(|version| Package {
                name: ds.name.clone(),
                key: name_to_key(&ds.name),
                version,
                direct_url: None,
            })
            .collect();

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
                if ds.validate_env_marker(ems) {
                    dep_specs.push(ds.clone());
                }
            }
        }
        // TODO: for pinned DepSpec, do not need to query pypi
        let packages: Vec<Package> = dep_specs
            .par_iter()
            .filter_map(|ds| {
                query_pypi_project(client.clone(), &ds.key, cache_config, log)
                    .ok()
                    .map(|project| {
                        project
                            .get_version_specs(Some(ds), Some(1)) // filter with DepSpec
                            .into_iter()
                            .map(|version| Package {
                                name: ds.name.clone(),
                                key: name_to_key(&ds.name),
                                version,
                                direct_url: None,
                            })
                            .collect::<Vec<_>>()
                    })
            })
            .flatten()
            .collect();

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
}
