use rayon::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::collections::VecDeque;

// use std::ops::Deref;
// use ureq;

use crate::ureq_client::UreqClient;

//------------------------------------------------------------------------------
#[derive(Debug, Deserialize)]
pub(crate) struct OSVVulnReference {
    url: String,
    r#type: String,
}

impl fmt::Display for OSVVulnReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.r#type, self.url)
    }
}

//------------------------------------------------------------------------------
#[derive(Debug, Deserialize)]
pub(crate) struct OSVReferences(Vec<OSVVulnReference>);

impl OSVReferences {
    /// Return a primary value for this collection.
    pub(crate) fn get_prime(&self) -> String {
        for s in self.0.iter() {
            if s.r#type == "ADVISORY" {
                return s.url.clone();
            }
        }
        return self.0[0].url.clone(); // just get the first
    }
}
// impl Deref for OSVReferences {
//     type Target = Vec<OSVVulnReference>;

//     fn deref(&self) -> &Self::Target {
//         &self.0
//     }
// }
impl fmt::Display for OSVReferences {
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

//------------------------------------------------------------------------------
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

//------------------------------------------------------------------------------
#[derive(Debug, Deserialize)]
pub(crate) struct OSVSeverities(Vec<OSVSeverity>);

impl OSVSeverities {
    pub(crate) fn get_prime(&self) -> String {
        // want to find the highest cvss...
        for s in self.0.iter() {
            println!("{:?}", s.r#type);
        }
        let mut priority: VecDeque<&String> = VecDeque::new();
        for s in self.0.iter() {
            if s.r#type == "CVSS_V4" {
                priority.push_front(&s.score);
            }
            else if s.r#type == "CVSS_V3" {
                priority.push_back(&s.score);
            }
        }
        if let Some(item) = priority.pop_front() {
            item.clone()
        } else {
            self.0[0].score.clone() // get first
        }
    }
}

impl fmt::Display for OSVSeverities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

//------------------------------------------------------------------------------
#[derive(Debug, Deserialize)]
pub(crate) struct OSVVulnInfo {
    pub(crate) summary: String,
    pub(crate) references: OSVReferences,
    pub(crate) severity: Option<OSVSeverities>,
    // details: String,
    // affected: Vec<OSVAffected>, // surprised this is an array of affected
}

//------------------------------------------------------------------------------
pub(crate) fn get_osv_url(vuln_id: &str) -> String {
    format!("https://osv.dev/vulnerability/{}", vuln_id)
}

fn query_osv_vuln<U: UreqClient + std::marker::Sync>(
    client: &U,
    vuln_id: &str,
) -> Option<OSVVulnInfo> {
    let url = format!("https://api.osv.dev/v1/vulns/{}", vuln_id);

    match client.get(&url) {
        Ok(body_str) => {
            let osv_vuln: OSVVulnInfo = serde_json::from_str(&body_str).unwrap();
            Some(osv_vuln)
        }
        Err(_) => None,
    }
}

pub(crate) fn query_osv_vulns<U: UreqClient + std::marker::Sync>(
    client: &U,
    vuln_ids: &Vec<String>,
) -> HashMap<String, OSVVulnInfo> {
    let results: Vec<(String, OSVVulnInfo)> = vuln_ids
        .par_iter()
        .filter_map(|vuln_id| {
            query_osv_vuln(client, vuln_id).map(|info| (vuln_id.clone(), info))
        })
        .collect();
    results.into_iter().collect() // to HashMap
}

