use crate::ureq_client::UreqClient;
use crate::util::logger;
use crate::util::FlagCacheRefresh;
use crate::util::FlagLog;
use cvss::Cvss;
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
pub struct OSVVulnReference {
    url: String,
    r#type: String,
}

impl fmt::Display for OSVVulnReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.r#type, self.url)
    }
}

//------------------------------------------------------------------------------
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OSVReferences(Vec<OSVVulnReference>);

impl OSVReferences {
    /// Return a primary value for this collection.
    pub fn get_prime(&self) -> String {
        for s in self.0.iter() {
            if s.r#type == "ADVISORY" {
                return s.url.clone();
            }
        }
        self.0[0].url.clone() // just get the first
    }
}

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

//------------------------------------------------------------------------------
/// If this is a CVSS_V3 or V4, the "score" is the vector, not the score
#[derive(Clone, Debug, Deserialize, Serialize, Ord, Eq, PartialEq, PartialOrd)]
pub struct OSVSeverity {
    r#type: String,
    score: String,
}

impl fmt::Display for OSVSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.r#type, self.score)
    }
}

//------------------------------------------------------------------------------
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OSVSeverities(Vec<OSVSeverity>);

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
/// This is a query object designed to match the response from the API.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OSVVulnInfo {
    pub id: String,
    pub summary: Option<String>,
    pub references: OSVReferences,
    pub severity: Option<OSVSeverities>,
    // details: String,
    // affected: Vec<OSVAffected>,
}

//------------------------------------------------------------------------------

