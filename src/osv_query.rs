use crate::util::DURATION_0;
use std::fs::File;
use std::io::Read;
use std::sync::Arc;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

// use crate::package::Package;
use crate::util::{
    hash_string, logger, path_within_duration, CacheConfig, FlagLog, ResultDynError,
};
use crate::{package::Package, ureq_client::UreqClient};

//------------------------------------------------------------------------------
// see https://google.github.io/osv.dev/post-v1-querybatch/

// OSV request component
#[derive(Serialize, Deserialize, Debug, Clone)]
struct OSVPackage {
    name: String,
    ecosystem: String,
}

/// OSV request component
#[derive(Serialize, Deserialize, Debug, Clone)]
struct OSVPackageQuery {
    package: OSVPackage,
    version: String,
    // note: commit can go here
    /// Set only on follow-up requests for a query whose previous result had a `next_page_token`; omitted otherwise, so cache keys are unaffected.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    page_token: Option<String>,
}

impl OSVPackageQuery {
    fn from_package(package: &Package) -> Self {
        OSVPackageQuery {
            package: OSVPackage {
                name: package.name.clone(),
                ecosystem: package.ecosystem.osv_name().to_string(),
            },
            version: package.version.to_string(),
            page_token: None,
        }
    }
}

/// OSV request component
#[derive(Serialize, Deserialize, Debug)]
struct OSVQueryBatch {
    queries: Vec<OSVPackageQuery>,
}

/// OSV response component
#[derive(Serialize, Deserialize, Debug, Clone)]
struct OSVVuln {
    id: String,
    modified: String,
}

/// OSV response component
#[derive(Serialize, Deserialize, Debug, Clone)]
struct OSVQueryResult {
    vulns: Option<Vec<OSVVuln>>,
    /// Present if more vulnerabilities remain for this query.
    next_page_token: Option<String>,
}

/// OSV response component
#[derive(Serialize, Deserialize, Debug, Clone)]
struct OSVResponse {
    results: Vec<OSVQueryResult>,
}

//------------------------------------------------------------------------------

const OSV_BATCH_URL: &str = "https://api.osv.dev/v1/querybatch";
const OSV_BATCH_SIZE: usize = 64;
/// Upper bound on requests per batch while following pagination, to guard against a server that never stops returning tokens.
const OSV_MAX_PAGES: usize = 100;

/// Send one request to the OSV batch API and return the results, which must align one-to-one with `queries`.
fn post_osv_batch(
    client: &Arc<dyn UreqClient>,
    queries: Vec<OSVPackageQuery>,
) -> Result<Vec<OSVQueryResult>, String> {
    let count = queries.len();
    let body =
        serde_json::to_string(&OSVQueryBatch { queries }).map_err(|e| e.to_string())?;
    let body_str = client
        .post(OSV_BATCH_URL, &body)
        .map_err(|e| format!("OSV query failed: {e}"))?;
    let osv_res: OSVResponse = serde_json::from_str(&body_str)
        .map_err(|e| format!("OSV response could not be parsed: {e}"))?;
    if osv_res.results.len() != count {
        return Err(format!(
            "OSV response has {} results for {count} queries",
            osv_res.results.len()
        ));
    }
    Ok(osv_res.results)
}

