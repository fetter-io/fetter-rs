//! CycloneDX 1.6 JSON export of a scan (https://cyclonedx.org/docs/1.6/json/).
//!
//! Each distinct package becomes one `library` component. Packages are grouped by purl, which serves as the `bom-ref`; a package with no purl gets a `fetter:` reference instead. Each site holding the package is an `evidence.occurrences` entry (its path as the `location`, its kind in `additionalContext`). Properties in the `fetter:` namespace record the ecosystem and a VCS commit if known; a VCS URL is an external reference. The `serialNumber` is derived from the components and the export time, so it is unique per export.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::origin_site::Site;
use crate::package::Package;
use crate::purl::package_purl;

pub const SPEC_VERSION: &str = "1.6";

/// One component under construction: the fields that do not depend on sites, plus the sites and commits gathered across packages that share the reference.
struct Entry {
    base: serde_json::Map<String, Value>,
    ecosystem: &'static str,
    sites: BTreeSet<(String, &'static str)>,
    commits: BTreeSet<String>,
    vcs: BTreeSet<String>,
}

/// The components of a scan, sorted by `bom-ref`, and the number without a purl.
pub fn components(package_to_sites: &HashMap<Package, Vec<Site>>) -> (Vec<Value>, usize) {
    let mut entries: BTreeMap<String, Entry> = BTreeMap::new();
    let mut without = 0;
    for (p, sites) in package_to_sites {
        let version = p.version.to_string();
        let purl = package_purl(p);
        let (bom_ref, name, group, purl_s) = match &purl {
            Some(u) => (
                u.to_string(),
                u.name.clone(),
                u.namespace.clone(),
                Some(u.to_string()),
            ),
            None => {
                without += 1;
                (
                    format!("fetter:{}/{}@{}", p.ecosystem.cli_name(), p.name, version),
                    p.name.clone(),
                    None,
                    None,
                )
            }
        };
        let e = entries.entry(bom_ref.clone()).or_insert_with(|| {
            let mut base = serde_json::Map::new();
            base.insert("type".into(), json!("library"));
            base.insert("bom-ref".into(), json!(bom_ref));
            if let Some(g) = group {
                base.insert("group".into(), json!(g));
            }
            base.insert("name".into(), json!(name));
            if !version.is_empty() {
                base.insert("version".into(), json!(version));
            }
            if let Some(s) = purl_s {
                base.insert("purl".into(), json!(s));
            }
            Entry {
                base,
                ecosystem: p.ecosystem.cli_name(),
                sites: BTreeSet::new(),
                commits: BTreeSet::new(),
                vcs: BTreeSet::new(),
            }
        });
        for s in sites {
            e.sites
                .insert((s.path.as_path().display().to_string(), s.kind.as_str()));
        }
        if let Some(du) = &p.direct_url {
            if let Some(vi) = &du.vcs_info {
                e.commits.insert(vi.commit_id.clone());
                e.vcs.insert(du.url.clone());
            }
        }
    }
    let out = entries
        .into_values()
        .map(|e| {
            let mut c = e.base;
            let mut props =
                vec![json!({"name": "fetter:ecosystem", "value": e.ecosystem})];
            c.insert(
                "evidence".into(),
                json!({"occurrences": e.sites.iter().map(|(path, kind)| json!({
                    "location": path,
                    "additionalContext": format!("fetter site kind: {kind}"),
                })).collect::<Vec<_>>()}),
            );
            for commit in &e.commits {
                props.push(json!({"name": "fetter:commit", "value": commit}));
            }
            if !e.vcs.is_empty() {
                c.insert(
                    "externalReferences".into(),
                    Value::Array(
                        e.vcs
                            .iter()
                            .map(|u| json!({"type": "vcs", "url": u}))
                            .collect(),
                    ),
                );
            }
            c.insert("properties".into(), Value::Array(props));
            Value::Object(c)
        })
        .collect();
    (out, without)
}

/// An RFC 4122 UUID (version 5 layout) from the SHA-256 of `data`.
fn uuid_from(data: &[u8]) -> String {
    let h = Sha256::digest(data);
    let mut b = [0u8; 16];
    b.copy_from_slice(&h[..16]);
    b[6] = (b[6] & 0x0f) | 0x50;
    b[8] = (b[8] & 0x3f) | 0x80;
    let x: String = b.iter().map(|v| format!("{v:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &x[0..8],
        &x[8..12],
        &x[12..16],
        &x[16..20],
        &x[20..32]
    )
}

/// Seconds since the Unix epoch as an RFC 3339 UTC timestamp.
fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// A CycloneDX 1.6 BOM of a scan, and the number of packages without a purl. `now` is the time since the Unix epoch (the current time if `None`); it sets the timestamp and, with the components, the serial number, so each export gets a unique one as the specification recommends.
pub fn to_bom(
    package_to_sites: &HashMap<Package, Vec<Site>>,
    now: Option<Duration>,
) -> (Value, usize) {
    let (comps, without) = components(package_to_sites);
    let now = now.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
    });
    let mut seed = serde_json::to_string(&comps).unwrap_or_default();
    seed.push_str(&now.as_nanos().to_string());
    let serial = uuid_from(seed.as_bytes());
    let secs = now.as_secs();
    let bom = json!({
        "bomFormat": "CycloneDX",
        "specVersion": SPEC_VERSION,
        "serialNumber": format!("urn:uuid:{serial}"),
        "version": 1,
        "metadata": {
            "timestamp": rfc3339(secs),
            "tools": {
                "components": [{
                    "type": "application",
                    "name": "fetter",
                    "version": env!("CARGO_PKG_VERSION"),
                }]
            }
        },
        "components": comps,
    });
    (bom, without)
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecosystem::Ecosystem;
    use crate::origin_site::SiteKind;
    use crate::package_durl::{DirectURL, VcsInfo};
    use crate::path_shared::PathShared;
    use std::path::PathBuf;

