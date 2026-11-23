//! The serialized form of a scan, and the `monitor-scan` payload (for fetter.io and the scan cache).
//!
//! Schema 1 is a JSON object with named fields (see `MonitorPayload` and SPEC §5): readers ignore fields they do not know, optional fields may be absent, and origins, sites and packages of an ecosystem or site kind the reader does not know are skipped and counted (`Skipped`), so that a newer fetter's payload is still read by an older one. Paths are stored once, in `paths`, and referred to by index.
//!
//! The format of fetter 3.0 to 4.0.1 (a JSON array `[tenant, system, scan, time]`, the scan itself a positional array of Python executables and site-packages) is read as a fallback (`MonitorPayload::from_json`), with all of its data reinterpreted as the Python ecosystem.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::eco::Hook;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::origin_site::SiteKind;
use crate::package::Package;
use crate::package_durl::DirectURL;
use crate::path_shared::PathShared;
use crate::scan_fs::ScanFS;
use crate::system_tag::SystemTag;
use crate::util::ScanConfig;
use crate::util::SiteSelect;
use crate::version_spec::VersionSpec;

/// The current schema version of the payload and of the scan object.
pub const SCHEMA: u32 = 1;

//------------------------------------------------------------------------------
// wire types

#[derive(Serialize, Deserialize)]
struct OriginWire {
    ecosystem: String,
    path: usize,
}

#[derive(Serialize, Deserialize)]
struct SiteWire {
    ecosystem: String,
    kind: String,
    path: usize,
    /// Present only if hooks were collected for the scan (empty if this site has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hooks: Option<Vec<Hook>>,
}

#[derive(Serialize, Deserialize)]
struct OriginSitesWire {
    origin: usize,
    sites: Vec<usize>,
}

#[derive(Serialize)]
struct PackageSitesOut<'a> {
    package: &'a Package,
    sites: Vec<usize>,
}

/// A package as read: an absent ecosystem is Python (as in fetter 4 and before); the `purl` written with it is ignored.
#[derive(Deserialize)]
struct PackageIn {
    #[serde(default)]
    ecosystem: Option<String>,
    name: String,
    #[serde(default)]
    key: Option<String>,
    version: String,
    #[serde(default)]
    direct_url: Option<DirectURL>,
}

#[derive(Deserialize)]
struct PackageSitesIn {
    package: PackageIn,
    #[serde(default)]
    sites: Vec<usize>,
}

#[derive(Deserialize, Default)]
struct ConfigIn {
    #[serde(default)]
    force_usite: bool,
    #[serde(default)]
    all_users: bool,
    #[serde(default)]
    ecosystems: Vec<String>,
    #[serde(default)]
    roots: Vec<PathBuf>,
    /// Absent before fetter 5.3, whose scans reported all Sites.
    #[serde(default = "sites_all")]
    sites: SiteSelect,
}

fn sites_all() -> SiteSelect {
    SiteSelect::All
}

#[derive(Serialize)]
struct ScanOut<'a> {
    paths: Vec<String>,
    origins: Vec<OriginWire>,
    sites: Vec<SiteWire>,
    origin_sites: Vec<OriginSitesWire>,
    packages: Vec<PackageSitesOut<'a>>,
    config: &'a ScanConfig,
    origins_hash: &'a str,
}

#[derive(Deserialize)]
pub(crate) struct ScanIn {
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    origins: Vec<OriginWire>,
    #[serde(default)]
    sites: Vec<SiteWire>,
    #[serde(default)]
    origin_sites: Vec<OriginSitesWire>,
    #[serde(default)]
    packages: Vec<PackageSitesIn>,
    #[serde(default)]
    config: ConfigIn,
    #[serde(default)]
    origins_hash: String,
}

//------------------------------------------------------------------------------
/// What a reader skipped because it does not know an ecosystem or site kind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Skipped {
    pub origins: usize,
    pub sites: usize,
    pub packages: usize,
    /// The unknown ecosystem and site kind names.
    pub names: BTreeSet<String>,
}

impl Skipped {
    pub fn is_empty(&self) -> bool {
        self.origins == 0
            && self.sites == 0
            && self.packages == 0
            && self.names.is_empty()
    }
}

//------------------------------------------------------------------------------
// writing

