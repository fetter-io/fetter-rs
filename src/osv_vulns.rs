use rayon::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
// use std::sync::Mutex;
use std::fmt;
use ureq;

#[derive(Debug, Deserialize)]
struct OSVVulnReference {
    url: String,
    r#type: String,
}

impl fmt::Display for OSVVulnReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.r#type, self.url)
    }
}

#[derive(Debug, Deserialize)]
struct OSVVulnReferences(Vec<OSVVulnReference>);

impl fmt::Display for OSVVulnReferences {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: might only show ADVISORY if defined
        write!(
            f,
            "{}",
            self.0
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

// #[derive(Debug, Deserialize)]
// struct OSVEcoSpecific {
//     severity: String,
// }

// #[derive(Debug, Deserialize)]
// struct OSVAffected {
//     ecosystem_specific: Option<OSVEcoSpecific>,
//     // package
//     // ranges
//     // database_specific
// }

#[derive(Debug, Deserialize)]
struct OSVSeverity {
    r#type: String,
    score: String,
}

impl fmt::Display for OSVSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.r#type, self.score)
    }
}

#[derive(Debug, Deserialize)]
struct OSVSeverities(Vec<OSVSeverity>);

impl fmt::Display for OSVSeverities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // NOTE: might only show the highest CVSS version (CVSS_V3, CVSS_V4)
        write!(
            f,
            "{}",
            self.0
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

#[derive(Debug, Deserialize)]
struct OSVVulnInfo {
    summary: String,
    // details: String,
    references: OSVVulnReferences,
    // affected: Vec<OSVAffected>, // surprised this is an array of affected
    severity: OSVSeverities,
}

fn query_osv_vuln(vuln_id: &str) -> Option<OSVVulnInfo> {
    let url = format!("https://api.osv.dev/v1/vulns/{}", vuln_id);

    match ureq::get(&url).call() {
        Ok(response) => {
            if let Ok(body_str) = response.into_string() {
                // println!("body_str: {:?}", body_str);
                let osv_vuln: OSVVulnInfo = serde_json::from_str(&body_str).unwrap();
                Some(osv_vuln)
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

// fn query_osv_vulns(vuln_ids: Vec<String>) -> HashMap<String, Option<OSVVulnInfo>> {
//     // let results = Mutex::new(HashMap::new());

//     // vuln_ids.par_iter().for_each(|vuln_id| {
//     //     let info = query_osv_vuln(vuln_id);
//     //     let mut results = results.lock().unwrap();
//     //     results.insert(vuln_id.clone(), info);
//     // });

//     // results.into_inner().unwrap()
// }

fn query_osv_vulns(vuln_ids: Vec<String>) -> HashMap<String, OSVVulnInfo> {
    let results: Vec<(String, OSVVulnInfo)> = vuln_ids
        .par_iter()
        .filter_map(|vuln_id| query_osv_vuln(vuln_id).map(|info| (vuln_id.clone(), info)))
        .collect();

    // Convert the Vec to a HashMap after multi-threading
    results.into_iter().collect()
}

//--------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ureq_client::UreqClientMock;
    // use crate::ureq_client::UreqClientLive;

    #[test]
    fn test_vuln_a() {
        let vuln_ids = vec![
            "GHSA-48cq-79qq-6f7x".to_string(),
            "GHSA-pmv9-3xqp-8w42".to_string(),
            // add more ids here
        ];

        let result_map = query_osv_vulns(vuln_ids);

        for (vuln_id, vuln) in result_map {
            println!("Vuln ID: {}", vuln_id);

            println!("Summary: {:?}", vuln.summary);
            // println!("Details: {:?}", vuln.details);
            println!("References: {}", vuln.references);
            // println!("Affected: {:?}", vuln.affected);
            println!("Severity: {}", vuln.severity);

            // match info {
            //     Some(vuln) => {
            //         println!("Summary: {:?}", vuln.summary);
            //         // println!("Details: {:?}", vuln.details);
            //         println!("References: {:?}", vuln.references);
            //         // println!("Affected: {:?}", vuln.affected);
            //         println!("Severity: {:?}", vuln.severity);
            //     }
            //     None => {
            //         println!("No data found for this vulnerability.");
            //     }
            // }
            println!();
        }
    }
}