/// Send a batch of queries to the OSV API, following pagination, and return a Vec of vulnerability IDs per query (None if no vulnerabilities). OSV paginates when a query has more than ~1,000 vulnerabilities or the batch more than ~3,000 in total; follow-up requests re-send only the queries with a `next_page_token`, each with its `page_token`. Any failure (network, HTTP status, unparseable or misaligned response, too many pages) is an error: it must never be reported as "no vulnerabilities".
fn query_osv_batch(
    client: Arc<dyn UreqClient>,
    queries: &[OSVPackageQuery],
) -> Result<Vec<Option<Vec<String>>>, String> {
    let mut vulns: Vec<Vec<String>> = vec![Vec::new(); queries.len()];
    // (index into `queries`, page token) for queries with more pages
    let mut pending: Vec<(usize, Option<String>)> =
        (0..queries.len()).map(|i| (i, None)).collect();
    let mut pages = 0;

    while !pending.is_empty() {
        pages += 1;
        if pages > OSV_MAX_PAGES {
            return Err(format!("OSV pagination exceeded {OSV_MAX_PAGES} pages"));
        }
        let request: Vec<OSVPackageQuery> = pending
            .iter()
            .map(|(i, token)| OSVPackageQuery {
                page_token: token.clone(),
                ..queries[*i].clone()
            })
            .collect();
        let results = post_osv_batch(&client, request)?;

        let mut next = Vec::new();
        for ((i, _), result) in pending.iter().zip(results) {
            if let Some(vs) = result.vulns {
                vulns[*i].extend(vs.into_iter().map(|v| v.id));
            }
            if let Some(token) = result.next_page_token.filter(|t| !t.is_empty()) {
                next.push((*i, Some(token)));
            }
        }
        pending = next;
    }
    Ok(vulns
        .into_iter()
        .map(|mut ids| {
            if ids.is_empty() {
                None
            } else {
                // pages should not overlap, but never report an ID twice
                let mut seen = std::collections::HashSet::new();
                ids.retain(|id| seen.insert(id.clone()));
                Some(ids)
            }
        })
        .collect())
}

/// Return true if OSV can meaningfully be queried for this package. An empty version is excluded: OSV treats a missing version as "all versions", which would report every advisory ever published for the package.
pub(crate) fn is_osv_queryable(package: &Package) -> bool {
    !package.version.to_string().trim().is_empty()
}

/// Given a slice of Packages, get all vulnerabilities known for each package. Packages may be from mixed ecosystems. All packages must be queryable (see `is_osv_queryable`). Results are cached only if all batches succeed.
pub(crate) fn query_osv_batches(
    client: Arc<dyn UreqClient>,
    packages: &[Package],
    cache_config: &CacheConfig,
    log: FlagLog,
) -> ResultDynError<Vec<Option<Vec<String>>>> {
    if packages.is_empty() {
        return Ok(Vec::new());
    }
    let packages_osv: Vec<OSVPackageQuery> =
        packages.iter().map(OSVPackageQuery::from_package).collect();

    let query_api = || -> ResultDynError<Vec<Option<Vec<String>>>> {
        let batches: Result<Vec<_>, String> = packages_osv
            .par_chunks(OSV_BATCH_SIZE)
            .map(|chunk| query_osv_batch(client.clone(), chunk))
            .collect();
        Ok(batches?.into_iter().flatten().collect())
    };

    if cache_config.duration == DURATION_0 {
        // do not read or write cache_config
        logger!(log, module_path!(), "Cache OSV batch disabled by duration");
        return query_api();
    }

    let json =
        serde_json::to_string(&packages_osv).expect("Failed to serialize packages_osv");
    let cache_key = hash_string(&json);

    let cache_fp = cache_config
        .directory
        .join(format!("osv_batch_{cache_key}"))
        .with_extension("json");

    if path_within_duration(&cache_fp, cache_config.duration) {
        logger!(
            log,
            module_path!(),
            "Loading OSV batch cache: {:?}",
            cache_fp
        );
        if let Ok(mut file) = File::open(&cache_fp) {
            let mut contents = String::new();
            if file.read_to_string(&mut contents).is_ok() {
                if let Ok(cached_results) =
                    serde_json::from_str::<Vec<Option<Vec<String>>>>(&contents)
                {
                    if cached_results.len() == packages.len() {
                        return Ok(cached_results);
                    }
                }
            }
        }
    }
    // full fetch; on error, return before writing the cache
    let results = query_api()?;
    if let Ok(json) = serde_json::to_string(&results) {
        logger!(
            log,
            module_path!(),
            "Writing OSV batch cache: {:?}",
            cache_fp
        );
        let _ = std::fs::write(&cache_fp, json);
    }
    Ok(results)
}