/// Assign path indices in first-use order over sorted tables.
struct PathIndexer {
    paths: Vec<String>,
    index: HashMap<PathShared, usize>,
}

impl PathIndexer {
    fn get(&mut self, p: &PathShared) -> Result<usize, String> {
        if let Some(&i) = self.index.get(p) {
            return Ok(i);
        }
        let s = p
            .as_path()
            .to_str()
            .ok_or_else(|| format!("Path is not valid UTF-8: {:?}", p.as_path()))?;
        let i = self.paths.len();
        self.index.insert(p.clone(), i);
        self.paths.push(s.to_string());
        Ok(i)
    }
}

/// Serialize a scan as its schema 1 object; tables are sorted, so the output is deterministic.
pub(crate) fn serialize_scan<S: serde::Serializer>(
    sfs: &ScanFS,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::Error;
    let mut origins: Vec<&Origin> = sfs.origin_to_sites.keys().collect();
    origins.sort();
    let mut sites: Vec<&Site> = sfs
        .origin_to_sites
        .values()
        .chain(sfs.package_to_sites.values())
        .flatten()
        .collect();
    sites.sort();
    sites.dedup();

    let mut pi = PathIndexer {
        paths: Vec::new(),
        index: HashMap::new(),
    };
    let mut origin_index: HashMap<&Origin, usize> = HashMap::new();
    let mut origin_table = Vec::with_capacity(origins.len());
    for (i, o) in origins.iter().enumerate() {
        origin_table.push(OriginWire {
            ecosystem: o.ecosystem.cli_name().to_string(),
            path: pi.get(&o.path).map_err(S::Error::custom)?,
        });
        origin_index.insert(o, i);
    }
    let mut site_index: HashMap<&Site, usize> = HashMap::new();
    let mut site_table = Vec::with_capacity(sites.len());
    for (i, s) in sites.iter().enumerate() {
        site_table.push(SiteWire {
            ecosystem: s.ecosystem.cli_name().to_string(),
            kind: s.kind.as_str().to_string(),
            path: pi.get(&s.path).map_err(S::Error::custom)?,
            hooks: sfs
                .site_to_hooks
                .as_ref()
                .map(|m| m.get(*s).cloned().unwrap_or_default()),
        });
        site_index.insert(s, i);
    }
    // inner Vecs are sorted by construction (`ScanFS::from_parts`)
    let origin_sites = origins
        .iter()
        .map(|o| OriginSitesWire {
            origin: origin_index[o],
            sites: sfs.origin_to_sites[*o]
                .iter()
                .map(|s| site_index[s])
                .collect(),
        })
        .collect();
    let mut packages: Vec<&Package> = sfs.package_to_sites.keys().collect();
    packages.sort();
    let packages = packages
        .into_iter()
        .map(|p| PackageSitesOut {
            package: p,
            sites: sfs.package_to_sites[p]
                .iter()
                .map(|s| site_index[s])
                .collect(),
        })
        .collect();
    ScanOut {
        paths: pi.paths,
        origins: origin_table,
        sites: site_table,
        origin_sites,
        packages,
        config: &sfs.config,
        origins_hash: &sfs.origins_hash,
    }
    .serialize(serializer)
}

//------------------------------------------------------------------------------
// reading

fn package_from(p: PackageIn, eco: Ecosystem) -> Package {
    let key = p.key.unwrap_or_else(|| eco.name_to_key(&p.name));
    Package {
        ecosystem: eco,
        name: p.name,
        key,
        version: VersionSpec::new(&p.version),
        direct_url: p.direct_url,
    }
}

