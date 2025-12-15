use crate::dep_spec::DepSpec;
use crate::ureq_client::UreqClient;
use crate::util::logger;
use crate::util::path_within_duration;
use crate::util::CacheConfig;
use crate::util::FlagLog;
use crate::version_spec::VersionSpec;

// use rayon::prelude::*;
use serde::Deserialize;
use serde::Serialize;

use std::collections::HashMap;
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

impl PYPIProject {
    /// Return all available VersionSpecs for this project. Optional filters can be used, including filtering on a DepSpec as well as a limit, where the limit is the number of most recent versions found.
    pub fn get_version_specs(
        &self,
        filter: Option<&DepSpec>,
        limit: Option<usize>,
    ) -> Vec<VersionSpec> {
        let mut versions: Vec<VersionSpec> = match filter {
            None => self
                .releases
                .0
                .keys()
                .map(|v| VersionSpec::new(v))
                .collect(),
            Some(dep_spec) => self
                .releases
                .0
                .keys()
                .map(|v| VersionSpec::new(v))
                .filter(|v| dep_spec.validate_version(v))
                .collect(),
        };

        // most recent first
        versions.sort_by(|a, b| b.cmp(a));

        match limit {
            None => versions,
            Some(n) => versions.into_iter().take(n).collect(),
        }
    }
}

//------------------------------------------------------------------------------

