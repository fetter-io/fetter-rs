use crate::ureq_client::UreqClient;
use crate::util::logger;
use crate::util::FlagCacheRefresh;
use crate::util::FlagLog;

use rayon::prelude::*;
use serde::Deserialize;
use serde::Serialize;

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

//------------------------------------------------------------------------------
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PYPIProjectURLs {
    documentation: String,
    homepage: String,
    repository: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PYPIInfo {
    author: String, // might be a collection
    name: String,
    project_urls: PYPIProjectURLs,
}

//------------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PYPIRelease {
    filename: String,
    // url: String,
    // much more here
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PYPIReleases(HashMap<String, PYPIRelease>);

//------------------------------------------------------------------------------
/// This is a query object designed to match the response from the API.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PYPIProject {
    pub info: String,
    pub releases: PYPIReleases,
    // pub urls: Vec<PYPIRelease>,
    // vulnerabilities: String, // might be useful
}

//------------------------------------------------------------------------------

fn query_pypi_project(
    client: Arc<dyn UreqClient>,
    project: &str,
    cache_refresh: FlagCacheRefresh,
    cache_dir: &Path,
    log: FlagLog,
) -> Option<PYPIProject> {
    let cache_fp = cache_dir.join(format!("{project}.json"));

    // Try reading from cache
    if !bool::from(cache_refresh) && cache_fp.exists() {
        match std::fs::read_to_string(&cache_fp) {
            Ok(cached_data) => {
                if let Ok(osv_vuln) = serde_json::from_str(&cached_data) {
                    logger!(log, module_path!(), "Loaded PyPI {project} from cache");
                    return Some(osv_vuln);
                } else {
                    logger!(
                        log,
                        module_path!(),
                        "Failed to deserialize cached {project}, refetching"
                    );
                }
            }
            Err(e) => {
                logger!(
                    log,
                    module_path!(),
                    "Failed to read cache file {cache_fp:?}: {e}, refetching",
                );
            }
        }
    }

    // Fetch from API
    match client.get(&format!("https://pypi.org/pypi/{project}/json")) {
        Ok(body_str) => match serde_json::from_str(&body_str) {
            Ok(osv_vuln) => {
                if let Err(e) = std::fs::write(&cache_fp, &body_str) {
                    logger!(
                        log,
                        module_path!(),
                        "Failed to write cache file {cache_fp:?}: {e}"
                    );
                } else {
                    logger!(
                        log,
                        module_path!(),
                        "Cached OSV vuln response for {vuln_id}"
                    );
                }
                Some(osv_vuln)
            }
            Err(e) => {
                logger!(
                    log,
                    module_path!(),
                    "Failed to deserialize OSV vuln {vuln_id}: {e}"
                );
                None
            }
        },
        Err(e) => {
            logger!(
                log,
                module_path!(),
                "HTTP request failed for {vuln_id}: {e}"
            );
            None
        }
    }
}