/// Build a scan from its schema 1 object, skipping (and counting) what is of an unknown ecosystem or site kind. Out-of-range indices are errors.
pub(crate) fn scan_from_wire(w: ScanIn) -> Result<(ScanFS, Skipped), String> {
    let mut skipped = Skipped::default();
    let path = |i: usize| -> Result<PathShared, String> {
        w.paths
            .get(i)
            .map(|p| PathShared::from(p.as_str()))
            .ok_or_else(|| format!("path index {i} out of range"))
    };
    let mut origins: Vec<Option<Origin>> = Vec::with_capacity(w.origins.len());
    for o in &w.origins {
        let p = path(o.path)?;
        match Ecosystem::from_cli_name(&o.ecosystem) {
            Some(e) => origins.push(Some(Origin::new(e, p))),
            None => {
                skipped.origins += 1;
                skipped.names.insert(o.ecosystem.clone());
                origins.push(None);
            }
        }
    }
    let mut sites: Vec<Option<Site>> = Vec::with_capacity(w.sites.len());
    let mut site_to_hooks: Option<HashMap<Site, Vec<Hook>>> = None;
    for s in w.sites {
        let p = path(s.path)?;
        let eco = Ecosystem::from_cli_name(&s.ecosystem);
        let kind = SiteKind::from_name(&s.kind);
        let (Some(eco), Some(kind)) = (eco, kind) else {
            skipped.sites += 1;
            if eco.is_none() {
                skipped.names.insert(s.ecosystem.clone());
            }
            if kind.is_none() {
                skipped.names.insert(s.kind.clone());
            }
            sites.push(None);
            continue;
        };
        let site = Site::new(eco, kind, p);
        if let Some(hooks) = s.hooks {
            let m = site_to_hooks.get_or_insert_with(HashMap::new);
            if !hooks.is_empty() {
                m.insert(site.clone(), hooks);
            }
        }
        sites.push(Some(site));
    }
    let known_sites = |idx: &[usize]| -> Result<Vec<Site>, String> {
        let mut out = Vec::new();
        for &i in idx {
            match sites.get(i) {
                Some(Some(s)) => out.push(s.clone()),
                Some(None) => {}
                None => return Err(format!("site index {i} out of range")),
            }
        }
        Ok(out)
    };
    let mut origin_to_sites: HashMap<Origin, Vec<Site>> = origins
        .iter()
        .flatten()
        .map(|o| (o.clone(), Vec::new()))
        .collect();
    for os in &w.origin_sites {
        match origins.get(os.origin) {
            Some(Some(o)) => {
                let s = known_sites(&os.sites)?;
                origin_to_sites.entry(o.clone()).or_default().extend(s);
            }
            Some(None) => {}
            None => return Err(format!("origin index {} out of range", os.origin)),
        }
    }
    let mut package_to_sites: HashMap<Package, Vec<Site>> = HashMap::new();
    for ps in w.packages {
        let eco = match &ps.package.ecosystem {
            None => Some(Ecosystem::Python),
            Some(name) => Ecosystem::from_cli_name(name),
        };
        let s = known_sites(&ps.sites)?;
        let Some(eco) = eco else {
            skipped.packages += 1;
            skipped.names.extend(ps.package.ecosystem.clone());
            continue;
        };
        // a package found only at sites this reader cannot represent is skipped
        if s.is_empty() && !ps.sites.is_empty() {
            skipped.packages += 1;
            continue;
        }
        package_to_sites
            .entry(package_from(ps.package, eco))
            .or_default()
            .extend(s);
    }
    let mut ecosystems: Vec<Ecosystem> = Vec::new();
    for name in &w.config.ecosystems {
        match Ecosystem::from_cli_name(name) {
            Some(e) => ecosystems.push(e),
            None => {
                skipped.names.insert(name.clone());
            }
        }
    }
    ecosystems.sort();
    ecosystems.dedup();
    let config = ScanConfig {
        force_usite: w.config.force_usite,
        all_users: w.config.all_users,
        ecosystems,
        roots: w.config.roots,
        sites: w.config.sites,
    };
    let mut sfs =
        ScanFS::from_parts(origin_to_sites, package_to_sites, config, w.origins_hash);
    sfs.site_to_hooks = site_to_hooks;
    Ok((sfs, skipped))
}

/// The scan of fetter 3.0 to 4.0.1: `(paths, exe to site indices, packages with site indices, site to exe indices, force_usite, all_users, exes_hash)`, packages without an ecosystem.
type LegacyScan = (
    Vec<String>,
    Vec<(usize, Vec<usize>)>,
    Vec<(PackageIn, Vec<usize>)>,
    Vec<(usize, Vec<usize>)>,
    bool,
    bool,
    String,
);

