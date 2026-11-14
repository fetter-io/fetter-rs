use crate::allow_list::AllowList;
use crate::allow_list::ReqEntry;
use crate::audit_report::AuditReport;
use crate::audit_report::Unresolved;
use crate::dep_manifest::DepManifest;
use crate::dep_spec::DepSpec;
use crate::ecosystem::Ecosystem;
use crate::env_marker::EnvMarkerState;
use crate::package::Package;
use crate::pypi_project::query_pypi_project;
use crate::registry::is_not_found;
use crate::registry::list_versions;
use crate::ureq_client::UreqClient;
use crate::util::logger;
use crate::util::CacheConfig;
use crate::util::FlagCacheRefresh;
use crate::util::FlagLog;
use crate::util::FlagRetainPassing;
use crate::util::ResultDynError;
use crate::version_spec::VersionSpec;
use crate::CvssFilter;
use rayon::prelude::*;
use serde::Serialize;
use std::ops::Deref;
use std::sync::Arc;

#[derive(Debug, Serialize)]
pub struct LookupReport(pub AuditReport);

/// The outcome of resolving one requirement to packages. Errors are carried as strings so outcomes can cross threads.
enum Resolution {
    Found(Vec<Package>),
    /// The registry does not have the package.
    Unresolved(Unresolved),
    Failed(String),
}

impl Resolution {
    fn from_result(
        ecosystem: Ecosystem,
        requirement: String,
        result: ResultDynError<Vec<Package>>,
    ) -> Self {
        match result {
            Ok(p) => Resolution::Found(p),
            Err(e) if is_not_found(e.as_ref()) => Resolution::Unresolved(Unresolved {
                ecosystem,
                requirement,
            }),
            Err(e) => Resolution::Failed(e.to_string()),
        }
    }
}

/// Split `name@requirement` at the last `@` that is not the first character (scoped npm names start with `@`).
pub(crate) fn split_name_requirement(arg: &str) -> (&str, Option<&str>) {
    match arg[1.min(arg.len())..].rfind('@') {
        Some(i) => {
            let at = i + 1;
            let req = arg[at + 1..].trim();
            (arg[..at].trim(), (!req.is_empty()).then_some(req))
        }
        None => (arg.trim(), None),
    }
}

/// The exact version a requirement names, if it names exactly one: a bare version (npm, VS Code), `=version` (Cargo), or a single ref (GitHub Actions).
fn exact_version(eco: Ecosystem, spec: &str) -> Option<String> {
    let s = spec.trim();
    match eco {
        Ecosystem::Npm | Ecosystem::VsCode => {
            let v = s.strip_prefix('=').unwrap_or(s).trim();
            nodejs_semver::Version::parse(v).ok().map(|_| v.to_string())
        }
        Ecosystem::Cargo => {
            let v = s.strip_prefix('=')?.trim();
            semver::Version::parse(v).ok().map(|_| v.to_string())
        }
        Ecosystem::Gha => (s != "*" && !s.contains(',')).then(|| s.to_string()),
        Ecosystem::Python => None,
    }
}

/// Packages for a requirement of a non-Python ecosystem: the exact version if the requirement names one (with the registry consulted only for a canonical name, for Cargo and VS Code), otherwise the registry's versions that satisfy the requirement, the most recent `limit` if given. Registry failures are errors.
fn requirement_to_packages(
    client: &Arc<dyn UreqClient>,
    eco: Ecosystem,
    name: &str,
    entry: Option<&ReqEntry>,
    limit: Option<usize>,
    cache_config: &CacheConfig,
    log: FlagLog,
) -> ResultDynError<Vec<Package>> {
    let exact = entry
        .filter(|e| e.specs.len() == 1)
        .and_then(|e| exact_version(eco, &e.specs[0]));
    if let Some(v) = &exact {
        if matches!(eco, Ecosystem::Npm | Ecosystem::Gha) {
            return Ok(vec![Package::new(eco, name, VersionSpec::new(v), None)]);
        }
    }
    let rv = list_versions(client, eco, name, cache_config, log)?;
    if let Some(v) = exact {
        return Ok(vec![Package::new(
            eco,
            &rv.name,
            VersionSpec::new(&v),
            None,
        )]);
    }
    let mut packages: Vec<Package> = rv
        .versions
        .iter()
        .map(|v| Package::new(eco, &rv.name, VersionSpec::new(v), None))
        .filter(|p| entry.is_none_or(|e| e.matches(p)))
        .collect();
    if let Some(n) = limit {
        let skip = packages.len().saturating_sub(n);
        packages.drain(..skip);
    }
    Ok(packages)
}