//--------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecosystem::Ecosystem;
    use crate::version_spec::VersionSpec;
    use crate::{ureq_client::UreqClientMock, util::path_cache};
    use std::collections::HashMap;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Duration;

    #[test]
    fn test_osv_querybatch_a() {
        let mut mock_post_map = HashMap::new();
        mock_post_map.insert("https://api.osv.dev".to_string(), "{\"results\":[{\"vulns\":[{\"id\":\"GHSA-34rf-p3r3-58x2\",\"modified\":\"2024-05-06T14:46:47.572046Z\"},{\"id\":\"GHSA-3f95-mxq2-2f63\",\"modified\":\"2024-04-10T22:19:39.095481Z\"},{\"id\":\"GHSA-48cq-79qq-6f7x\",\"modified\":\"2024-05-21T14:58:25.710902Z\"}]},{\"vulns\":[{\"id\":\"GHSA-pmv9-3xqp-8w42\",\"modified\":\"2024-09-18T19:36:03.377591Z\"}]}]}".to_string());

        let client = Arc::new(UreqClientMock {
            mock_post: Some(mock_post_map),
            mock_get: None,
        });
        let packages = vec![
            Package::from_name_version_durl("gradio", "4.0.0", None).unwrap(),
            Package::from_name_version_durl("mesop", "0.11.1", None).unwrap(),
        ];

        let cache_dir = path_cache(true).unwrap();
        let cache_config = CacheConfig::new(DURATION_0, cache_dir);
        let results =
            query_osv_batches(client, &packages, &cache_config, FlagLog(false)).unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0],
            Some(vec![
                "GHSA-34rf-p3r3-58x2".to_string(),
                "GHSA-3f95-mxq2-2f63".to_string(),
                "GHSA-48cq-79qq-6f7x".to_string()
            ])
        );
        assert_eq!(results[1], Some(vec!["GHSA-pmv9-3xqp-8w42".to_string()]));
    }

    fn pkg(eco: Ecosystem, name: &str, version: &str) -> Package {
        Package::new(eco, name, VersionSpec::new(version), None)
    }

    fn mock_post(response: Option<&str>) -> Arc<dyn UreqClient> {
        let mock_post = response.map(|r| {
            let mut m = HashMap::new();
            m.insert("https://api.osv.dev".to_string(), r.to_string());
            m
        });
        Arc::new(UreqClientMock {
            mock_post,
            mock_get: None,
        })
    }

    #[test]
    fn test_osv_query_mixed_ecosystems_a() {
        let queries: Vec<OSVPackageQuery> = [
            pkg(Ecosystem::Python, "jinja2", "2.4.1"),
            pkg(Ecosystem::Npm, "lodash", "4.17.15"),
            pkg(Ecosystem::Cargo, "time", "0.1.43"),
            pkg(Ecosystem::Gha, "actions/checkout", "4.1.0"),
            pkg(Ecosystem::VsCode, "ms-python.python", "2024.1.0"),
        ]
        .iter()
        .map(OSVPackageQuery::from_package)
        .collect();
        let json = serde_json::to_string(&OSVQueryBatch { queries }).unwrap();
        assert_eq!(
            json,
            r#"{"queries":[{"package":{"name":"jinja2","ecosystem":"PyPI"},"version":"2.4.1"},{"package":{"name":"lodash","ecosystem":"npm"},"version":"4.17.15"},{"package":{"name":"time","ecosystem":"crates.io"},"version":"0.1.43"},{"package":{"name":"actions/checkout","ecosystem":"GitHub Actions"},"version":"4.1.0"},{"package":{"name":"ms-python.python","ecosystem":"VSCode"},"version":"2024.1.0"}]}"#
        );
    }

    #[test]
    fn test_osv_querybatch_mixed_ecosystems_b() {
        // results are aligned to queries regardless of ecosystem
        let client = mock_post(Some(
            r#"{"results":[{"vulns":[{"id":"GHSA-py","modified":"x"}]},{},{"vulns":[{"id":"RUSTSEC-2020-0071","modified":"x"}]}]}"#,
        ));
        let packages = vec![
            pkg(Ecosystem::Python, "jinja2", "2.4.1"),
            pkg(Ecosystem::Npm, "left-pad", "1.3.0"),
            pkg(Ecosystem::Cargo, "time", "0.1.43"),
        ];
        let cache_config = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let results =
            query_osv_batches(client, &packages, &cache_config, FlagLog(false)).unwrap();
        assert_eq!(
            results,
            vec![
                Some(vec!["GHSA-py".to_string()]),
                None,
                Some(vec!["RUSTSEC-2020-0071".to_string()])
            ]
        );
    }

    #[test]
    fn test_osv_querybatch_empty_a() {
        // no request is made, so a client without responses is fine
        let cache_config = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let results =
            query_osv_batches(mock_post(None), &[], &cache_config, FlagLog(false))
                .unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_osv_querybatch_errors_a() {
        let packages = vec![pkg(Ecosystem::Python, "jinja2", "2.4.1")];
        let cache_config = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        // request failure
        assert!(query_osv_batches(
            mock_post(None),
            &packages,
            &cache_config,
            FlagLog(false)
        )
        .is_err());
        // unparseable response (e.g. an HTTP 400 body)
        let bad = r#"{"code":3,"message":"invalid ecosystem"}"#;
        assert!(query_osv_batches(
            mock_post(Some(bad)),
            &packages,
            &cache_config,
            FlagLog(false)
        )
        .is_err());
        // misaligned response
        let short = r#"{"results":[]}"#;
        assert!(query_osv_batches(
            mock_post(Some(short)),
            &packages,
            &cache_config,
            FlagLog(false)
        )
        .is_err());
    }

    #[test]
    fn test_osv_querybatch_failure_not_cached_a() {
        let dir = tempfile::tempdir().unwrap();
        let cache_config =
            CacheConfig::new(Duration::from_secs(600), dir.path().to_path_buf());
        let packages = vec![pkg(Ecosystem::Npm, "lodash", "4.17.15")];
        assert!(query_osv_batches(
            mock_post(None),
            &packages,
            &cache_config,
            FlagLog(false)
        )
        .is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

        // a success is cached, and then served from cache even if the API fails
        let ok =
            r#"{"results":[{"vulns":[{"id":"GHSA-29mw-wpgm-hmr9","modified":"x"}]}]}"#;
        let r1 = query_osv_batches(
            mock_post(Some(ok)),
            &packages,
            &cache_config,
            FlagLog(false),
        )
        .unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        let r2 =
            query_osv_batches(mock_post(None), &packages, &cache_config, FlagLog(false))
                .unwrap();
        assert_eq!(r1, r2);
    }

    #[test]
    #[ignore] // hits the live OSV API
    fn test_osv_ecosystem_names_live() {
        // OSV rejects a whole batch (HTTP 400) if any ecosystem name is invalid
        let client: Arc<dyn UreqClient> = Arc::new(crate::ureq_client::UreqClientLive);
        let packages: Vec<Package> = Ecosystem::ALL
            .iter()
            .map(|e| pkg(*e, "fetter-nonexistent-package", "1.0.0"))
            .collect();
        let cache_config = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        let results =
            query_osv_batches(client.clone(), &packages, &cache_config, FlagLog(false))
                .unwrap();
        assert_eq!(results.len(), Ecosystem::ALL.len());

        // known-vulnerable versions are reported for ecosystems with server-side version matching
        let packages = vec![
            pkg(Ecosystem::Python, "jinja2", "2.4.1"),
            pkg(Ecosystem::Npm, "lodash", "4.17.15"),
            pkg(Ecosystem::Cargo, "time", "0.1.43"),
        ];
        let results =
            query_osv_batches(client, &packages, &cache_config, FlagLog(false)).unwrap();
        assert!(results
            .iter()
            .all(|r| r.as_ref().is_some_and(|v| !v.is_empty())));
    }

    /// A client that returns queued responses in order and records each request body.
    struct SeqClient {
        responses: Mutex<VecDeque<String>>,
        bodies: Mutex<Vec<String>>,
    }

    impl SeqClient {
        fn new(responses: &[&str]) -> Arc<Self> {
            Arc::new(SeqClient {
                responses: Mutex::new(responses.iter().map(|r| r.to_string()).collect()),
                bodies: Mutex::new(Vec::new()),
            })
        }
        fn bodies(&self) -> Vec<String> {
            self.bodies.lock().unwrap().clone()
        }
    }

    impl UreqClient for SeqClient {
        fn post(&self, _url: &str, body: &str) -> Result<String, ureq::Error> {
            self.bodies.lock().unwrap().push(body.to_string());
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| std::io::Error::other("no more responses").into())
        }
        fn get(&self, _url: &str) -> Result<String, ureq::Error> {
            Err(std::io::Error::other("unused").into())
        }
    }

    fn query_ids(
        client: Arc<dyn UreqClient>,
        packages: &[Package],
    ) -> Vec<Option<Vec<String>>> {
        let cache_config = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        query_osv_batches(client, packages, &cache_config, FlagLog(false)).unwrap()
    }

    fn ids(v: &[&str]) -> Option<Vec<String>> {
        Some(v.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn test_osv_pagination_a() {
        let client = SeqClient::new(&[
            r#"{"results":[{"vulns":[{"id":"A","modified":"x"}],"next_page_token":"t0"},{"vulns":[{"id":"B","modified":"x"}]}]}"#,
            r#"{"results":[{"vulns":[{"id":"C","modified":"x"}]}]}"#,
        ]);
        let packages = vec![
            pkg(Ecosystem::Python, "jinja2", "2.4.1"),
            pkg(Ecosystem::Npm, "lodash", "4.17.15"),
        ];
        let results = query_ids(client.clone(), &packages);
        assert_eq!(results, vec![ids(&["A", "C"]), ids(&["B"])]);

        // the follow-up re-sends only the paginated query, with its page token
        let bodies = client.bodies();
        assert_eq!(bodies.len(), 2);
        assert!(!bodies[0].contains("page_token"));
        assert_eq!(
            bodies[1],
            r#"{"queries":[{"package":{"name":"jinja2","ecosystem":"PyPI"},"version":"2.4.1","page_token":"t0"}]}"#
        );
    }

    #[test]
    fn test_osv_pagination_b() {
        // only the second query paginates; an ID repeated across pages is reported once
        let client = SeqClient::new(&[
            r#"{"results":[{},{"vulns":[{"id":"B","modified":"x"}],"next_page_token":"t1"}]}"#,
            r#"{"results":[{"vulns":[{"id":"B","modified":"x"},{"id":"E","modified":"x"}]}]}"#,
        ]);
        let packages = vec![
            pkg(Ecosystem::Cargo, "time", "0.1.43"),
            pkg(Ecosystem::Npm, "lodash", "4.17.15"),
        ];
        // first response: query 0 has no token, so it is complete with no vulns
        let results = query_ids(client.clone(), &packages);
        assert_eq!(results, vec![None, ids(&["B", "E"])]);
        assert_eq!(client.bodies().len(), 2);
    }

    #[test]
    fn test_osv_pagination_c() {
        // a token with an empty page is followed
        let client = SeqClient::new(&[
            r#"{"results":[{"next_page_token":"t0"}]}"#,
            r#"{"results":[{"vulns":[{"id":"G","modified":"x"}]}]}"#,
        ]);
        let packages = vec![pkg(Ecosystem::Python, "jinja2", "2.4.1")];
        assert_eq!(query_ids(client, &packages), vec![ids(&["G"])]);
    }

    #[test]
    fn test_osv_pagination_errors_a() {
        let packages = vec![pkg(Ecosystem::Python, "jinja2", "2.4.1")];
        let cache_config = CacheConfig::new(DURATION_0, path_cache(true).unwrap());
        // a server that always returns a token is bounded
        let endless = mock_post(Some(
            r#"{"results":[{"vulns":[{"id":"A","modified":"x"}],"next_page_token":"again"}]}"#,
        ));
        let err = query_osv_batches(endless, &packages, &cache_config, FlagLog(false))
            .unwrap_err()
            .to_string();
        assert!(err.contains("pagination exceeded"), "{err}");
        // a failure on a later page fails the whole query
        let client = SeqClient::new(&[
            r#"{"results":[{"vulns":[{"id":"A","modified":"x"}],"next_page_token":"t0"}]}"#,
        ]);
        assert!(
            query_osv_batches(client, &packages, &cache_config, FlagLog(false)).is_err()
        );
    }

    #[test]
    #[ignore] // hits the live OSV API
    fn test_osv_pagination_live() {
        // These OS-distribution queries are not fetter ecosystems, but they paginate (observed 2026-11-13): Ubuntu returns 1000 vulns plus a token on the first page; Linux/Kernel returns two empty pages with tokens, then a final empty page (no matches).
        let query = |name: &str, ecosystem: &str, version: &str| OSVPackageQuery {
            package: OSVPackage {
                name: name.to_string(),
                ecosystem: ecosystem.to_string(),
            },
            version: version.to_string(),
            page_token: None,
        };
        let queries = vec![
            query("linux", "Ubuntu:20.04:LTS", "5.4.0-26.30"),
            query("Kernel", "Linux", "5.10.0"),
        ];
        let client: Arc<dyn UreqClient> = Arc::new(crate::ureq_client::UreqClientLive);
        let results = query_osv_batch(client, &queries).unwrap();
        let ubuntu = results[0].as_ref().unwrap();
        assert!(ubuntu.len() > 1000, "{}", ubuntu.len());
        let unique: std::collections::HashSet<&String> = ubuntu.iter().collect();
        assert_eq!(unique.len(), ubuntu.len());
        // empty pages with tokens were followed to completion
        assert_eq!(results[1], None);
    }

    #[test]
    fn test_is_osv_queryable_a() {
        assert!(is_osv_queryable(&pkg(Ecosystem::Python, "jinja2", "2.4.1")));
        assert!(!is_osv_queryable(&pkg(Ecosystem::Python, "jinja2", "")));
        assert!(!is_osv_queryable(&pkg(
            Ecosystem::Gha,
            "actions/checkout",
            " "
        )));
    }

    #[test]
    fn test_osv_querybatch_cache_disabled() {
        let mut mock_post_map = HashMap::new();
        mock_post_map.insert("https://api.osv.dev".to_string(), "{\"results\":[{\"vulns\":[{\"id\":\"GHSA-test-disabled\",\"modified\":\"2024-05-06T14:46:47.572046Z\"}]}]}".to_string());

        let client = Arc::new(UreqClientMock {
            mock_post: Some(mock_post_map),
            mock_get: None,
        });
        let packages =
            vec![Package::from_name_version_durl("test-package", "1.0.0", None).unwrap()];

        // Test with cache disabled (DURATION_0)
        let cache_dir = path_cache(true).unwrap();
        let cache_config = CacheConfig::new(DURATION_0, cache_dir);
        let results =
            query_osv_batches(client, &packages, &cache_config, FlagLog(false)).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], Some(vec!["GHSA-test-disabled".to_string()]));
    }
}