/// Build a scan from the fetter 3.0 to 4.0.1 form, as the Python ecosystem: executables are Python origins, sites are installed Python sites, and packages are Python packages.
fn scan_from_legacy(v: JsonValue) -> Result<ScanFS, String> {
    let (paths, exe_to_sites, packages, _site_to_exes, force_usite, all_users, exes_hash): LegacyScan =
        serde_json::from_value(v).map_err(|e| format!("Invalid legacy scan: {e}"))?;
    let path = |i: usize| -> Result<PathShared, String> {
        paths
            .get(i)
            .map(|p| PathShared::from(p.as_str()))
            .ok_or_else(|| format!("path index {i} out of range"))
    };
    let mut origin_to_sites: HashMap<Origin, Vec<Site>> = HashMap::new();
    for (exe, site_idx) in exe_to_sites {
        let sites = site_idx
            .into_iter()
            .map(|i| path(i).map(Site::python))
            .collect::<Result<Vec<_>, _>>()?;
        origin_to_sites
            .entry(Origin::new(Ecosystem::Python, path(exe)?))
            .or_default()
            .extend(sites);
    }
    let mut package_to_sites: HashMap<Package, Vec<Site>> = HashMap::new();
    for (p, site_idx) in packages {
        let sites = site_idx
            .into_iter()
            .map(|i| path(i).map(Site::python))
            .collect::<Result<Vec<_>, _>>()?;
        package_to_sites
            .entry(package_from(p, Ecosystem::Python))
            .or_default()
            .extend(sites);
    }
    let config = ScanConfig {
        force_usite,
        all_users,
        ecosystems: vec![Ecosystem::Python],
        roots: Vec::new(),
        sites: SiteSelect::All,
    };
    Ok(ScanFS::from_parts(
        origin_to_sites,
        package_to_sites,
        config,
        exes_hash,
    ))
}

//------------------------------------------------------------------------------
/// A `monitor-scan` post: who sent it, from which system, when, and the scan (`None` if unchanged since the previous post).
///
/// Schema 1 (written by `to_json`):
///
/// ```text
/// {"schema": 1, "fetter_version": "5.0.0", "tenant": "...",
///  "system": {"username", "hostname", "os_name", "os_version", "architecture", "logical_cores"},
///  "time": {"secs": u64, "nanos": u32},
///  "scan": null | {"paths": [str], "origins": [{"ecosystem", "path"}],
///                  "sites": [{"ecosystem", "kind", "path", "hooks"?: [{"name", "detail"}]}],
///                  "origin_sites": [{"origin", "sites": [int]}],
///                  "packages": [{"package": {"ecosystem", "name", "key", "version", "direct_url", "purl"?}, "sites": [int]}],
///                  "config": {"force_usite", "all_users", "ecosystems", "roots", "sites"}, "origins_hash": str}}
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorPayload {
    /// The schema version read (`0` for the format of fetter 3.0 to 4.0.1).
    pub schema: u32,
    /// The sending fetter's version, if given.
    pub fetter_version: Option<String>,
    pub tenant: String,
    pub system: SystemTag,
    /// Time since the Unix epoch at which the scan was taken.
    pub time: Duration,
    pub scan: Option<ScanFS>,
    /// What was skipped while reading (unknown ecosystems or site kinds).
    pub skipped: Skipped,
}

/// The schema 1 JSON of a payload.
pub(crate) fn payload_json(
    tenant: &str,
    system: &SystemTag,
    time: &Duration,
    scan: Option<&ScanFS>,
) -> Result<String, String> {
    #[derive(Serialize)]
    struct Out<'a> {
        schema: u32,
        fetter_version: &'a str,
        tenant: &'a str,
        system: &'a SystemTag,
        time: &'a Duration,
        scan: Option<&'a ScanFS>,
    }
    serde_json::to_string(&Out {
        schema: SCHEMA,
        fetter_version: env!("CARGO_PKG_VERSION"),
        tenant,
        system,
        time,
        scan,
    })
    .map_err(|e| format!("Cannot serialize payload: {e}"))
}

impl MonitorPayload {
    /// A payload of the current schema, from this fetter.
    pub fn new(
        tenant: &str,
        system: SystemTag,
        time: Duration,
        scan: Option<ScanFS>,
    ) -> Self {
        MonitorPayload {
            schema: SCHEMA,
            fetter_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            tenant: tenant.to_string(),
            system,
            time,
            scan,
            skipped: Skipped::default(),
        }
    }

