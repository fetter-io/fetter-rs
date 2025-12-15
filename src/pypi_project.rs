use crate::ureq_client::UreqClient;
use crate::util::logger;
use crate::util::FlagCacheRefresh;
use crate::util::FlagLog;

// use rayon::prelude::*;
use serde::Deserialize;
use serde::Serialize;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

//------------------------------------------------------------------------------
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PYPIInfo {
    pub author: Option<String>, // might be a collection or null
    pub name: String,
    pub project_urls: Option<HashMap<String, String>>,
}

//------------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PYPIRelease {
    pub filename: String,
    // pub url: String,
    // much more here
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PYPIReleases(pub HashMap<String, Vec<PYPIRelease>>);

//------------------------------------------------------------------------------
/// This is a query object designed to match the response from the API.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PYPIProject {
    pub info: PYPIInfo,
    pub releases: PYPIReleases,
    // pub urls: Vec<PYPIRelease>,
    // pub vulnerabilities: Option<Vec<String>>, // might be useful
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
                if let Ok(pypi_project) = serde_json::from_str(&cached_data) {
                    logger!(log, module_path!(), "Loaded PyPI {project} from cache");
                    return Some(pypi_project);
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
            Ok(pypi_project) => {
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
                        "Cached PYPI project response for {project}"
                    );
                }
                Some(pypi_project)
            }
            Err(e) => {
                logger!(
                    log,
                    module_path!(),
                    "Failed to deserialize PYPI project {project}: {e}"
                );
                None
            }
        },
        Err(e) => {
            logger!(
                log,
                module_path!(),
                "HTTP request failed for {project}: {e}"
            );
            None
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ureq_client::UreqClientLive, util::path_cache};

    #[test]
    #[ignore] // This is a temporary test that hits the live PyPI endpoint
    fn test_query_pypi_project_live() {
        // Test with a well-known package
        let project = "conditional-futures";
        let client = Arc::new(UreqClientLive);
        let cache_dir = path_cache(true).unwrap();

        let result = query_pypi_project(
            client,
            project,
            FlagCacheRefresh(true), // Force fresh fetch
            &cache_dir,
            FlagLog(true),
        );

        assert!(result.is_some(), "Failed to fetch {project} from PyPI");

        let pypi_project = result.unwrap();

        // Check info fields
        assert_eq!(pypi_project.info.name, project);
        println!("Project name: {}", pypi_project.info.name);

        if let Some(author) = &pypi_project.info.author {
            println!("Author: {}", author);
        }

        if let Some(urls) = &pypi_project.info.project_urls {
            println!("Project URLs:");
            for (key, value) in urls.iter() {
                println!("  {}: {}", key, value);
            }
        }

        // Check releases - numpy should have many versions
        assert!(!pypi_project.releases.0.is_empty(), "Expected at least one release");
        println!("Number of releases: {}", pypi_project.releases.0.len());

        // Check a recent version has multiple files (wheels, source dist, etc.)
        if let Some((version, files)) = pypi_project.releases.0.iter().next() {
            println!("Sample version {}: {} files", version, files.len());
            if !files.is_empty() {
                println!("  First file: {}", files[0].filename);
            }
        }
    }
}