impl LookupReport {
    /// Convert a DepSpec to a list of Packages by querying PyPI if the DepSpec is not a pinned resource. A package PyPI does not have is a `registry::NotFound` error.
    fn dep_spec_to_packages(
        client: Arc<dyn UreqClient>,
        ds: &DepSpec,
        limit: Option<usize>,
        cache_config: &CacheConfig,
        log: FlagLog,
    ) -> ResultDynError<Vec<Package>> {
        // if a package is pinned (exact) do not query pypi
        match ds.get_exact() {
            Some(version) => Ok(vec![Package::new(
                Ecosystem::Python,
                &ds.name,
                version,
                None,
            )]),
            None => Ok(query_pypi_project(client, &ds.key, cache_config, log)?
                .get_version_specs(Some(ds), limit) // filter by ds, limit
                .into_iter()
                .map(|version| Package::new(Ecosystem::Python, &ds.name, version, None))
                .collect()),
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
            Some(version) => {
                vec![Package::new(Ecosystem::Python, &ds.name, version, None)]
            }
            None => query_pypi_project(client.clone(), &ds.key, cache_config, log)?
                .get_version_specs(Some(ds), limit) // filter by DepSpec
                .into_iter()
                .map(|version| Package::new(Ecosystem::Python, &ds.name, version, None))
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
            cache_config,
            log,
            filter_cvss,
            retain_passing,
        )?;
        Ok(LookupReport(audit_report))
    }

