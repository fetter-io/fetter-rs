use rayon::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use ureq::Agent;



#[derive(Debug, Deserialize)]
struct OSVVulnReference {
    url: String,
    // type
}

#[derive(Debug, Deserialize)]
struct OSVEcoSpecific {
    severity: String,
}


#[derive(Debug, Deserialize)]
struct OSVAffected {
    ecosystem_specific: OSVEcoSpecific,
    // package
    // ranges
    // database_specific
}

#[derive(Debug, Deserialize)]
struct OSVVulnInfo {
    summary: String,
    details: String,
    references: Vec<OSVVulnReference>,
    affected: Vec<OSVAffected>, // surprised this is an array of affected
}


// fn query_osv_vuln(agent: &Agent, vuln_id: &str) -> Option<OSVVulnInfo> {
//     let url = "https://api.osv.dev/v1/vulns";

//     let response = agent.post(url).send_json(ureq::json!({
//         "vuln_ids": [vuln_id]
//     }));

//     match response {
//         Ok(res) => {
//             if res.ok() {
//                 let response_data: HashMap<String, Vec<OSVVulnInfo>> =
//                     res.into_json().ok()?;
//                 // OSV returns a map with "vulns" key
//                 response_data
//                     .get("vulns")
//                     .and_then(|vulns| vulns.get(0).cloned())
//             } else {
//                 None
//             }
//         }
//         Err(_) => None,
//     }
// }

fn query_osv_vuln<U: UreqClient + std::marker::Sync>(
    client: &U,
    vuln_id: &str,
) -> Vec<Option<Vec<String>>> {
    let url = "https://api.osv.dev/v1/vulns";

    let vuln_query = OSVQueryBatch {
        queries: packages.to_vec(),
    };
    let body = serde_json::to_string(&vuln_query).unwrap();
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


fn query_osv_vulns(vuln_ids: Vec<String>) -> HashMap<String, Option<OSVVulnInfo>> {
    let agent = ureq::agent();
    let results = Mutex::new(HashMap::new());

    vuln_ids.par_iter().for_each(|vuln_id| {
        let info = query_osv_vuln(&agent, vuln_id);
        let mut results = results.lock().unwrap();
        results.insert(vuln_id.clone(), info);
    });

    results.into_inner().unwrap()
}

fn main() {
    let vuln_ids = vec![
        "CVE-2021-44228".to_string(),
        "CVE-2022-22965".to_string(),
        // add more ids here
    ];

    let result_map = query_osv_vulns(vuln_ids);

    for (vuln_id, info) in result_map {
        println!("Vuln ID: {}", vuln_id);
        match info {
            Some(vuln) => {
                println!("Summary: {:?}", vuln.summary);
                println!("Details: {:?}", vuln.details);
                println!("References: {:?}", vuln.references);
                println!("Severity: {:?}", vuln.severity);
            }
            None => {
                println!("No data found for this vulnerability.");
            }
        }
        println!();
    }
}