    fn site(p: &str, kind: SiteKind, eco: Ecosystem) -> Site {
        Site::new(eco, kind, PathShared::from_path_buf(PathBuf::from(p)))
    }

    fn sample() -> HashMap<Package, Vec<Site>> {
        let mut m = HashMap::new();
        let sp = site("/a/site-packages", SiteKind::Installed, Ecosystem::Python);
        let sp2 = site("/b/site-packages", SiteKind::Installed, Ecosystem::Python);
        m.insert(
            Package::from_name_version_durl("requests", "2.31.0", None).unwrap(),
            vec![sp.clone(), sp2],
        );
        let mut dill = Package::from_name_version_durl("dill", "0.3.8", None).unwrap();
        dill.direct_url = Some(DirectURL {
            url: "https://github.com/uqfoundation/dill.git".into(),
            vcs_info: Some(VcsInfo {
                commit_id: "a0a8e86976708d0436eec5c8f7d25329da727cb5".into(),
                vcs: "git".into(),
                revision: None,
            }),
        });
        m.insert(dill, vec![sp]);
        let lock = site("/p/package-lock.json", SiteKind::Lockfile, Ecosystem::Npm);
        m.insert(
            Package::new(
                Ecosystem::Npm,
                "@babel/core",
                crate::version_spec::VersionSpec::new("7.24.0"),
                None,
            ),
            vec![lock],
        );
        m
    }

    #[test]
    fn test_rfc3339_a() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_794_787_199), "2026-11-15T23:59:59Z");
    }

    #[test]
    fn test_uuid_from_a() {
        let u = uuid_from(b"x");
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "5");
        assert!(matches!(&u[19..20], "8" | "9" | "a" | "b"));
        assert_eq!(u, uuid_from(b"x"));
    }

    #[test]
    fn test_to_bom_a() {
        let (bom, without) = to_bom(&sample(), Some(Duration::ZERO));
        assert_eq!(without, 0);
        assert_eq!(bom["bomFormat"], "CycloneDX");
        assert_eq!(bom["specVersion"], "1.6");
        assert_eq!(bom["metadata"]["timestamp"], "1970-01-01T00:00:00Z");
        let comps = bom["components"].as_array().unwrap();
        let refs: Vec<&str> = comps
            .iter()
            .map(|c| c["bom-ref"].as_str().unwrap())
            .collect();
        assert_eq!(
            refs,
            [
                "pkg:npm/%40babel/core@7.24.0",
                "pkg:pypi/dill@0.3.8",
                "pkg:pypi/requests@2.31.0"
            ]
        );
        assert_eq!(comps[0]["group"], "@babel");
        assert_eq!(comps[0]["name"], "core");
        assert_eq!(
            comps[2]["properties"],
            json!([{"name": "fetter:ecosystem", "value": "python"}])
        );
        assert_eq!(
            comps[2]["evidence"]["occurrences"],
            json!([
                {"location": "/a/site-packages", "additionalContext": "fetter site kind: installed"},
                {"location": "/b/site-packages", "additionalContext": "fetter site kind: installed"},
            ])
        );
        assert_eq!(comps[1]["externalReferences"][0]["type"], "vcs");
        assert_eq!(
            comps[1]["properties"][1]["value"],
            "a0a8e86976708d0436eec5c8f7d25329da727cb5"
        );
        // components are deterministic; the serial number is unique per export time
        let (same, _) = to_bom(&sample(), Some(Duration::ZERO));
        assert_eq!(bom, same);
        let (later, _) = to_bom(&sample(), Some(Duration::from_nanos(1)));
        assert_eq!(bom["components"], later["components"]);
        assert_ne!(bom["serialNumber"], later["serialNumber"]);
    }

    #[test]
    fn test_to_bom_round_trip_a() {
        let (bom, _) = to_bom(&sample(), Some(Duration::ZERO));
        let (purls, without) = crate::purl::sbom_purls(&bom).unwrap();
        assert_eq!(without, 0);
        assert_eq!(purls.len(), 3);
    }
}
