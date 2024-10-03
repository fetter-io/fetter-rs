use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use ureq;

// use crate::package::Package;
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

// Function to send a single batch of queries to the OSV API
fn query_osv_batch<U: UreqClient + std::marker::Sync>(
    client: &U,
    packages: &[OSVPackageQuery],
) -> Vec<Option<Vec<String>>> {
    let url = "https://api.osv.dev/v1/querybatch";

    let batch_query = OSVQueryBatch {
        queries: packages.to_vec(),
    };
    let body = serde_json::to_string(&batch_query).unwrap();
    // println!("{:?}", body);

    let response: Result<String, ureq::Error> = client.post(url, &body);
    match response {
        Ok(body_str) => {
            // let body_str = body.into_string().unwrap_or_default();
            // println!("{:?}", body_str);
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

pub(crate) fn query_osv<U: UreqClient + std::marker::Sync>(
    client: &U,
    packages: &Vec<Package>,
) -> Vec<Option<Vec<String>>> {
    let packages_osv: Vec<OSVPackageQuery> = packages
        .iter()
        .map(|p| OSVPackageQuery::from_package(p))
        .collect();

    // par_chunks sends groups of 4 to batch query
    let results: Vec<Option<Vec<String>>> = packages_osv
        .par_chunks(4)
        .flat_map(|chunk| query_osv_batch(client, chunk))
        .collect();
    results
}

//--------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ureq_client::UreqClientMock;
    // use crate::ureq_client::UreqClientLive;

    #[test]
    fn test_osv_querybatch_a() {
        let client = UreqClientMock {
            mock_response : "{\"results\":[{\"vulns\":[{\"id\":\"GHSA-34rf-p3r3-58x2\",\"modified\":\"2024-05-06T14:46:47.572046Z\"},{\"id\":\"GHSA-3f95-mxq2-2f63\",\"modified\":\"2024-04-10T22:19:39.095481Z\"},{\"id\":\"GHSA-48cq-79qq-6f7x\",\"modified\":\"2024-05-21T14:58:25.710902Z\"}]},{\"vulns\":[{\"id\":\"GHSA-pmv9-3xqp-8w42\",\"modified\":\"2024-09-18T19:36:03.377591Z\"}]}]}".to_string(),
        };
        // let client = UreqClientLive;
        let packages = vec![
            Package::from_name_version_durl("gradio", "4.0.0", None).unwrap(),
            Package::from_name_version_durl("mesop", "0.11.1", None).unwrap(),
        ];

        let results = query_osv(&client, &packages);

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
}
