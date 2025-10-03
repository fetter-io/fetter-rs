use crate::util::DURATION_0;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

// use crate::package::Package;
use crate::util::{hash_string, logger, path_within_duration, FlagLog, ResultDynError};
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
}

impl OSVPackageQuery {
    fn from_package(package: &Package) -> Self {
        OSVPackageQuery {
            package: OSVPackage {
                name: package.name.clone(),
                ecosystem: "PyPI".to_string(),
            },
            version: package.version.to_string(),
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
}

/// OSV response component
#[derive(Serialize, Deserialize, Debug, Clone)]
struct OSVResponse {
    results: Vec<OSVQueryResult>,
}

//------------------------------------------------------------------------------

const OSV_BATCH_URL: &str = "https://api.osv.dev/v1/querybatch";

/// Function to send a single batch of queries to the OSV API, and return a Vec of vulnerabilities per package.
fn query_osv_batch(
    client: Arc<dyn UreqClient>,
    packages: &[OSVPackageQuery],
) -> Vec<Option<Vec<String>>> {
    let batch_query = OSVQueryBatch {
        queries: packages.to_vec(),
    };
    let body = serde_json::to_string(&batch_query).unwrap();
    let response: Result<String, ureq::Error> = client.post(OSV_BATCH_URL, &body);
    match response {
        Ok(body_str) => {
            let osv_res: OSVResponse = serde_json::from_str(&body_str).unwrap();
            osv_res
                .results
                .iter()
                .map(|result| {
                    result.vulns.as_ref().map(|vuln_list| {
                        vuln_list
                            .iter()
                            .map(|v| v.id.clone())
                            .collect::<Vec<String>>()
                    })
                })
                .collect()
        }
        Err(_) => {
            vec![None; packages.len()]
        }
    }
}

/// Given a slice of Package refs, get all vulnerabilities known for each package.
pub(crate) fn query_osv_batches(
    client: Arc<dyn UreqClient>,
    packages: &[Package],
    cache_dur: Duration,
    cache_dir: &PathBuf,
    log: FlagLog,
) -> ResultDynError<Vec<Option<Vec<String>>>> {
    let packages_osv: Vec<OSVPackageQuery> =
        packages.iter().map(OSVPackageQuery::from_package).collect();

    let query_api = || -> Vec<Option<Vec<String>>> {
        let chunk_size = 64.min(packages_osv.len());
        packages_osv
            .par_chunks(chunk_size)
            .flat_map(|chunk| query_osv_batch(client.clone(), chunk))
            .collect()
    };

    if cache_dur == DURATION_0 {
        // do not read or write cache
        logger!(log, module_path!(), "Cache OSV batch disabled by duration");
        return Ok(query_api());
    }

    let json =
        serde_json::to_string(&packages_osv).expect("Failed to serialize packages_osv");
    let cache_key = hash_string(&json);

    let cache_fp = cache_dir
        .join(format!("osv_batch_{cache_key}"))
        .with_extension("json");

    if path_within_duration(&cache_fp, cache_dur) {
        logger!(
            log,
            module_path!(),
            "Loading OSV batch cache: {:?}",
            cache_fp
        );
        if let Ok(mut file) = File::open(&cache_fp) {
            let mut contents = String::new();
            if file.read_to_string(&mut contents).is_ok() {
                if let Ok(cached_results) = serde_json::from_str(&contents) {
                    return Ok(cached_results);
                }
            }
        }
    }
    // full fetch
    let results = query_api();
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
    use crate::{ureq_client::UreqClientMock, util::path_cache};

    #[test]
    fn test_osv_querybatch_a() {
        let client = Arc::new(UreqClientMock {
            mock_post : Some("{\"results\":[{\"vulns\":[{\"id\":\"GHSA-34rf-p3r3-58x2\",\"modified\":\"2024-05-06T14:46:47.572046Z\"},{\"id\":\"GHSA-3f95-mxq2-2f63\",\"modified\":\"2024-04-10T22:19:39.095481Z\"},{\"id\":\"GHSA-48cq-79qq-6f7x\",\"modified\":\"2024-05-21T14:58:25.710902Z\"}]},{\"vulns\":[{\"id\":\"GHSA-pmv9-3xqp-8w42\",\"modified\":\"2024-09-18T19:36:03.377591Z\"}]}]}".to_string()),
            mock_get : None,
        });
        // let client = UreqClientLive;
        let packages = vec![
            Package::from_name_version_durl("gradio", "4.0.0", None).unwrap(),
            Package::from_name_version_durl("mesop", "0.11.1", None).unwrap(),
        ];

        let cache_dir = path_cache(true).unwrap();
        let results =
            query_osv_batches(client, &packages, DURATION_0, &cache_dir, FlagLog(false))
                .unwrap();

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

    #[test]
    fn test_osv_querybatch_cache_disabled() {
        let client = Arc::new(UreqClientMock {
            mock_post : Some("{\"results\":[{\"vulns\":[{\"id\":\"GHSA-test-disabled\",\"modified\":\"2024-05-06T14:46:47.572046Z\"}]}]}".to_string()),
            mock_get : None,
        });
        let packages =
            vec![Package::from_name_version_durl("test-package", "1.0.0", None).unwrap()];

        // Test with cache disabled (DURATION_0)
        let cache_dir = path_cache(true).unwrap();
        let results =
            query_osv_batches(client, &packages, DURATION_0, &cache_dir, FlagLog(false))
                .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], Some(vec!["GHSA-test-disabled".to_string()]));
    }
}
