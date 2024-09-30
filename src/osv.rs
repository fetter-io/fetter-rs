use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use ureq::Error;
// use std::collections::HashMap;

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

// Function to create the batch query payload
// fn create_query_batch(packages: &[OSVPackageQuery]) -> OSVQueryBatch {
//     OSVQueryBatch {
//         queries: packages.to_vec(),
//     }
// }

// Function to send a single batch of queries to the OSV API
fn query_osv_batch(packages: &[OSVPackageQuery]) -> Vec<Option<String>> {
    let url = "https://api.osv.dev/v1/querybatch";

    let batch_query = OSVQueryBatch {
        queries: packages.to_vec(),
    };
    let body = serde_json::to_string(&batch_query).unwrap();
    println!("{:?}", body);

    let response: Result<ureq::Response, Error> = ureq::post(url)
        .set("Content-Type", "application/json") // Set content type explicitly
        .send_string(&body); // Send the serialized JSON

    match response {
        Ok(body) => {
            let body_str = body.into_string().unwrap_or_default();
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
                            .join(", ")
                    })
                })
                .collect()
        }
        Err(_) => {
            vec![None; packages.len()]
        }
    }
}

fn query_osv(packages: Vec<OSVPackageQuery>) -> Vec<Option<String>> {
    // par_chunks sends groups of 4 to batch query
    let results: Vec<Option<String>> = packages
        .par_chunks(4)
        .flat_map(|chunk| query_osv_batch(chunk))
        .collect();
    results
}

//--------------------------------------------------------------------------

#[test]
fn test_osv_querybatch_a() {
    // Example input data
    let packages = vec![
        OSVPackageQuery {
            package: OSVPackage {
                name: "gradio".to_string(),
                ecosystem: "PyPI".to_string(),
            },
            version: "4.0.0".to_string(),
        },
        OSVPackageQuery {
            package: OSVPackage {
                name: "mesop".to_string(),
                ecosystem: "PyPI".to_string(),
            },
            version: "0.11.1".to_string(),
        },
    ];

    let results: Vec<Option<String>> = query_osv(packages.clone());

    // Print results
    for (result, pkg) in results.iter().zip(packages.iter()) {
        match result {
            Some(vuln_id) => println!("Found vulnerability: {:?} {}", pkg, vuln_id),
            None => println!("No vulnerability: {:?}", pkg),
        }
    }
}

// NOTE: this works
// cat <<EOF | curl -d @- "https://api.osv.dev/v1/querybatch"
// {"queries":[{"package":{"name":"gradio","ecosystem":"PyPI"},"version":"4.0.0"},{"package":{"name":"mesop","ecosystem":"PyPI"},"version":"0.11.1"}]}
// EOF