fn query_osv_vuln(
    client: Arc<dyn UreqClient>,
    vuln_id: &str,
    cache_refresh: FlagCacheRefresh,
    cache_dir: &Path,
    log: FlagLog,
) -> Option<OSVVulnInfo> {
    let cache_fp = cache_dir.join(format!("{vuln_id}.json"));

    // Try reading from cache
    if !bool::from(cache_refresh) && cache_fp.exists() {
        match std::fs::read_to_string(&cache_fp) {
            Ok(cached_data) => {
                if let Ok(osv_vuln) = serde_json::from_str(&cached_data) {
                    logger!(log, module_path!(), "Loaded OSV vuln {vuln_id} from cache");
                    return Some(osv_vuln);
                } else {
                    logger!(
                        log,
                        module_path!(),
                        "Failed to deserialize cached {vuln_id}, refetching"
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
    match client.get(&format!("https://api.osv.dev/v1/vulns/{vuln_id}")) {
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

//------------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize)]
pub enum CvssVersion {
    Unknown,
    V3_0,
    V3_1,
    V4_0,
}

impl CvssVersion {
    fn from_vector(s: &str) -> Self {
        if s.starts_with("CVSS:4.0") {
            Self::V4_0
        } else if s.starts_with("CVSS:3.1") {
            Self::V3_1
        } else if s.starts_with("CVSS:3.0") {
            Self::V3_0
        } else {
            Self::Unknown
        }
    }
}

#[derive(Debug, PartialEq, Clone, Deserialize, Serialize)]
pub struct CvssDetail {
    pub version: CvssVersion,
    pub vector: String,
    pub score: f64,
    pub severity: String,
}

impl CvssDetail {
    pub fn from_vector(vector: &str) -> Result<Self, String> {
        let cvss: Cvss = vector
            .parse()
            .map_err(|e| format!("Failed to parse CVSS vector: {e}"))?;

        Ok(Self {
            version: CvssVersion::from_vector(vector),
            vector: vector.to_string(),
            score: cvss.score(),
            severity: cvss.severity().to_string(),
        })
    }
}

impl fmt::Display for CvssDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut chars = self.severity.chars();
        let severity_title = match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        };

        write!(
            f,
            "CVSS {:.1} ({}): {}",
            self.score, severity_title, self.vector
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CvssDetails(Vec<CvssDetail>);

impl fmt::Display for CvssDetails {
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

impl CvssDetails {
    /// For the max version of CVSS, get the max score with full display formatting
    pub fn get_prime(&self) -> String {
        self.0
            .iter()
            .max_by(|a, b| match a.version.cmp(&b.version) {
                Ordering::Equal => {
                    a.score.partial_cmp(&b.score).unwrap_or(Ordering::Equal)
                }
                other => other,
            })
            .map(|d| d.to_string())
            .unwrap_or_default()
    }

    /// Get the maximum CVSS score among all details
    pub fn get_max_score(&self) -> Option<f64> {
        self.0.iter().map(|d| d.score).reduce(f64::max)
    }

    /// Check if any CVSS detail has a score >= threshold
    pub fn has_score_gte(&self, threshold: f64) -> bool {
        self.0.iter().any(|detail| detail.score >= threshold)
    }
}

//--------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VulnInfo {
    pub id: String,
    pub summary: Option<String>,
    pub references: OSVReferences,
    pub cvss_details: Option<CvssDetails>,
}

impl VulnInfo {
    pub fn get_url(&self) -> String {
        format!("https://osv.dev/vulnerability/{}", self.id)
    }
}

// NOTE: Keep only severity entries that look like CVSS (defensive); drop entries that fail to parse.
impl From<OSVVulnInfo> for VulnInfo {
    fn from(src: OSVVulnInfo) -> Self {
        let OSVVulnInfo {
            id,
            summary,
            references,
            severity,
        } = src;
        let cvss_details = severity
            .as_ref()
            .map(|sevs| {
                sevs.0
                    .iter()
                    .filter(|s| s.r#type.to_ascii_uppercase().starts_with("CVSS"))
                    // parse each vector into CvssDetail; drop failures
                    .filter_map(|s| CvssDetail::from_vector(&s.score).ok())
                    .collect::<Vec<_>>()
            })
            // turn empty vecs into None
            .filter(|v| !v.is_empty())
            .map(CvssDetails);

        VulnInfo {
            id,
            summary: summary.map(|s| s.trim().to_string()),
            references,
            cvss_details,
        }
    }
}

//--------------------------------------------------------------------------
pub fn query_osv_vulns(
    client: Arc<dyn UreqClient>,
    vuln_ids: &Vec<String>,
    cache_refresh: FlagCacheRefresh,
    cache_dir: &Path,
    log: FlagLog,
) -> HashMap<String, VulnInfo> {
    vuln_ids
        .par_iter()
        .filter_map(|vuln_id| {
            query_osv_vuln(client.clone(), vuln_id, cache_refresh, cache_dir, log)
                .map(|info| (vuln_id.clone(), VulnInfo::from(info)))
        })
        .collect() // directly collect to HashMap
}

//--------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ureq_client::UreqClientMock, util::path_cache};
    use cvss::Cvss;

    #[test]
    fn test_get_prime_prefers_highest_version_and_score() {
        // v3.1 example, score ~4.3 (Medium)
        let d1 = CvssDetail::from_vector("CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L")
            .unwrap();

        // v4.0 example, lower score (~1.7 Low)
        let d2 = CvssDetail::from_vector(
            "CVSS:4.0/AV:N/AC:L/AT:P/PR:N/UI:N/VC:N/VI:L/VA:N/SC:N/SI:N/SA:N",
        )
        .unwrap();

        // v4.0 example, higher score (should be chosen as prime)
        let d3 = CvssDetail::from_vector(
            "CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:H/VI:H/VA:H/SC:H/SI:H/SA:H",
        )
        .unwrap();

        let details = CvssDetails(vec![d1, d2, d3]);

        // Ensure string formatting looks right and it picked the higher-scoring v4.0 vector
        let prime_str = details.get_prime();
        assert!(
            prime_str.contains("CVSS:4.0"),
            "Expected CVSS:4.0 in prime string: {}",
            prime_str
        );
        assert!(
            prime_str.contains("VC:H/VI:H/VA:H"),
            "Expected high impact scores in prime string: {}",
            prime_str
        );
        assert!(
            prime_str.starts_with("CVSS"),
            "Expected formatted display starting with 'CVSS': {}",
            prime_str
        );
    }

    #[test]
    fn test_cvss_score_a() {
        let s1 = "CVSS:4.0/AV:N/AC:L/AT:P/PR:N/UI:N/VC:N/VI:L/VA:N/SC:N/SI:N/SA:N/E:U";
        let v1: Cvss = s1.parse().unwrap(); // calls FromStr automatically

        assert_eq!(v1.score(), 1.7);
        assert_eq!(v1.severity().to_string(), "low");

        let s2 = "CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L";
        let v2: Cvss = s2.parse().unwrap(); // calls FromStr automatically
        assert_eq!(v2.score(), 4.3);
        assert_eq!(v2.severity().to_string(), "medium");
    }

    #[test]
    fn test_vuln_a() {
        let vuln_ids = vec!["GHSA-48cq-79qq-6f7x".to_string()];

        let content = r#"
        {"id":"GHSA-48cq-79qq-6f7x","summary":"Gradio applications running locally vulnerable to 3rd party websites accessing routes and uploading files","details":" Impact\nThis CVE covers the ability of 3rd party websites to access routes and upload files to users running Gradio applications locally.  For example, the malicious owners of [www.dontvisitme.com](http://www.dontvisitme.com/) could put a script on their website that uploads a large file to http://localhost:7860/upload and anyone who visits their website and has a Gradio app will now have that large file uploaded on their computer\n\n### Patches\nYes, the problem has been patched in Gradio version 4.19.2 or higher. We have no knowledge of this exploit being used against users of Gradio applications, but we encourage all users to upgrade to Gradio 4.19.2 or higher.\n\nFixed in: https://github.com/gradio-app/gradio/commit/84802ee6a4806c25287344dce581f9548a99834a\nCVE: https://nvd.nist.gov/vuln/detail/CVE-2024-1727","aliases":["CVE-2024-1727"],"modified":"2024-05-21T15:12:35.101662Z","published":"2024-05-21T14:43:50Z","database_specific":{"github_reviewed_at":"2024-05-21T14:43:50Z","github_reviewed":true,"severity":"MODERATE","cwe_ids":["CWE-352"],"nvd_published_at":null},"references":[{"type":"WEB","url":"https://github.com/gradio-app/gradio/security/advisories/GHSA-48cq-79qq-6f7x"},{"type":"ADVISORY","url":"https://nvd.nist.gov/vuln/detail/CVE-2024-1727"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/pull/7503"},{"type":"WEB","url":"https://github.com/gradio-app/gradio/commit/84802ee6a4806c25287344dce581f9548a99834a"},{"type":"PACKAGE","url":"https://github.com/gradio-app/gradio"},{"type":"WEB","url":"https://huntr.com/bounties/a94d55fb-0770-4cbe-9b20-97a978a2ffff"}],"affected":[{"package":{"name":"gradio","ecosystem":"PyPI","purl":"pkg:pypi/gradio"},"ranges":[{"type":"ECOSYSTEM","events":[{"introduced":"0"},{"fixed":"4.19.2"}]}],"versions":["4.18.0","4.19.0","4.19.1","4.2.0","4.3.0","4.4.0","4.4.1","4.5.0","4.7.0","4.7.1","4.8.0","4.9.0","4.9.1"],"database_specific":{"source":"https://github.com/github/advisory-database/blob/main/advisories/github-reviewed/2024/05/GHSA-48cq-79qq-6f7x/GHSA-48cq-79qq-6f7x.json"}}],"schema_version":"1.6.0","severity":[{"type":"CVSS_V3","score":"CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L"}]}"#;

        let mut mock_get_map = HashMap::new();
        mock_get_map.insert("https://api.osv.dev".to_string(), content.to_string());

        let client = Arc::new(UreqClientMock {
            mock_get: Some(mock_get_map),
            mock_post: None,
        });

        let cache_dir = path_cache(true).unwrap();
        let result_map = query_osv_vulns(
            client,
            &vuln_ids,
            FlagCacheRefresh(true),
            &cache_dir,
            FlagLog(false),
        );

        let mut rm = result_map.iter();
        let (vuln_id, vuln) = rm.next().unwrap();
        assert_eq!(vuln_id, "GHSA-48cq-79qq-6f7x");
        assert_eq!(vuln.summary.as_ref().unwrap(), "Gradio applications running locally vulnerable to 3rd party websites accessing routes and uploading files");
        assert_eq!(
            vuln.references.get_prime(),
            "https://nvd.nist.gov/vuln/detail/CVE-2024-1727"
        );
        assert_eq!(
            vuln.cvss_details.as_ref().unwrap().get_prime(),
            "CVSS 4.3 (Medium): CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L"
        );
    }
}
