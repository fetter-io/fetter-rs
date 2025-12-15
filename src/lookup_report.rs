use crate::audit_report::AuditReport;
use crate::dep_spec::DepSpec;
use crate::package::Package;
use crate::pypi_project::query_pypi_project;
use crate::ureq_client::UreqClient;
use crate::util::name_to_key;
use crate::util::CacheConfig;
use crate::util::FlagCacheRefresh;
use crate::util::FlagLog;
use crate::CvssFilter;
use std::ops::Deref;
use std::sync::Arc;
use serde::Serialize;


// given a set of packages (with defined specific versions), check if those versions have vulnerabilities; if so provide vulnerability details for each. Vuln details can reuse AuditRecord, AuditReport

// so what we need are two specialized constructors:
// from name_or_dep_spec(Option<limit>); if a name, get all, if a dep-spec, apply filtering... or just any string can be made into a dep spec
// from_dep_manifest

#[derive(Debug, Serialize)]
pub struct LookupReport(pub AuditReport);

impl LookupReport {
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

        println!("packages: {packages:?}");
        let audit_report = AuditReport::from_packages(
            client,
            &packages,
            cache_refresh,
            cache_config.clone(),
            log,
            filter_cvss,
        );
        LookupReport(audit_report)
    }
}

impl Deref for LookupReport {
    type Target = AuditReport;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
