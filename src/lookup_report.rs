use crate::audit_report::AuditReport;
use crate::dep_spec::DepSpec;
use crate::package::Package;
use crate::pypi_project::query_pypi_project;
use crate::ureq_client::UreqClient;
use crate::util::logger;
use crate::util::name_to_key;
use crate::util::CacheConfig;
use crate::util::FlagCacheRefresh;
use crate::util::FlagLog;
use crate::CvssFilter;
use serde::Serialize;
use std::ops::Deref;
use std::sync::Arc;
use crate::util::ResultDynError;


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
        file_path: &Path,
        bound_options: Option<&Vec<String>>,
        env_marker_state: Option<&EnvMarkerState>,
        cache_config: &CacheConfig,
        cache_refresh: FlagCacheRefresh,
        log: FlagLog,
    ) -> ResultDynError<Self> {
        let dm = DepManifest::from_path_or_url(file_path, bound_options.as_ref())?;
        for key, dsoom in dm.dep_specs {
            let ds_iter = match dsoom {
                DepSpecOOM::One(ds) => std::slice::from_ref(ds).iter(),
                DepSpecOOM::Many(ds_vec) => ds_vec.iter(),
            };
            dss: Vec<DepSpec> = Vec::new();
            for ds in ds_iter {
                if ds.env_marker.is_empty() {
                    dss.push(ds);
                }
                else {
                    let ems = env_marker_state.expect("EMS should be loaded");
                    if ds.validate_env_marker(ems) {
                        dss.push(ds);
                    }

                }
            }
            for ds in dss {
                let pypi_project = query_pypi_project(client.clone(), &ds.key, cache_config, log);

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

            }
        }

    }
}

impl Deref for LookupReport {
    type Target = AuditReport;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