fn query_pypi_project(
    client: Arc<dyn UreqClient>,
    project: &str,
    cache_config: &CacheConfig,
    log: FlagLog,
) -> Option<PYPIProject> {
    let cache_fp = cache_config.directory.join(format!("{project}.json"));

    // Try reading from cache if within duration
    if path_within_duration(&cache_fp, cache_config.duration) {
        match std::fs::read_to_string(&cache_fp) {
            Ok(cached_data) => {
                if let Ok(pypi_project) = serde_json::from_str(&cached_data) {
                    logger!(
                        log,
                        module_path!(),
                        "Loaded PyPI {project} from {cache_fp:?}"
                    );
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

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ureq_client::UreqClientLive, ureq_client::UreqClientMock, util::path_cache,
    };
    use std::time::Duration;

    #[test]
    fn test_query_pypi_project_a() {
        let content = r#"{"info":{"author":"Christopher Ariza","name":"conditional-futures","project_urls":{"Homepage":"https://github.com/static-frame/conditional-futures","Issues":"https://github.com/static-frame/conditional-futures/issues","Repository":"https://github.com/static-frame/conditional-futures"}},"releases":{"1.0.0":[{"filename":"conditional_futures-1.0.0-py3-none-any.whl"},{"filename":"conditional_futures-1.0.0.tar.gz"}],"1.0.2":[{"filename":"conditional_futures-1.0.2-py3-none-any.whl"},{"filename":"conditional_futures-1.0.2.tar.gz"}]}}"#;

        let client = Arc::new(UreqClientMock {
            mock_get: Some(content.to_string()),
            mock_post: None,
        });

        let cache_dir = path_cache(true).unwrap();
        let cache_config = CacheConfig::new(Duration::from_secs(0), cache_dir);

        let result = query_pypi_project(
            client,
            "conditional-futures",
            &cache_config,
            FlagLog(false),
        );

        assert!(result.is_some());
        let pypi_project = result.unwrap();

        assert_eq!(pypi_project.info.name, "conditional-futures");
        assert_eq!(
            pypi_project.info.author.as_ref().unwrap(),
            "Christopher Ariza"
        );

        let urls = pypi_project.info.project_urls.as_ref().unwrap();
        assert_eq!(
            urls.get("Homepage").unwrap(),
            "https://github.com/static-frame/conditional-futures"
        );
        assert_eq!(
            urls.get("Repository").unwrap(),
            "https://github.com/static-frame/conditional-futures"
        );
        assert_eq!(
            urls.get("Issues").unwrap(),
            "https://github.com/static-frame/conditional-futures/issues"
        );

        assert_eq!(pypi_project.releases.0.len(), 2); // 1.0.0 and 1.0.2

        let v1_0_0 = pypi_project.releases.0.get("1.0.0").unwrap();
        assert_eq!(v1_0_0.len(), 2);
        assert_eq!(
            v1_0_0[0].filename,
            "conditional_futures-1.0.0-py3-none-any.whl"
        );
        assert_eq!(v1_0_0[1].filename, "conditional_futures-1.0.0.tar.gz");

        let v1_0_2 = pypi_project.releases.0.get("1.0.2").unwrap();
        assert_eq!(v1_0_2.len(), 2);
        assert_eq!(
            v1_0_2[0].filename,
            "conditional_futures-1.0.2-py3-none-any.whl"
        );
        assert_eq!(v1_0_2[1].filename, "conditional_futures-1.0.2.tar.gz");
    }

    #[test]
    fn test_get_version_specs() {
        let content = r#"{"info":{"author":"Christopher Ariza","name":"conditional-futures","project_urls":{"Homepage":"https://github.com/static-frame/conditional-futures","Issues":"https://github.com/static-frame/conditional-futures/issues","Repository":"https://github.com/static-frame/conditional-futures"}},"releases":{"1.0.0":[{"filename":"conditional_futures-1.0.0-py3-none-any.whl"},{"filename":"conditional_futures-1.0.0.tar.gz"}],"1.0.2":[{"filename":"conditional_futures-1.0.2-py3-none-any.whl"},{"filename":"conditional_futures-1.0.2.tar.gz"}]}}"#;

        let client = Arc::new(UreqClientMock {
            mock_get: Some(content.to_string()),
            mock_post: None,
        });

        let cache_dir = path_cache(true).unwrap();
        let cache_config = CacheConfig::new(Duration::from_secs(0), cache_dir);

        let result = query_pypi_project(
            client,
            "conditional-futures",
            &cache_config,
            FlagLog(false),
        );

        assert!(result.is_some());
        let pypi_project = result.unwrap();

        // Get version specs without filter or limit (returns sorted descending)
        let version_specs = pypi_project.get_version_specs(None, None);

        assert_eq!(version_specs.len(), 2);
        // Should be sorted descending (most recent first)
        assert_eq!(version_specs[0].to_string(), "1.0.2");
        assert_eq!(version_specs[1].to_string(), "1.0.0");
    }

    #[test]
    fn test_get_version_specs_with_filter() {
        let content = r#"{"info":{"author":"Christopher Ariza","name":"conditional-futures","project_urls":{"Homepage":"https://github.com/static-frame/conditional-futures","Issues":"https://github.com/static-frame/conditional-futures/issues","Repository":"https://github.com/static-frame/conditional-futures"}},"releases":{"1.0.0":[{"filename":"conditional_futures-1.0.0-py3-none-any.whl"},{"filename":"conditional_futures-1.0.0.tar.gz"}],"1.0.2":[{"filename":"conditional_futures-1.0.2-py3-none-any.whl"},{"filename":"conditional_futures-1.0.2.tar.gz"}]}}"#;

        let client = Arc::new(UreqClientMock {
            mock_get: Some(content.to_string()),
            mock_post: None,
        });

        let cache_dir = path_cache(true).unwrap();
        let cache_config = CacheConfig::new(Duration::from_secs(0), cache_dir);

        let result = query_pypi_project(
            client,
            "conditional-futures",
            &cache_config,
            FlagLog(false),
        );

        assert!(result.is_some());
        let pypi_project = result.unwrap();

        // Test filter: >=1.0.1 (should only match 1.0.2)
        let filter_ge = DepSpec::from_string("conditional-futures>=1.0.1").unwrap();
        let filtered_specs = pypi_project.get_version_specs(Some(&filter_ge), None);

        assert_eq!(filtered_specs.len(), 1);
        assert_eq!(filtered_specs[0].to_string(), "1.0.2");

        // Test filter: <1.0.2 (should only match 1.0.0)
        let filter_lt = DepSpec::from_string("conditional-futures<1.0.2").unwrap();
        let filtered_specs = pypi_project.get_version_specs(Some(&filter_lt), None);

        assert_eq!(filtered_specs.len(), 1);
        assert_eq!(filtered_specs[0].to_string(), "1.0.0");

        // Test filter: ==1.0.0 (should only match 1.0.0)
        let filter_eq = DepSpec::from_string("conditional-futures==1.0.0").unwrap();
        let filtered_specs = pypi_project.get_version_specs(Some(&filter_eq), None);

        assert_eq!(filtered_specs.len(), 1);
        assert_eq!(filtered_specs[0].to_string(), "1.0.0");

        // Test filter: >=1.0.0 (should match both, sorted descending)
        let filter_all = DepSpec::from_string("conditional-futures>=1.0.0").unwrap();
        let filtered_specs = pypi_project.get_version_specs(Some(&filter_all), None);

        assert_eq!(filtered_specs.len(), 2);
        assert_eq!(filtered_specs[0].to_string(), "1.0.2");
        assert_eq!(filtered_specs[1].to_string(), "1.0.0");

        // Test filter: >2.0.0 (should match none)
        let filter_none = DepSpec::from_string("conditional-futures>2.0.0").unwrap();
        let filtered_specs = pypi_project.get_version_specs(Some(&filter_none), None);

        assert_eq!(filtered_specs.len(), 0);
    }

    #[test]
    fn test_get_version_specs_with_limit() {
        // Extended test data with more versions: 0.9.0, 1.0.0, 1.0.1, 1.0.2, 1.1.0, 1.2.0, 2.0.0, 2.1.0
        let content = r#"{"info":{"author":"Christopher Ariza","name":"conditional-futures","project_urls":{"Homepage":"https://github.com/static-frame/conditional-futures","Issues":"https://github.com/static-frame/conditional-futures/issues","Repository":"https://github.com/static-frame/conditional-futures"}},"releases":{"0.9.0":[{"filename":"conditional_futures-0.9.0-py3-none-any.whl"}],"1.0.0":[{"filename":"conditional_futures-1.0.0-py3-none-any.whl"}],"1.0.1":[{"filename":"conditional_futures-1.0.1-py3-none-any.whl"}],"1.0.2":[{"filename":"conditional_futures-1.0.2-py3-none-any.whl"}],"1.1.0":[{"filename":"conditional_futures-1.1.0-py3-none-any.whl"}],"1.2.0":[{"filename":"conditional_futures-1.2.0-py3-none-any.whl"}],"2.0.0":[{"filename":"conditional_futures-2.0.0-py3-none-any.whl"}],"2.1.0":[{"filename":"conditional_futures-2.1.0-py3-none-any.whl"}]}}"#;

        let client = Arc::new(UreqClientMock {
            mock_get: Some(content.to_string()),
            mock_post: None,
        });

        let cache_dir = path_cache(true).unwrap();
        let cache_config = CacheConfig::new(Duration::from_secs(0), cache_dir);

        let result = query_pypi_project(
            client,
            "conditional-futures",
            &cache_config,
            FlagLog(false),
        );

        assert!(result.is_some());
        let pypi_project = result.unwrap();

        // Verify total count: should have 8 versions
        let all_specs = pypi_project.get_version_specs(None, None);
        assert_eq!(all_specs.len(), 8);
        // Verify sorted descending (most recent first)
        assert_eq!(all_specs[0].to_string(), "2.1.0");
        assert_eq!(all_specs[7].to_string(), "0.9.0");

        // Test limit: get only 1 most recent version
        let limited_specs = pypi_project.get_version_specs(None, Some(1));
        assert_eq!(limited_specs.len(), 1);
        assert_eq!(limited_specs[0].to_string(), "2.1.0");

        // Test limit: get 3 most recent versions
        let limited_specs = pypi_project.get_version_specs(None, Some(3));
        assert_eq!(limited_specs.len(), 3);
        assert_eq!(limited_specs[0].to_string(), "2.1.0");
        assert_eq!(limited_specs[1].to_string(), "2.0.0");
        assert_eq!(limited_specs[2].to_string(), "1.2.0");

        // Test limit: get 5 most recent versions
        let limited_specs = pypi_project.get_version_specs(None, Some(5));
        assert_eq!(limited_specs.len(), 5);
        assert_eq!(limited_specs[0].to_string(), "2.1.0");
        assert_eq!(limited_specs[4].to_string(), "1.0.2");

        // Test limit: request more than available
        let limited_specs = pypi_project.get_version_specs(None, Some(20));
        assert_eq!(limited_specs.len(), 8);

        // Test filter + limit: >=1.0.0,<2.0.0 with limit 3 (should get 3 most recent 1.x versions)
        let filter = DepSpec::from_string("conditional-futures>=1.0.0,<2.0.0").unwrap();
        let filtered_limited = pypi_project.get_version_specs(Some(&filter), Some(3));
        assert_eq!(filtered_limited.len(), 3);
        assert_eq!(filtered_limited[0].to_string(), "1.2.0");
        assert_eq!(filtered_limited[1].to_string(), "1.1.0");
        assert_eq!(filtered_limited[2].to_string(), "1.0.2");

        // Test filter + limit: >=2.0.0 with limit 10 (should get both 2.x versions)
        let filter = DepSpec::from_string("conditional-futures>=2.0.0").unwrap();
        let filtered_limited = pypi_project.get_version_specs(Some(&filter), Some(10));
        assert_eq!(filtered_limited.len(), 2);
        assert_eq!(filtered_limited[0].to_string(), "2.1.0");
        assert_eq!(filtered_limited[1].to_string(), "2.0.0");

        // Test filter + limit: <1.0.0 with limit 1 (should get 0.9.0)
        let filter = DepSpec::from_string("conditional-futures<1.0.0").unwrap();
        let filtered_limited = pypi_project.get_version_specs(Some(&filter), Some(1));
        assert_eq!(filtered_limited.len(), 1);
        assert_eq!(filtered_limited[0].to_string(), "0.9.0");

        // Test filter + limit: ==1.0.1 with limit 5 (should get only 1.0.1)
        let filter = DepSpec::from_string("conditional-futures==1.0.1").unwrap();
        let filtered_limited = pypi_project.get_version_specs(Some(&filter), Some(5));
        assert_eq!(filtered_limited.len(), 1);
        assert_eq!(filtered_limited[0].to_string(), "1.0.1");

        // Test limit: 0 should return empty
        let limited_specs = pypi_project.get_version_specs(None, Some(0));
        assert_eq!(limited_specs.len(), 0);
    }

    //--------------------------------------------------------------------------
    #[test]
    #[ignore] // This is a temporary test that hits the live PyPI endpoint
    fn test_query_pypi_project_live() {
        // Test with a well-known package
        let project = "conditional-futures";
        let client = Arc::new(UreqClientLive);
        let cache_dir = path_cache(true).unwrap();
        let cache_config = CacheConfig::new(Duration::from_secs(0), cache_dir); // 0 duration = always fetch

        let result = query_pypi_project(client, project, &cache_config, FlagLog(true));

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
        assert!(
            !pypi_project.releases.0.is_empty(),
            "Expected at least one release"
        );
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