    /// Get a LookupReport for a package of a non-Python ecosystem, given as `name` or `name@requirement` (the requirement in the ecosystem's native syntax, as in a fetter.toml). For GitHub Actions, a subpath (`owner/repo/path`) is reduced to the repository.
    #[allow(clippy::too_many_arguments)]
    pub fn from_name_requirement(
        client: Arc<dyn UreqClient>,
        ecosystem: Ecosystem,
        arg: &str,
        limit: Option<usize>,
        cache_config: &CacheConfig,
        cache_refresh: FlagCacheRefresh,
        log: FlagLog,
        filter_cvss: CvssFilter,
        retain_passing: FlagRetainPassing,
    ) -> ResultDynError<Self> {
        let (raw, req) = split_name_requirement(arg);
        let name = if ecosystem == Ecosystem::Gha {
            raw.split('/').take(2).collect::<Vec<_>>().join("/")
        } else {
            raw.to_string()
        };
        if name.is_empty() {
            return Err(format!("No package name in {arg:?}").into());
        }
        let entry = req
            .map(|r| ReqEntry::new(ecosystem, &name, vec![r.to_string()]))
            .transpose()?;
        let packages = requirement_to_packages(
            &client,
            ecosystem,
            &name,
            entry.as_ref(),
            limit,
            cache_config,
            log,
        )?;
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
            cache_config,
            log,
            filter_cvss,
            retain_passing,
        )?;
        Ok(LookupReport(audit_report))
    }

    /// Get a LookupReport from an allow list (a fetter.toml or any Python bound): for each requirement, the most recent satisfying version, for the provided ecosystems. Python requirements are resolved as in `from_dep_manifest` (including env markers). A requirement whose package is not in its registry is reported in `unresolved`; any other failure is an error.
    #[allow(clippy::too_many_arguments)]
    pub fn from_allow_list(
        client: Arc<dyn UreqClient>,
        allow_list: &AllowList,
        ecosystems: &[Ecosystem],
        env_marker_state: Option<&EnvMarkerState>,
        cache_config: &CacheConfig,
        cache_refresh: FlagCacheRefresh,
        log: FlagLog,
        filter_cvss: CvssFilter,
        retain_passing: FlagRetainPassing,
    ) -> ResultDynError<Self> {
        let mut resolutions: Vec<Resolution> = Vec::new();
        if ecosystems.contains(&Ecosystem::Python) {
            if let Some(dm) = &allow_list.python {
                resolutions.extend(Self::dep_manifest_resolutions(
                    client.clone(),
                    dm,
                    env_marker_state,
                    cache_config,
                    log,
                ));
            }
        }
        let entries: Vec<(Ecosystem, &ReqEntry)> = allow_list
            .sections
            .iter()
            .filter(|(e, _)| ecosystems.contains(e))
            .flat_map(|(e, s)| s.iter().map(move |(_, entry)| (*e, entry)))
            .collect();
        resolutions.par_extend(entries.par_iter().map(|(eco, entry)| {
            Resolution::from_result(
                *eco,
                entry.to_string(),
                requirement_to_packages(
                    &client,
                    *eco,
                    &entry.name,
                    Some(entry),
                    Some(1),
                    cache_config,
                    log,
                ),
            )
        }));
        Self::from_resolutions(
            client,
            resolutions,
            cache_config,
            cache_refresh,
            log,
            filter_cvss,
            retain_passing,
        )
    }

    /// Resolve each Python requirement whose env marker (if any) applies to its most recent satisfying version.
    fn dep_manifest_resolutions(
        client: Arc<dyn UreqClient>,
        dep_manifest: &DepManifest,
        env_marker_state: Option<&EnvMarkerState>,
        cache_config: &CacheConfig,
        log: FlagLog,
    ) -> Vec<Resolution> {
        let dep_specs: Vec<DepSpec> = dep_manifest
            .iter_dep_specs()
            .filter(|ds| {
                ds.env_marker.is_empty()
                    || env_marker_state.is_some_and(|ems| ds.validate_env_marker(ems))
            })
            .cloned()
            .collect();
        dep_specs
            .par_iter()
            .map(|ds| {
                Resolution::from_result(
                    Ecosystem::Python,
                    ds.to_string(),
                    Self::dep_spec_to_packages(
                        client.clone(),
                        ds,
                        Some(1),
                        cache_config,
                        log,
                    ),
                )
            })
            .collect()
    }

    /// Audit the packages of all found requirements; record unresolved ones; fail on the first failure.
    fn from_resolutions(
        client: Arc<dyn UreqClient>,
        resolutions: Vec<Resolution>,
        cache_config: &CacheConfig,
        cache_refresh: FlagCacheRefresh,
        log: FlagLog,
        filter_cvss: CvssFilter,
        retain_passing: FlagRetainPassing,
    ) -> ResultDynError<Self> {
        let mut packages: Vec<Package> = Vec::new();
        let mut unresolved: Vec<Unresolved> = Vec::new();
        for r in resolutions {
            match r {
                Resolution::Found(p) => packages.extend(p),
                Resolution::Unresolved(u) => unresolved.push(u),
                Resolution::Failed(e) => return Err(e.into()),
            }
        }
        packages.sort();
        unresolved.sort_by(|a, b| {
            (a.ecosystem, &a.requirement).cmp(&(b.ecosystem, &b.requirement))
        });
        if !unresolved.is_empty() {
            logger!(
                log,
                module_path!(),
                "Unresolved requirements: {:?}",
                unresolved
            );
        }
        let mut audit_report = AuditReport::from_packages(
            client,
            &packages,
            cache_refresh,
            cache_config,
            log,
            filter_cvss,
            retain_passing,
        )?;
        audit_report.unresolved = unresolved;
        Ok(LookupReport(audit_report))
    }

    /// Get a LookupReport from a single `DepManifest`. A requirement whose package is not on PyPI is reported in `unresolved`; any other failure is an error.
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
        let resolutions = Self::dep_manifest_resolutions(
            client.clone(),
            dep_manifest,
            env_marker_state,
            cache_config,
            log,
        );
        Self::from_resolutions(
            client,
            resolutions,
            cache_config,
            cache_refresh,
            log,
            filter_cvss,
            retain_passing,
        )
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
    use crate::audit_report::Unresolved;
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
    fn test_from_dep_spec_mock() {
        use crate::ureq_client::UreqClientMock;
        use std::collections::HashMap;

        // Sample PyPI JSON for gradio package
        let pypi_json = r#"{"info":{"author":"Abubakar Abid","name":"gradio","project_urls":{"Homepage":"https://github.com/gradio-app/gradio"}},"releases":{"4.0.0":[{"filename":"gradio-4.0.0-py3-none-any.whl"}],"4.18.0":[{"filename":"gradio-4.18.0-py3-none-any.whl"}],"4.19.2":[{"filename":"gradio-4.19.2-py3-none-any.whl"}]}}"#;

        // Sample OSV batch query response: one result per queried version (4.0.0, 4.18.0, 4.19.2), only the first vulnerable
        let osv_batch_json = r#"{"results":[{"vulns":[{"id":"GHSA-48cq-79qq-6f7x","modified":"2024-05-21T14:58:25.710902Z"}]},{},{}]}"#;

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
        assert_eq!(records[0].package.version.to_string(), "4.0.0");
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

    #[test]
    fn test_split_name_requirement_a() {
        assert_eq!(
            split_name_requirement("lodash@^4.17"),
            ("lodash", Some("^4.17"))
        );
        assert_eq!(
            split_name_requirement("@babel/core@>=7.24"),
            ("@babel/core", Some(">=7.24"))
        );
        assert_eq!(split_name_requirement("@babel/core"), ("@babel/core", None));
        assert_eq!(split_name_requirement("serde"), ("serde", None));
        assert_eq!(split_name_requirement("serde@"), ("serde", None));
        assert_eq!(
            split_name_requirement("actions/checkout@v4"),
            ("actions/checkout", Some("v4"))
        );
        assert_eq!(split_name_requirement(""), ("", None));
    }

    #[test]
    fn test_exact_version_a() {
        assert_eq!(
            exact_version(Ecosystem::Npm, "4.17.15"),
            Some("4.17.15".to_string())
        );
        assert_eq!(
            exact_version(Ecosystem::Npm, "=4.17.15"),
            Some("4.17.15".to_string())
        );
        assert_eq!(exact_version(Ecosystem::Npm, "^4.17.15"), None);
        // a bare cargo version is a caret requirement, not exact
        assert_eq!(exact_version(Ecosystem::Cargo, "1.0.200"), None);
        assert_eq!(
            exact_version(Ecosystem::Cargo, "=1.0.200"),
            Some("1.0.200".to_string())
        );
        assert_eq!(exact_version(Ecosystem::Gha, "v4"), Some("v4".to_string()));
        assert_eq!(exact_version(Ecosystem::Gha, "*"), None);
    }

    fn mock_client(get: &[(&str, &str)], post: &str) -> Arc<dyn UreqClient> {
        use crate::ureq_client::UreqClientMock;
        use std::collections::HashMap;
        let mut g = HashMap::new();
        for (k, v) in get {
            g.insert(k.to_string(), v.to_string());
        }
        let mut p = HashMap::new();
        p.insert(
            "https://api.osv.dev/v1/querybatch".to_string(),
            post.to_string(),
        );
        Arc::new(UreqClientMock {
            mock_post: Some(p),
            mock_get: Some(g),
        })
    }

    #[test]
    fn test_from_name_requirement_npm_a() {
        // three versions satisfy ^4.17.0; limit 2 takes the most recent two; neither is vulnerable
        let client = mock_client(
            &[(
                "https://registry.npmjs.org/lodash",
                r#"{"name": "lodash", "versions": {"4.16.0": {}, "4.17.15": {}, "4.17.20": {}, "4.17.21": {}}}"#,
            )],
            r#"{"results": [{}, {}]}"#,
        );
        let cc = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let lr = LookupReport::from_name_requirement(
            client,
            Ecosystem::Npm,
            "lodash@^4.17.0",
            Some(2),
            &cc,
            FlagCacheRefresh(false),
            FlagLog(false),
            CvssFilter::All,
            FlagRetainPassing(true),
        )
        .unwrap();
        let versions: Vec<String> = lr
            .records
            .iter()
            .map(|r| r.package.version.to_string())
            .collect();
        assert_eq!(versions, vec!["4.17.20", "4.17.21"]);
        assert!(lr
            .records
            .iter()
            .all(|r| r.package.ecosystem == Ecosystem::Npm));
    }

    #[test]
    fn test_from_name_requirement_errors_a() {
        let client = mock_client(&[], r#"{"results": []}"#);
        let cc = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let call = |arg: &str| {
            LookupReport::from_name_requirement(
                client.clone(),
                Ecosystem::Npm,
                arg,
                None,
                &cc,
                FlagCacheRefresh(false),
                FlagLog(false),
                CvssFilter::All,
                FlagRetainPassing(false),
            )
        };
        // not in the registry
        assert!(call("no-such-package").is_err());
        // an invalid requirement
        assert!(call("lodash@^^^").is_err());
    }

    #[test]
    fn test_from_dep_manifest_unresolved_a() {
        // gradio is on (mock) PyPI; internal-pkg is not: it is unresolved, and the lookup still audits gradio
        let pypi_gradio = r#"{"info":{"author":"A","name":"gradio","project_urls":{"Homepage":"https://x"}},"releases":{"4.0.0":[{"filename":"gradio-4.0.0-py3-none-any.whl"}]}}"#;
        let client = mock_client(
            &[("https://pypi.org/pypi/gradio/json", pypi_gradio)],
            r#"{"results": [{}]}"#,
        );
        let dm =
            DepManifest::try_from_iter(vec!["gradio>=4", "internal-pkg>=1"]).unwrap();
        let cc = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let lr = LookupReport::from_dep_manifest(
            client,
            &dm,
            None,
            &cc,
            FlagCacheRefresh(false),
            FlagLog(false),
            CvssFilter::All,
            FlagRetainPassing(true),
        )
        .unwrap();
        assert_eq!(lr.records.len(), 1);
        assert_eq!(lr.records[0].package.name, "gradio");
        assert_eq!(
            lr.unresolved,
            vec![Unresolved {
                ecosystem: Ecosystem::Python,
                requirement: "internal-pkg>=1".to_string()
            }]
        );
        let json = serde_json::to_string(&lr).unwrap();
        assert!(
            json.contains(
                r#""unresolved":[{"ecosystem":"python","requirement":"internal-pkg>=1"}]"#
            ),
            "{json}"
        );
    }

    /// A client whose GET requests fail with a server error.
    struct FailingClient;
    impl UreqClient for FailingClient {
        fn post(&self, _url: &str, _body: &str) -> Result<String, ureq::Error> {
            Err(ureq::Error::StatusCode(500))
        }
        fn get(&self, _url: &str) -> Result<String, ureq::Error> {
            Err(ureq::Error::StatusCode(500))
        }
    }

    #[test]
    fn test_from_dep_manifest_failure_is_error_a() {
        // a server error is not "not found": the lookup fails rather than reporting nothing
        let dm = DepManifest::try_from_iter(vec!["gradio>=4"]).unwrap();
        let cc = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let r = LookupReport::from_dep_manifest(
            Arc::new(FailingClient),
            &dm,
            None,
            &cc,
            FlagCacheRefresh(false),
            FlagLog(false),
            CvssFilter::All,
            FlagRetainPassing(false),
        );
        let err = r.unwrap_err().to_string();
        assert!(err.contains("500"), "{err}");
    }

    #[test]
    fn test_from_allow_list_unresolved_a() {
        let client = mock_client(
            &[(
                "https://registry.npmjs.org/lodash",
                r#"{"name": "lodash", "versions": {"4.17.21": {}}}"#,
            )],
            r#"{"results": [{}]}"#,
        );
        let al = AllowList::from_fetter_toml(
            "[python]\nrequirements = [\"internal-pkg>=1\"]\n[npm]\n\"lodash\" = \"^4\"\n\"@corp/private\" = \"^1\"\n",
        )
        .unwrap();
        let cc = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let lr = LookupReport::from_allow_list(
            client,
            &al,
            &Ecosystem::ALL,
            None,
            &cc,
            FlagCacheRefresh(false),
            FlagLog(false),
            CvssFilter::All,
            FlagRetainPassing(true),
        )
        .unwrap();
        assert_eq!(lr.records.len(), 1);
        assert_eq!(lr.records[0].package.to_string(), "lodash-4.17.21");
        let unresolved: Vec<String> = lr
            .unresolved
            .iter()
            .map(|u| format!("{} {}", u.ecosystem, u.requirement))
            .collect();
        assert_eq!(
            unresolved,
            vec!["python internal-pkg>=1", "npm @corp/private ^1"]
        );
    }
}