    /// The payload as schema 1 JSON.
    pub fn to_json(&self) -> Result<String, String> {
        payload_json(&self.tenant, &self.system, &self.time, self.scan.as_ref())
    }

    /// Read a payload: a JSON object with a `schema` field is read as schema 1 (a newer schema is read the same way, with what is unknown skipped); a JSON array of four is the format of fetter 3.0 to 4.0.1, read as the Python ecosystem (`schema` 0).
    pub fn from_json(json: &str) -> Result<Self, String> {
        let v: JsonValue = serde_json::from_str(json)
            .map_err(|e| format!("Invalid JSON payload: {e}"))?;
        match v {
            JsonValue::Object(mut m) => {
                let schema = m
                    .get("schema")
                    .and_then(JsonValue::as_u64)
                    .ok_or("Payload object has no numeric \"schema\" field")?;
                let schema = u32::try_from(schema).map_err(|_| "schema out of range")?;
                let mut take = |k: &str| m.remove(k).unwrap_or(JsonValue::Null);
                let tenant: String = serde_json::from_value(take("tenant"))
                    .map_err(|e| format!("Invalid tenant: {e}"))?;
                let system: SystemTag = serde_json::from_value(take("system"))
                    .map_err(|e| format!("Invalid system: {e}"))?;
                let time: Duration = serde_json::from_value(take("time"))
                    .map_err(|e| format!("Invalid time: {e}"))?;
                let fetter_version = take("fetter_version").as_str().map(str::to_string);
                let (scan, skipped) = match take("scan") {
                    JsonValue::Null => (None, Skipped::default()),
                    s => {
                        let w: ScanIn = serde_json::from_value(s)
                            .map_err(|e| format!("Invalid scan: {e}"))?;
                        let (sfs, skipped) = scan_from_wire(w)?;
                        (Some(sfs), skipped)
                    }
                };
                Ok(MonitorPayload {
                    schema,
                    fetter_version,
                    tenant,
                    system,
                    time,
                    scan,
                    skipped,
                })
            }
            JsonValue::Array(a) if a.len() == 4 => {
                let mut it = a.into_iter();
                let mut next = || it.next().unwrap_or(JsonValue::Null);
                let tenant: String = serde_json::from_value(next())
                    .map_err(|e| format!("Invalid tenant: {e}"))?;
                let system: SystemTag = serde_json::from_value(next())
                    .map_err(|e| format!("Invalid system: {e}"))?;
                let scan = match next() {
                    JsonValue::Null => None,
                    s => Some(scan_from_legacy(s)?),
                };
                let time: Duration = serde_json::from_value(next())
                    .map_err(|e| format!("Invalid time: {e}"))?;
                Ok(MonitorPayload {
                    schema: 0,
                    fetter_version: None,
                    tenant,
                    system,
                    time,
                    scan,
                    skipped: Skipped::default(),
                })
            }
            _ => Err("Payload is neither a schema object nor a fetter 3-4 array".into()),
        }
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const V4: &str = include_str!("../tests/fixtures/monitor/v4.json");
    const SCHEMA1: &str = include_str!("../tests/fixtures/monitor/schema1.json");

    fn system() -> SystemTag {
        SystemTag {
            username: "user".into(),
            hostname: "host".into(),
            os_name: "linux".into(),
            os_version: "6.8".into(),
            architecture: "x86_64".into(),
            logical_cores: 8,
        }
    }

    /// A Python environment and an npm project; the Python site has a hook.
    fn sample(with_hooks: bool) -> ScanFS {
        let py = Origin::new(
            Ecosystem::Python,
            PathShared::from("/home/user/.venv/bin/python"),
        );
        let sp = Site::python(PathShared::from(
            "/home/user/.venv/lib/python3.12/site-packages",
        ));
        let npm = Origin::new(Ecosystem::Npm, PathShared::from("/home/user/proj"));
        let lock = Site::new(
            Ecosystem::Npm,
            SiteKind::Lockfile,
            PathShared::from("/home/user/proj/package-lock.json"),
        );
        let origin_to_sites =
            HashMap::from([(py, vec![sp.clone()]), (npm, vec![lock.clone()])]);
        let package_to_sites = HashMap::from([
            (
                Package::from_name_version_durl("requests", "2.31.0", None).unwrap(),
                vec![sp.clone()],
            ),
            (
                Package::new(Ecosystem::Npm, "lodash", VersionSpec::new("4.17.15"), None),
                vec![lock],
            ),
        ]);
        let mut sfs = ScanFS::from_parts(
            origin_to_sites,
            package_to_sites,
            ScanConfig::new(false, false),
            "hash".into(),
        );
        if with_hooks {
            sfs.site_to_hooks = Some(HashMap::from([(
                sp,
                vec![Hook {
                    name: "_virtualenv.pth".into(),
                    detail: "import _virtualenv".into(),
                }],
            )]));
        }
        sfs
    }

    fn names(sfs: &ScanFS) -> Vec<String> {
        let mut out: Vec<String> = sfs
            .package_to_sites
            .keys()
            .map(|p| format!("{}:{p}", p.ecosystem))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn test_legacy_v4_a() {
        let p = MonitorPayload::from_json(V4).unwrap();
        assert_eq!(p.schema, 0);
        assert_eq!(p.tenant, "tenant-key");
        assert_eq!(p.system.hostname, "host");
        assert_eq!(p.time.as_secs(), 1794774241);
        let sfs = p.scan.unwrap();
        // two executables sharing one site, all Python
        assert_eq!(sfs.origin_to_sites.len(), 2);
        assert!(sfs
            .origin_to_sites
            .keys()
            .all(|o| o.ecosystem == Ecosystem::Python));
        assert_eq!(sfs.site_to_origins.len(), 1);
        let site = sfs.site_to_origins.keys().next().unwrap();
        assert_eq!(site.kind, SiteKind::Installed);
        assert_eq!(site.ecosystem, Ecosystem::Python);
        assert_eq!(names(&sfs), ["python:dill-0.3.8", "python:requests-2.31.0"]);
        let dill = sfs
            .package_to_sites
            .keys()
            .find(|p| p.name == "dill")
            .unwrap();
        assert_eq!(
            dill.direct_url
                .as_ref()
                .unwrap()
                .vcs_info
                .as_ref()
                .unwrap()
                .commit_id,
            "a0a8e86976708d0436eec5c8f7d25329da727cb5"
        );
        assert_eq!(sfs.config.ecosystems, [Ecosystem::Python]);
        assert!(sfs.site_to_hooks.is_none());
        // re-sent as schema 1, it reads back the same
        let again = MonitorPayload::new(&p.tenant, p.system, p.time, Some(sfs.clone()));
        let back = MonitorPayload::from_json(&again.to_json().unwrap()).unwrap();
        assert_eq!(back.schema, SCHEMA);
        assert_eq!(back.scan.unwrap(), sfs);
    }

    #[test]
    fn test_legacy_unchanged_a() {
        let mut v: JsonValue = serde_json::from_str(V4).unwrap();
        v[2] = JsonValue::Null;
        let p = MonitorPayload::from_json(&v.to_string()).unwrap();
        assert_eq!(p.schema, 0);
        assert!(p.scan.is_none());
    }

    #[test]
    fn test_round_trip_a() {
        for with_hooks in [false, true] {
            let p = MonitorPayload::new(
                "t",
                system(),
                Duration::new(1_700_000_000, 5),
                Some(sample(with_hooks)),
            );
            let json = p.to_json().unwrap();
            let back = MonitorPayload::from_json(&json).unwrap();
            assert_eq!(back, p);
            assert!(back.skipped.is_empty());
            // hooks are written only when collected
            assert_eq!(json.contains("\"hooks\""), with_hooks);
        }
        // unchanged: no scan
        let p = MonitorPayload::new("t", system(), Duration::ZERO, None);
        let back = MonitorPayload::from_json(&p.to_json().unwrap()).unwrap();
        assert_eq!(back, p);
        assert!(p.to_json().unwrap().contains("\"scan\":null"));
    }

    #[test]
    fn test_schema1_fixture_a() {
        let p = MonitorPayload::from_json(SCHEMA1).unwrap();
        assert_eq!(p.schema, 1);
        assert_eq!(p.fetter_version.as_deref(), Some("5.0.0"));
        assert_eq!(p.time, Duration::new(1_700_000_000, 5));
        assert_eq!(p.scan.as_ref().unwrap(), &sample(true));
        assert!(p.skipped.is_empty());
        // and this fetter writes the same scan object
        let ours: JsonValue = serde_json::from_str(
            &MonitorPayload::new("t", system(), p.time, Some(sample(true)))
                .to_json()
                .unwrap(),
        )
        .unwrap();
        let fixture: JsonValue = serde_json::from_str(SCHEMA1).unwrap();
        assert_eq!(ours["scan"], fixture["scan"]);
        assert_eq!(ours["system"], fixture["system"]);
    }

    #[test]
    fn test_schema1_sites_absent_a() {
        // scans from before `--sites` reported all Sites
        let mut v: JsonValue = serde_json::from_str(SCHEMA1).unwrap();
        v["scan"]["config"].as_object_mut().unwrap().remove("sites");
        let p = MonitorPayload::from_json(&v.to_string()).unwrap();
        assert_eq!(p.scan.unwrap().config.sites, SiteSelect::All);
    }

    #[test]
    fn test_forward_compatible_a() {
        let mut v: JsonValue = serde_json::from_str(
            &MonitorPayload::new("t", system(), Duration::ZERO, Some(sample(true)))
                .to_json()
                .unwrap(),
        )
        .unwrap();
        // a newer schema, with fields, ecosystems and site kinds this reader does not know
        v["schema"] = json!(7);
        v["future_field"] = json!({"x": 1});
        let scan = &mut v["scan"];
        scan["future_table"] = json!([1, 2]);
        let paths = scan["paths"].as_array_mut().unwrap();
        paths.push(json!("/home/user/app"));
        paths.push(json!("/home/user/app/packages.lock.json"));
        paths.push(json!("/home/user/proj/registry"));
        let n = paths.len();
        scan["origins"]
            .as_array_mut()
            .unwrap()
            .push(json!({"ecosystem": "nuget", "path": n - 3, "extra": true}));
        let sites = scan["sites"].as_array_mut().unwrap();
        sites.push(json!({"ecosystem": "nuget", "kind": "lockfile", "path": n - 2}));
        sites.push(json!({"ecosystem": "npm", "kind": "registry", "path": n - 1}));
        let (nuget_site, npm_registry_site) = (sites.len() - 2, sites.len() - 1);
        scan["origin_sites"]
            .as_array_mut()
            .unwrap()
            .push(json!({"origin": 2, "sites": [nuget_site]}));
        let pkgs = scan["packages"].as_array_mut().unwrap();
        pkgs.push(json!({"package": {"ecosystem": "nuget", "name": "Newtonsoft.Json", "version": "13.0.1"}, "sites": [nuget_site]}));
        // a known ecosystem found only at an unknown kind of site
        pkgs.push(json!({"package": {"ecosystem": "npm", "name": "left-pad", "version": "1.3.0", "license": "MIT"}, "sites": [npm_registry_site]}));
        scan["config"]["ecosystems"]
            .as_array_mut()
            .unwrap()
            .push(json!("nuget"));

        let p = MonitorPayload::from_json(&v.to_string()).unwrap();
        assert_eq!(p.schema, 7);
        assert_eq!(p.scan.unwrap(), sample(true));
        assert_eq!(p.skipped.origins, 1);
        assert_eq!(p.skipped.sites, 2);
        assert_eq!(p.skipped.packages, 2);
        assert_eq!(
            p.skipped
                .names
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["nuget", "registry"]
        );
    }

    #[test]
    fn test_errors_a() {
        assert!(MonitorPayload::from_json("not json").is_err());
        assert!(MonitorPayload::from_json("[1, 2]").is_err());
        assert!(MonitorPayload::from_json(r#"{"tenant": "t"}"#)
            .unwrap_err()
            .contains("schema"));
        let mut v: JsonValue = serde_json::from_str(
            &MonitorPayload::new("t", system(), Duration::ZERO, Some(sample(false)))
                .to_json()
                .unwrap(),
        )
        .unwrap();
        v["scan"]["packages"][0]["sites"] = json!([99]);
        assert!(MonitorPayload::from_json(&v.to_string())
            .unwrap_err()
            .contains("out of range"));
    }
}