//--------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ureq_client::UreqClientMock;

    // #[test]
    // fn test_vuln_live() {
    //     use crate::ureq_client::UreqClientLive;

    //     let vuln_ids = vec![
    //         "GHSA-48cq-79qq-6f7x".to_string(),
    //         "GHSA-pmv9-3xqp-8w42".to_string(),
    //     ];

    //     let result_map = query_osv_vulns(&UreqClientLive, &vuln_ids);

    //     for (vuln_id, vuln) in result_map {
    //         println!("Vuln: {}", vuln_id);
    //         println!("Summary: {:?}", vuln.summary);
    //         println!("References: {}", vuln.references.get_prime());
    //         // println!("Severity: {}", vuln.severity.unwrap().get_prime());
    //         println!();
    //     }
    // }

    #[test]
    fn test_vuln_a() {
        let vuln_ids = vec!["GHSA-48cq-79qq-6f7x".to_string()];

        let content = r#"
        {"id":"GHSA-48cq-79qq-6f7x","summary":"Gradio applications running locally vulnerable to 3rd party websites accessing routes and uploading files","details":" Impact\nThis CVE covers the ability of 3rd party websites to access routes and upload files to users running Gradio applications locally.  For example, the malicious owners of [www.dontvisitme.com](http://www.dontvisitme.com/) could put a script on their website that uploads a large file to http://localhost:7860/upload and anyone who visits their website and has a Gradio app will now have that large file uploaded on their computer\n\n### Patches\nYes, the problem has been patched in Gradio version 4.19.2 or higher. We have no knowledge of this exploit being used against users of Gradio applications, but we encourage all users to upgrade to Gradio 4.19.2 or higher.\n\nFixed in: https://github.com/gradio-app/gradio/commit/84802ee6a4806c25287344dce581f9548a99834a\nCVE: https://nvd.nist.gov/vuln/detail/CVE-2024-1727","aliases":["CVE-2024-1727"],"modified":"2024-05-21T15:12:35.101662Z","published":"2024-05-21T14:43:50Z","database_specific":{"github_reviewed_at":"2024-05-21T14:43:50Z","github_reviewed":true,"severity":"MODERATE","cwe_ids":["CWE-352"],"nvd_published_at":null},"references":[{"type":"WEB","url":"https://github.com/gradio-app/gradio/security/advisories/GHSA-48cq-79qq-6f7x"},{"type":"ADVISORY","url":"https://nvd.nist.gov/vuln/detail/CVE-2024-1727"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/pull/7503"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/commit/84802ee6a4806c25287344dce581f9548a99834a"},{"type":"PACKAGE","url":"https://github.com/gradio-app/gradio"},{"type":"WEB","url":"https://huntr.com/bounties/a94d55fb-0770-4cbe-9b20-97a978a2ffff"}],"affected":[{"package":{"name":"gradio","ecosystem":"PyPI","purl":"pkg:pypi/gradio"},"ranges":[{"type":"ECOSYSTEM","events":[{"introduced":"0"},{"fixed":"4.19.2"}]}],"versions":["4.18.0","4.19.0","4.19.1","4.2.0","4.3.0","4.4.0","4.4.1","4.5.0","4.7.0","4.7.1","4.8.0","4.9.0","4.9.1"],"database_specific":{"source":"https://github.com/github/advisory-database/blob/main/advisories/github-reviewed/2024/05/GHSA-48cq-79qq-6f7x/GHSA-48cq-79qq-6f7x.json"}}],"schema_version":"1.6.0","severity":[{"type":"CVSS_V3","score":"CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L"}]}"#;

        let client = UreqClientMock {
            mock_get: Some(content.to_string()),
            mock_post: None,
        };

        let result_map = query_osv_vulns(&client, &vuln_ids);

        let mut rm = result_map.iter();
        let (vuln_id, vuln) = rm.next().unwrap();
        assert_eq!(vuln_id, "GHSA-48cq-79qq-6f7x");
        assert_eq!(vuln.summary, "Gradio applications running locally vulnerable to 3rd party websites accessing routes and uploading files");
        assert_eq!(
            vuln.references.get_prime(),
            "https://nvd.nist.gov/vuln/detail/CVE-2024-1727"
        );
        assert_eq!(
            vuln.severity.as_ref().unwrap().get_prime(),
            "CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L"
        );
    }
}
