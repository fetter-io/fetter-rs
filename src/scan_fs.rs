use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::fs::File;
use std::io;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use rayon::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::audit_report::AuditReport;
use crate::cli::CvssFilter;
use crate::count_report::CountReport;
use crate::dep_manifest::DepManifest;
use crate::ecosystem::Ecosystem;
use crate::env_marker::EnvMarkerState;
use crate::exe_search::find_exe;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::origin_site::SiteKind;
use crate::package::Package;
use crate::package_match::match_str;
use crate::path_shared::PathShared;
use crate::scan_report::ScanReport;
use crate::site_customize::install_validation;
use crate::site_customize::uninstall_validation;
use crate::unpack_report::UnpackReport;
use crate::ureq_client::UreqClient;
use crate::util::exe_path_normalize;
use crate::util::hash_paths;
use crate::util::logger;
use crate::util::path_is_component;
use crate::util::path_normalize;
use crate::util::path_within_duration;
use crate::util::Anchor;
use crate::util::CacheConfig;
use crate::util::FlagCacheRefresh;
use crate::util::FlagLog;
use crate::util::FlagRetainPassing;
use crate::util::ResultDynError;
use crate::util::ScanConfig;
use crate::util::DURATION_0;
use crate::validation_report::ValidationFlags;
use crate::validation_report::ValidationReport;

//------------------------------------------------------------------------------

/// Given a path to a Python binary, call out to Python to get all known site packages; some site packages may not exist; we do not filter them here. This will include "dist-packages" on Linux. If `config.force_usite` is false, we use site.ENABLE_USER_SITE to determine if we should include the user site packages; if `config.force_usite` is true, we always include usite.
/// Calling Python using `-S` disables loading site so that we can mock sitecustomize.py (which fetter might customize). We then call `site.main()` to force proper initialization.
const PY_SITE_PACKAGES: &str = "import sys;import site;import types;sys.modules['fetter_validate'] = types.ModuleType('fetter_validate');site.main();print(site.ENABLE_USER_SITE);print(\"\\n\".join(site.getsitepackages()));print(site.getusersitepackages())";
fn get_site_package_dirs(
    executable: &Path,
    config: &ScanConfig,
    log: FlagLog,
) -> Vec<PathShared> {
    match Command::new(executable)
        .arg("-S") // disable site on startup
        .arg("-c")
        .arg(PY_SITE_PACKAGES)
        .output()
    {
        Ok(output) => {
            let mut paths = Vec::new();
            let mut usite_enabled = false;

            let lines = std::str::from_utf8(&output.stdout)
                .expect("Failed to convert to UTF-8")
                .trim()
                .lines();
            for (i, line) in lines.enumerate() {
                if i == 0 {
                    usite_enabled = line.trim() == "True";
                } else {
                    paths.push(PathShared::from(line.trim()));
                }
            }
            // if necessary, remove the usite
            if !config.force_usite && !usite_enabled {
                let _p = paths.pop();
            }
            paths
        }
        Err(e) => {
            logger!(
                log,
                module_path!(),
                "Failed to execute command with {:?}: {}",
                executable,
                e
            );

            Vec::with_capacity(0)
        }
    }
}

// Given a package directory, collect the name of all packages.
fn get_packages(site_packages: &Path) -> Vec<Package> {
    let mut packages = Vec::new();
    if let Ok(entries) = fs::read_dir(site_packages) {
        for entry in entries.flatten() {
            let file_path = entry.path();
            if let Some(package) = Package::from_file_path(&file_path) {
                packages.push(package);
            }
        }
    }
    packages
}

//------------------------------------------------------------------------------

// The result of a file-system scan.
//
// Invariant: every inner `Vec` is sorted (and so deterministic) after construction; all constructors must end by calling `ScanFS::from_parts`.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanFS {
    // NOTE: these attributes are used by reporters
    /// A mapping of Origin (e.g. a Python executable) to Sites (e.g. site-packages dirs).
    pub origin_to_sites: HashMap<Origin, Vec<Site>>,
    /// A mapping of Package to the Sites in which it was found.
    pub package_to_sites: HashMap<Package, Vec<Site>>,
    /// A mapping of Site to the Origins that use it; derived from `origin_to_sites`.
    pub site_to_origins: HashMap<Site, Vec<Origin>>,
    /// Optionally populate EnvMarkerState for all Python Origins, only if env markers are found.
    pub origin_to_ems: Option<HashMap<Origin, EnvMarkerState>>,
    /// Scan configuration
    config: ScanConfig,
    /// Store the hash of the un-normalized origin inputs for cache lookup.
    origins_hash: String,
}

struct PathIndexer {
    paths: Vec<String>,
    path_to_index: HashMap<PathShared, usize>,
}

impl PathIndexer {
    fn new() -> Self {
        Self {
            paths: Vec::new(),
            path_to_index: HashMap::new(),
        }
    }

    fn get_index<S: serde::ser::Error>(&mut self, p: &PathShared) -> Result<usize, S> {
        if let Some(&idx) = self.path_to_index.get(p) {
            Ok(idx)
        } else {
            let idx = self.paths.len();
            let s = p
                .as_path()
                .to_str()
                .ok_or_else(|| S::custom("Invalid UTF-8 in path"))?;
            self.path_to_index.insert(p.clone(), idx);
            self.paths.push(s.to_string());
            Ok(idx)
        }
    }
}

/// Flattened data representation used for serialization: a path dictionary, an Origin table and a Site table (both referring to paths by index), and mappings that refer to Origins and Sites by index. `site_to_origins` is not stored, as it is derived from `origin_to_sites`.
type ScanFSData = (
    Vec<String>,                       // paths
    Vec<(Ecosystem, usize)>,           // origins: (ecosystem, path index)
    Vec<(Ecosystem, SiteKind, usize)>, // sites: (ecosystem, kind, path index)
    Vec<(usize, Vec<usize>)>,          // origin index to site indices
    Vec<(Package, Vec<usize>)>,        // package to site indices
    bool,                              // force_usite
    bool,                              // all_users
    String,                            // origins_hash
);

impl Serialize for ScanFS {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Assign indices in sorted order for stable output.
        let mut origins: Vec<&Origin> = self.origin_to_sites.keys().collect();
        origins.sort();
        let mut sites: Vec<&Site> = self
            .origin_to_sites
            .values()
            .chain(self.package_to_sites.values())
            .flatten()
            .collect();
        sites.sort();
        sites.dedup();

        let mut pi = PathIndexer::new();
        let mut origin_to_index: HashMap<&Origin, usize> = HashMap::new();
        let mut origin_table = Vec::with_capacity(origins.len());
        for (i, o) in origins.iter().enumerate() {
            origin_table.push((o.ecosystem, pi.get_index(&o.path)?));
            origin_to_index.insert(o, i);
        }
        let mut site_to_index: HashMap<&Site, usize> = HashMap::new();
        let mut site_table = Vec::with_capacity(sites.len());
        for (i, s) in sites.iter().enumerate() {
            site_table.push((s.ecosystem, s.kind, pi.get_index(&s.path)?));
            site_to_index.insert(s, i);
        }

        // inner Vecs are already sorted by construction
        let origin_to_sites_idx: Vec<(usize, Vec<usize>)> = origins
            .iter()
            .map(|o| {
                let site_idx = self.origin_to_sites[*o]
                    .iter()
                    .map(|s| site_to_index[s])
                    .collect();
                (origin_to_index[o], site_idx)
            })
            .collect();

        let mut packages: Vec<&Package> = self.package_to_sites.keys().collect();
        packages.sort();
        let package_to_sites_idx: Vec<(&Package, Vec<usize>)> = packages
            .into_iter()
            .map(|p| {
                let site_idx = self.package_to_sites[p]
                    .iter()
                    .map(|s| site_to_index[s])
                    .collect();
                (p, site_idx)
            })
            .collect();

        let data = (
            pi.paths,
            origin_table,
            site_table,
            origin_to_sites_idx,
            package_to_sites_idx,
            self.config.force_usite,
            self.config.all_users,
            &self.origins_hash,
        );
        data.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ScanFS {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (
            paths,
            origin_table,
            site_table,
            origin_to_sites_idx,
            package_to_sites_idx,
            force_usite,
            all_users,
            origins_hash,
        ): ScanFSData = Deserialize::deserialize(deserializer)?;

        let get = |i: usize| -> Result<PathShared, D::Error> {
            paths
                .get(i)
                .map(|p| PathShared::from(p.as_str()))
                .ok_or_else(|| serde::de::Error::custom("path index out of range"))
        };
        let origins: Vec<Origin> = origin_table
            .into_iter()
            .map(|(e, i)| Ok(Origin::new(e, get(i)?)))
            .collect::<Result<_, D::Error>>()?;
        let sites: Vec<Site> = site_table
            .into_iter()
            .map(|(e, k, i)| Ok(Site::new(e, k, get(i)?)))
            .collect::<Result<_, D::Error>>()?;

        let site_at = |i: usize| -> Result<Site, D::Error> {
            sites
                .get(i)
                .cloned()
                .ok_or_else(|| serde::de::Error::custom("site index out of range"))
        };
        let mut origin_to_sites = HashMap::new();
        for (oi, sis) in origin_to_sites_idx {
            let origin = origins
                .get(oi)
                .cloned()
                .ok_or_else(|| serde::de::Error::custom("origin index out of range"))?;
            let s: Vec<Site> = sis.into_iter().map(site_at).collect::<Result<_, _>>()?;
            origin_to_sites.insert(origin, s);
        }
        let mut package_to_sites = HashMap::new();
        for (pkg, sis) in package_to_sites_idx {
            let s: Vec<Site> = sis.into_iter().map(site_at).collect::<Result<_, _>>()?;
            package_to_sites.insert(pkg, s);
        }

        Ok(ScanFS::from_parts(
            origin_to_sites,
            package_to_sites,
            ScanConfig::new(force_usite, all_users),
            origins_hash,
        ))
    }
}

/// Collect packages from a Site, dispatching on its ecosystem.
fn site_to_packages(site: &Site) -> Vec<Package> {
    match site.ecosystem {
        Ecosystem::Python => get_packages(site.as_path()),
        // other ecosystems are not yet implemented
        _ => Vec::new(),
    }
}

impl ScanFS {
    /// Final assembly of a ScanFS: derives `site_to_origins` and sorts all inner Vecs so that all downstream output is deterministic. All constructors must end here.
    fn from_parts(
        mut origin_to_sites: HashMap<Origin, Vec<Site>>,
        mut package_to_sites: HashMap<Package, Vec<Site>>,
        config: ScanConfig,
        origins_hash: String,
    ) -> Self {
        let mut site_to_origins: HashMap<Site, Vec<Origin>> = HashMap::new();
        for (origin, sites) in &origin_to_sites {
            for site in sites {
                site_to_origins
                    .entry(site.clone())
                    .or_default()
                    .push(origin.clone());
            }
        }
        // NOTE: slice sort returns early for len < 2, so no need to guard here
        for sites in origin_to_sites.values_mut() {
            sites.sort();
        }
        for sites in package_to_sites.values_mut() {
            sites.sort();
        }
        for origins in site_to_origins.values_mut() {
            origins.sort();
        }
        ScanFS {
            origin_to_sites,
            package_to_sites,
            site_to_origins,
            origin_to_ems: None,
            config,
            origins_hash,
        }
    }

    /// Given a mapping of Origins to Sites, read all packages from all Sites.
    fn from_origin_to_sites(
        origin_to_sites: HashMap<Origin, Vec<Site>>,
        config: &ScanConfig,
        origins_hash: String,
    ) -> ResultDynError<Self> {
        // Collect unique sites so that each site (which might be shared by multiple origins) is only read once
        let sites: HashSet<&Site> = origin_to_sites.values().flatten().collect();
        let site_to_packages: Vec<(Site, Vec<Package>)> = sites
            .into_par_iter()
            .map(|site| (site.clone(), site_to_packages(site)))
            .collect();

        let mut package_to_sites: HashMap<Package, Vec<Site>> = HashMap::new();
        for (site, packages) in site_to_packages {
            for package in packages {
                package_to_sites
                    .entry(package)
                    .or_default()
                    .push(site.clone());
            }
        }
        Ok(Self::from_parts(
            origin_to_sites,
            package_to_sites,
            *config,
            origins_hash,
        ))
    }

    /// Create a ScanFS from a cache: exes provided here should be pre-normalization.
    pub(crate) fn from_cache(
        exes: &[PathBuf],
        config: &ScanConfig,
        cache_config: &CacheConfig,
        log: FlagLog,
    ) -> ResultDynError<Self> {
        if cache_config.duration == DURATION_0 {
            Err("Cache disabled by duration".into())
        } else {
            let origins_hash = hash_paths(exes, config);
            let cache_fp = Self::cache_path(cache_config, &origins_hash);

            if path_within_duration(&cache_fp, cache_config.duration) {
                logger!(log, module_path!(), "Loading ScanFS cache: {:?}", cache_fp);

                let mut file = File::open(cache_fp)?;
                let mut contents = String::new();
                file.read_to_string(&mut contents)?;
                let data: ScanFS = serde_json::from_str(&contents)?;
                Ok(data)
            } else if cache_fp.exists() {
                // NOTE: could remove cache_fp to clean up
                Err("Cache expired".into())
            } else {
                Err("Cache file does not exist".into())
            }
        }
    }

    /// Path of the cache file for a given hash; the `v6` marker ensures caches from prior formats are never read.
    fn cache_path(cache_config: &CacheConfig, origins_hash: &str) -> PathBuf {
        cache_config
            .directory
            .join(format!("scan_fs_v6_{origins_hash}"))
            .with_extension("json")
    }

    /// Given a Vec of PathBuf to executables, use them to collect site packages. In this function, provided PathBuf are normalized to absolute paths, and if a PathBuf is "*", a system-wide path search will be conducted.
    pub(crate) fn from_exes(
        exes: &Vec<PathBuf>,
        config: &ScanConfig,
        log: FlagLog,
    ) -> ResultDynError<Self> {
        let path_wild = PathBuf::from("*");
        let mut exes_norm = Vec::new();
        for e in exes {
            if path_is_component(e) && *e == path_wild {
                exes_norm.extend(find_exe(config.all_users));
            } else {
                exes_norm.push(exe_path_normalize(e)?);
            }
        }

        let origin_to_sites: HashMap<Origin, Vec<Site>> = exes_norm
            .into_par_iter()
            .map(|exe| {
                let sites = get_site_package_dirs(&exe, config, log)
                    .into_iter()
                    .map(Site::python)
                    .collect();
                (Origin::new(Ecosystem::Python, PathShared::from(exe)), sites)
            })
            .collect();

        let origins_hash = hash_paths(exes, config);
        Self::from_origin_to_sites(origin_to_sites, config, origins_hash)
    }

    /// Alternative constructor from in-memory objects, only for testing. Here we provide notional exe and site paths, and focus just on collecting Packages.
    #[cfg(test)]
    pub(crate) fn from_exe_site_packages(
        exe: PathBuf,
        site: PathBuf,
        packages: Vec<Package>,
    ) -> ResultDynError<Self> {
        let site = Site::python(PathShared::from_path_buf(site));
        let mut origin_to_sites = HashMap::new();
        origin_to_sites.insert(
            Origin::new(Ecosystem::Python, PathShared::from(exe.clone())),
            vec![site.clone()],
        );
        let mut package_to_sites = HashMap::new();
        for package in packages {
            package_to_sites
                .entry(package)
                .or_insert_with(Vec::new)
                .push(site.clone());
        }
        let config = ScanConfig::new(false, false);
        let origins_hash = hash_paths(&[exe], &config);
        Ok(Self::from_parts(
            origin_to_sites,
            package_to_sites,
            config,
            origins_hash,
        ))
    }

    //--------------------------------------------------------------------------

    // If not set, optionally load EnvMarkerState for each Python Origin
    pub(crate) fn load_env_marker_state(&mut self, log: FlagLog) {
        logger!(log, module_path!(), "Fetching EnvMarkerState");

        if self.origin_to_ems.is_none() {
            let ems_map: HashMap<Origin, EnvMarkerState> = self
                .origin_to_sites
                .keys()
                .filter(|o| o.ecosystem == Ecosystem::Python)
                .cloned()
                .collect::<Vec<_>>()
                .into_par_iter()
                .map(|origin| {
                    let ems = EnvMarkerState::from_exe(origin.as_path()).unwrap();
                    (origin, ems)
                })
                .collect();

            self.origin_to_ems = Some(ems_map);
        }
    }

    // searching
    pub(crate) fn search_by_match(
        &self,
        pattern: &str,
        case_insensitive: bool,
    ) -> Vec<Package> {
        // take ownership of Package in the result of get_packages
        let matched = self
            .get_packages()
            .into_par_iter()
            .filter(|package| {
                match_str(pattern, package.to_string().as_str(), case_insensitive)
            })
            .collect();
        matched
    }

    /// Return sorted packages.
    pub fn get_packages(&self) -> Vec<Package> {
        let mut packages: Vec<Package> = self.package_to_sites.keys().cloned().collect();
        packages.sort();
        packages
    }

    //--------------------------------------------------------------------------

    pub(crate) fn to_cache(
        &self,
        cache_config: &CacheConfig,
        log: FlagLog,
    ) -> ResultDynError<()> {
        // use hash of exes observed at initialization
        let cache_fp = Self::cache_path(cache_config, &self.origins_hash);

        // only write if cache does not exist or it is out of duration
        if !cache_fp.exists() || !path_within_duration(&cache_fp, cache_config.duration) {
            logger!(log, module_path!(), "Writing ScanFS cache: {:?}", cache_fp);

            let json = serde_json::to_string(self)?;
            let mut file = File::create(cache_fp)?;
            file.write_all(json.as_bytes())?;
            Ok(())
        } else {
            logger!(log, module_path!(), "Keeping ScanFS cache {:?}", cache_fp);
            Ok(())
        }
    }

    //--------------------------------------------------------------------------

    /// Validate this scan against the provided DepManifest.
    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn to_validation_report(
        &mut self,
        dm: DepManifest,
        vf: ValidationFlags,
        ignore: Option<&HashSet<String>>,
        log: FlagLog,
    ) -> ValidationReport {
        if dm.env_marker_active {
            self.load_env_marker_state(log);
        }
        let packages = self.get_packages();

        ValidationReport::from_components(
            &packages,
            &self.package_to_sites,
            &self.site_to_origins,
            &self.origin_to_ems,
            &dm,
            &vf,
            ignore,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn to_audit_report(
        &self,
        pattern: &str,
        client: Arc<dyn UreqClient>,
        case_insensitive: bool,
        cache_refresh: FlagCacheRefresh,
        cache_config: &CacheConfig,
        log: FlagLog,
        filter_cvss: CvssFilter,
        retain_passing: FlagRetainPassing,
    ) -> AuditReport {
        let packages = self.search_by_match(pattern, case_insensitive);
        // NOTE: packages is sorted upstream
        AuditReport::from_packages(
            client,
            &packages,
            cache_refresh,
            cache_config,
            log,
            filter_cvss,
            retain_passing,
        )
    }

    /// The `count` Boolean determine what type of UnpackReport is returned
    pub(crate) fn to_unpack_report(
        &self,
        pattern: &str,
        case_insensitive: bool,
        count: bool,
    ) -> UnpackReport {
        let mut packages = self.search_by_match(pattern, case_insensitive);
        packages.sort();
        let package_to_sites = packages
            .iter()
            .map(|p| (p.clone(), self.package_to_sites.get(p).unwrap().clone()))
            .collect();

        UnpackReport::from_package_to_sites(count, &package_to_sites)
    }

    /// Given an `anchor`, produce a DepManifest based ont the packages observed in this scan.
    pub(crate) fn to_dep_manifest(
        &self,
        anchor: Anchor,
    ) -> Result<DepManifest, Box<dyn std::error::Error>> {
        DepManifest::from_packages(self.package_to_sites.keys(), anchor)
    }

    pub(crate) fn to_scan_report(&self) -> ScanReport {
        ScanReport::from_package_to_sites(&self.package_to_sites)
    }

    pub(crate) fn to_count_report(&self) -> CountReport {
        CountReport::from_scan_fs(self)
    }

    pub(crate) fn to_search_report(
        &self,
        pattern: &str,
        case_insensitive: bool,
    ) -> ScanReport {
        let packages = self.search_by_match(pattern, case_insensitive);
        // println!("packages: {:?}", packages);
        ScanReport::from_packages(&packages, &self.package_to_sites)
    }

    pub(crate) fn to_purge_pattern(
        &self,
        pattern: &Option<String>,
        case_insensitive: bool,
        log: FlagLog,
    ) -> io::Result<()> {
        let packages = match pattern {
            Some(p) => self.search_by_match(p, case_insensitive),
            None => self.package_to_sites.keys().cloned().collect(),
        };
        // packages.sort();
        let package_to_sites = packages
            .iter()
            .map(|p| (p.clone(), self.package_to_sites.get(p).unwrap().clone()))
            .collect();

        let sr = UnpackReport::from_package_to_sites(false, &package_to_sites);
        sr.remove(log)
    }

    #[allow(clippy::wrong_self_convention)]
    pub(crate) fn to_purge_invalid(
        &mut self,
        dm: DepManifest,
        vf: ValidationFlags,
        log: FlagLog,
    ) -> io::Result<()> {
        let vr = self.to_validation_report(dm, vf, None, log);
        let packages: Vec<Package> = vr
            .records
            .iter()
            .filter_map(|r| r.package.as_ref().cloned())
            .collect();
        // packages.sort();
        let package_to_sites = packages
            .iter()
            .map(|p| (p.clone(), self.package_to_sites.get(p).unwrap().clone()))
            .collect();

        let sr = UnpackReport::from_package_to_sites(false, &package_to_sites);
        sr.remove(log)
    }

    pub(crate) fn site_validate_install(
        &self,
        bound: &Path,
        bound_options: &Option<Vec<String>>,
        ignore: &[String],
        vf: &ValidationFlags,
        exit_else_warn: Option<i32>,
        log: FlagLog,
    ) -> ResultDynError<()> {
        if self.origin_to_sites.len() > 1 {
            return Err(format!("site-install will not operate on multiple ({}) Python environments; use `-e` to specify a single Python environment.", self.origin_to_sites.len()).into());
        }
        let ba = path_normalize(bound, true)?;
        // only a single exe, so no need to parallelize
        for (exe, sites) in &self.origin_to_sites {
            // NOTE: taking the first site, but might prioritize by some other criteria
            if let Some(site) = sites.first() {
                install_validation(
                    exe.as_path(),
                    &ba,
                    bound_options,
                    ignore,
                    vf,
                    exit_else_warn,
                    &site.path,
                    env::current_dir().ok(), // as option type
                    log,
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn site_validate_uninstall(&self, log: FlagLog) -> ResultDynError<()> {
        if self.origin_to_sites.len() > 1 {
            return Err(format!("site-install will not operate on multiple ({}) Python environments; use `-e` to specify a single Python environment.", self.origin_to_sites.len()).into());
        }
        for sites in self.origin_to_sites.values() {
            if let Some(site) = sites.first() {
                uninstall_validation(&site.path, log)?;
            }
        }
        Ok(())
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;
    // use rand::seq::SliceRandom;
    // use rand::rng;

    /// Resolve a real interpreter path the way production code does, so tests
    /// that evaluate environment markers spawn an interpreter that actually
    /// exists on this platform (Windows often has "python", not "python3").
    fn resolved_python_exe() -> PathBuf {
        crate::util::default_python_exe_names()
            .iter()
            .find_map(|name| crate::util::get_absolute_path_from_exe(name))
            .expect("no python interpreter found on PATH")
    }

    #[test]
    fn test_get_site_package_dirs_a() {
        let p1 = Path::new("python3");
        let config_force = ScanConfig::new(true, false);
        let paths1 = get_site_package_dirs(p1, &config_force, FlagLog(false));
        assert!(!paths1.is_empty());
        let config_no_force = ScanConfig::new(false, false);
        let paths2 = get_site_package_dirs(p1, &config_no_force, FlagLog(false));
        assert!(paths1.len() >= paths2.len());
    }
    #[test]
    fn test_from_exe_to_sites_a() {
        let fp_dir = tempdir().unwrap();
        let fp_exe = fp_dir.path().join("python");
        let _ = File::create(fp_exe.clone()).unwrap();

        let fp_sp = fp_dir.path().join("site-packages");
        fs::create_dir(fp_sp.clone()).unwrap();

        let fp_p1 = fp_sp.join("numpy-1.19.1.dist-info");
        fs::create_dir(&fp_p1).unwrap();

        let fp_p2 = fp_sp.join("foo-3.0.dist-info");
        fs::create_dir(&fp_p2).unwrap();

        let mut origin_to_sites = HashMap::<Origin, Vec<Site>>::new();
        origin_to_sites.insert(
            Origin::new(Ecosystem::Python, PathShared::from(fp_exe.clone())),
            vec![Site::python(PathShared::from_path_buf(fp_sp.to_path_buf()))],
        );
        let config = ScanConfig::new(false, false);
        let mut sfs =
            ScanFS::from_origin_to_sites(origin_to_sites, &config, "".to_string())
                .unwrap();
        assert_eq!(sfs.package_to_sites.len(), 2);

        let dm1 = DepManifest::try_from_iter(vec!["numpy >= 1.19", "foo==3"]).unwrap();
        assert_eq!(dm1.len(), 2);
        let invalid1 = sfs.to_validation_report(
            dm1,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        assert_eq!(invalid1.len(), 0);

        let dm2 = DepManifest::try_from_iter(vec!["numpy >= 2", "foo==3"]).unwrap();
        let invalid2 = sfs.to_validation_report(
            dm2,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        assert_eq!(invalid2.len(), 1);
    }
    //--------------------------------------------------------------------------
    #[test]
    fn from_exe_site_packages_a() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3.8/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("numpy", "1.20.1", None).unwrap(),
            Package::from_name_version_durl("numpy", "2.1.1", None).unwrap(),
            Package::from_name_version_durl("requests", "0.7.6", None).unwrap(),
            Package::from_name_version_durl("requests", "2.32.3", None).unwrap(),
            Package::from_name_version_durl("flask", "3.0.3", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();
        assert_eq!(sfs.package_to_sites.len(), 7);
        // sfs.report();
        let dm = sfs.to_dep_manifest(Anchor::Lower).unwrap();
        assert_eq!(dm.len(), 3);
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_validation_a() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("requests", "0.7.6", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let dm = DepManifest::try_from_iter(
            ["numpy>1.19", "requests==0.7.6", "flask> 1"].iter(),
        )
        .unwrap();

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();
        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        assert_eq!(vr.len(), 0);
    }
    #[test]
    fn test_validation_b() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("requests", "0.7.6", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let dm = DepManifest::try_from_iter(
            ["numpy>1.19", "requests==0.7.6", "flask> 2"].iter(),
        )
        .unwrap();

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();
        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );

        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[{"package":"flask-1.1.3","dependency":"flask>2","explain":"Misdefined","sites":["/usr/lib/python3/site-packages"]}]"#
        );
    }
    #[test]
    fn test_validation_c() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("requests", "0.7.6", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let dm = DepManifest::try_from_iter(
            ["numpy>2", "requests==0.7.1", "flask> 2,<3"].iter(),
        )
        .unwrap();

        let mut sfs =
            ScanFS::from_exe_site_packages(exe.clone(), site, packages).unwrap();
        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        assert_eq!(
            sfs.origin_to_sites
                .get(&Origin::new(Ecosystem::Python, PathShared::from(exe)))
                .unwrap()[0]
                .path
                .strong_count(),
            8
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[{"package":"flask-1.1.3","dependency":"flask>2,<3","explain":"Misdefined","sites":["/usr/lib/python3/site-packages"]},{"package":"numpy-1.19.3","dependency":"numpy>2","explain":"Misdefined","sites":["/usr/lib/python3/site-packages"]},{"package":"requests-0.7.6","dependency":"requests==0.7.1","explain":"Misdefined","sites":["/usr/lib/python3/site-packages"]}]"#
        );
    }

    #[test]
    fn test_validation_d() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("requests", "0.7.6", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let dm = DepManifest::try_from_iter(["numpy>2", "flask> 2,<3"].iter()).unwrap();

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: true,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[{"package":"flask-1.1.3","dependency":"flask>2,<3","explain":"Misdefined","sites":["/usr/lib/python3/site-packages"]},{"package":"numpy-1.19.3","dependency":"numpy>2","explain":"Misdefined","sites":["/usr/lib/python3/site-packages"]}]"#
        );
    }
    #[test]
    fn test_validation_e() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // hyphen / underscore are normalized
        let dm = DepManifest::try_from_iter(
            ["numpy==1.19.3", "flask>1,<2", "static_frame==2.13.0"].iter(),
        )
        .unwrap();
        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        assert_eq!(vr.len(), 0);
    }
    #[test]
    fn test_validation_f() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
        ];
        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // hyphen / underscore are normalized
        let dm = DepManifest::try_from_iter(
            ["numpy==1.19.3", "flask>1,<2", "static_frame==2.13.0"].iter(),
        )
        .unwrap();
        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        assert_eq!(vr.len(), 1);
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[{"package":null,"dependency":"flask>1,<2","explain":"Missing","sites":null}]"#
        );
    }
    #[test]
    fn test_validation_g() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
        ];
        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();
        let dm = DepManifest::try_from_iter(["numpy==1.19.3"].iter()).unwrap();
        let vr1 = sfs.to_validation_report(
            dm.clone(),
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        assert_eq!(vr1.len(), 1);
        let json = serde_json::to_string(&vr1.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[{"package":"static-frame-2.13.0","dependency":null,"explain":"Unrequired","sites":["/usr/lib/python3/site-packages"]}]"#
        );

        let vr2 = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: true,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        assert_eq!(vr2.len(), 0);
    }
    #[test]
    fn test_validation_h() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
        ];
        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // hyphen / underscore are normalized
        let dm = DepManifest::try_from_iter(
            ["numpy==1.19.3", "flask>1,<2", "static_frame==2.13.0"].iter(),
        )
        .unwrap();
        let vr1 = sfs.to_validation_report(
            dm.clone(),
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr1.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[{"package":null,"dependency":"flask>1,<2","explain":"Missing","sites":null}]"#
        );

        let vr2 = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: true,
            },
            None,
            FlagLog(false),
        );
        assert_eq!(vr2.len(), 0);
    }

    //--------------------------------------------------------------------------

    #[test]
    fn test_validation_evn_marker_a() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let mut packages =
            vec![
                Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            ];
        if env::consts::OS == "macos" {
            packages
                .push(Package::from_name_version_durl("numpy", "1.19.3", None).unwrap());
        }
        if env::consts::OS == "linux" {
            packages.push(Package::from_name_version_durl("numpy", "2.1", None).unwrap());
        }
        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        let dm = DepManifest::try_from_iter(
            [
                "numpy==1.19.3; platform_system == 'Darwin'",
                "numpy==2.1; platform_system == 'Linux'",
                "static_frame==2.13.0",
            ]
            .iter(),
        )
        .unwrap();
        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(json, r#"[]"#);
    }

    #[test]
    fn test_validation_evn_marker_b1() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("numpy", "2.0", None).unwrap(),
        ];

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // this DM means that we only need NumPy if Python < 3,
        let dm = DepManifest::try_from_iter(
            [
                "numpy==1.19.3; python_version < '3.0'",
                "static_frame==2.13.0",
            ]
            .iter(),
        )
        .unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[{"package":"numpy-2.0","dependency":null,"explain":"Unrequired","sites":["/usr/lib/python3/site-packages"]}]"#
        );
    }

    #[test]
    fn test_validation_evn_marker_b2() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("numpy", "2.0", None).unwrap(),
        ];

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // this DM means that we only need NumPy if Python < 3,
        let dm = DepManifest::try_from_iter(
            [
                "numpy==1.19.3; python_version < '3.0'",
                "static_frame==2.13.0",
            ]
            .iter(),
        )
        .unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: true,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(json, r#"[]"#);
    }

    #[test]
    fn test_validation_evn_marker_b3() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("numpy", "2.0", None).unwrap(),
        ];

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // this DM means that we only need NumPy if Python < 3,
        let dm = DepManifest::try_from_iter(
            [
                "numpy==1.19.3; python_version < '3.0'",
                "static_frame==2.13.0; python_version >= '3.0'",
            ]
            .iter(),
        )
        .unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: true,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(json, r#"[]"#);
    }

    #[test]
    fn test_validation_evn_marker_b4() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("numpy", "2.0", None).unwrap(),
        ];

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // this DM means that we only need NumPy if Python < 3,
        let dm = DepManifest::try_from_iter(
            [
                "numpy==1.19.3; python_version < '3.0'",
                "static_frame==2.13.0; python_version >= '20.0'",
            ]
            .iter(),
        )
        .unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[{"package":"numpy-2.0","dependency":null,"explain":"Unrequired","sites":["/usr/lib/python3/site-packages"]},{"package":"static-frame-2.13.0","dependency":null,"explain":"Unrequired","sites":["/usr/lib/python3/site-packages"]}]"#
        );
    }

    #[test]
    fn test_validation_evn_marker_b5() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("numpy", "2.0", None).unwrap(),
        ];

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // this DM means that we only need NumPy if Python < 3,
        let dm = DepManifest::try_from_iter(
            [
                "numpy==1.19.3; python_version < '3.0'",
                "static_frame==2.13.0; python_version >= '20.0'",
            ]
            .iter(),
        )
        .unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: true,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[{"package":"numpy-2.0","dependency":null,"explain":"Unrequired","sites":["/usr/lib/python3/site-packages"]},{"package":"static-frame-2.13.0","dependency":null,"explain":"Unrequired","sites":["/usr/lib/python3/site-packages"]}]"#
        );
    }

    #[test]
    fn test_validation_evn_marker_b6() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("numpy", "2.0", None).unwrap(),
        ];

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // this DM means that we only need NumPy if Python < 3,
        let dm = DepManifest::try_from_iter(
            [
                "numpy==1.19.3; python_version < '3.0'",
                "static_frame==2.13.0; python_version >= '20.0'",
            ]
            .iter(),
        )
        .unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: true,
                permit_subset: true,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(json, r#"[]"#);
    }

    #[test]
    fn test_validation_evn_marker_c1() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
        ];

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        let dm = DepManifest::try_from_iter(
            [
                "numpy==1.2; python_version > '20'",
                "numpy==1.19.3; python_version > '3.0' and python_version < '20'",
                "numpy==2.0; python_version < '3.0'",
                "static_frame==2.13.0",
            ]
            .iter(),
        )
        .unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(json, r#"[]"#);
    }

    #[test]
    fn test_validation_evn_marker_c2() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
        ];

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        let dm = DepManifest::try_from_iter(
            [
                "numpy==1.2; python_version > '20'",
                "numpy==1.19.1; python_version > '3.0' and python_version < '20'",
                "numpy==2.0; python_version < '3.0'",
                "static_frame==2.13.0",
            ]
            .iter(),
        )
        .unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[{"package":"numpy-1.19.3","dependency":"numpy==1.19.1; python_version > '3.0' and python_version < '20'","explain":"Misdefined","sites":["/usr/lib/python3/site-packages"]}]"#
        );
    }

    #[test]
    fn test_validation_evn_marker_c3() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages =
            vec![
                Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            ];

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        let dm = DepManifest::try_from_iter(
            [
                "numpy==1.2; python_version > '20'",
                "numpy==1.19.1; python_version > '3.0' and python_version < '20'",
                "numpy==2.0; python_version < '3.0'",
                "static_frame==2.13.0",
            ]
            .iter(),
        )
        .unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[{"package":null,"dependency":"numpy==1.19.1; python_version > '3.0' and python_version < '20'","explain":"Missing","sites":null}]"#
        );
    }

    #[test]
    fn test_validation_evn_marker_c4() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages =
            vec![
                Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            ];

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        let dm = DepManifest::try_from_iter(
            [
                "numpy==1.2; python_version > '20'",
                "numpy==1.19.1; python_version > '3.0' and python_version < '20'",
                "numpy==2.0; python_version < '3.0'",
                "static_frame==2.13.0",
            ]
            .iter(),
        )
        .unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: true,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(json, r#"[]"#);
    }

    #[test]
    fn test_validation_evn_marker_c5() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
        ];

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        let dm = DepManifest::try_from_iter(
            [
                "numpy==1.2; python_version > '20'",
                "numpy==2.0; python_version < '3.0'",
                "static_frame==2.13.0",
            ]
            .iter(),
        )
        .unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[{"package":"numpy-1.19.3","dependency":null,"explain":"Unrequired","sites":["/usr/lib/python3/site-packages"]}]"#
        );
    }

    #[test]
    fn test_validation_evn_marker_c6() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
        ];

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        let dm = DepManifest::try_from_iter(
            [
                "numpy==1.2; python_version > '20'",
                "numpy==2.0; python_version < '3.0'",
                "static_frame==2.13.0",
            ]
            .iter(),
        )
        .unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: true,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(json, r#"[]"#);
    }

    #[test]
    fn test_validation_evn_marker_d1() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("numpy", "2.0", None).unwrap(),
        ];

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        let dm = DepManifest::try_from_iter(
            ["numpy==1.2", "numpy==2.0", "static_frame==2.13.0"].iter(),
        )
        .unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: true,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(json, r#"[]"#);
    }

    #[test]
    fn test_validation_evn_marker_d2() {
        let exe = PathBuf::from("python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("numpy", "2.2", None).unwrap(),
        ];

        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        let dm = DepManifest::try_from_iter(
            ["numpy==1.2", "numpy==2.0", "static_frame==2.13.0"].iter(),
        )
        .unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: true,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        assert_eq!(
            json,
            r#"[{"package":null,"dependency":"numpy==1.2","explain":"Missing","sites":null},{"package":null,"dependency":"numpy==2.0","explain":"Missing","sites":null}]"#
        );
    }

    #[test]
    fn test_validation_evn_marker_e() {
        // Resolve a real interpreter so the win32 markers evaluate against this
        // platform (a bare "python3" may not be spawnable on Windows).
        let exe = resolved_python_exe();
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![];
        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        let content = r#"
# This file is automatically @generated by Poetry 2.1.1 and should not be changed by hand.

[[package]]
name = "pathlib2"
version = "2.3.7.post1"
description = "Object-oriented filesystem paths"
optional = false
python-versions = "*"
groups = ["main"]
markers = "sys_platform == \"win32\""
files = [
    {file = "pathlib2-2.3.7.post1-py2.py3-none-any.whl", hash = "sha256:5266a0fd000452f1b3467d782f079a4343c63aaa119221fbdc4e39577489ca5b"},
    {file = "pathlib2-2.3.7.post1.tar.gz", hash = "sha256:9fe0edad898b83c0c3e199c842b27ed216645d2e177757b2dd67384d4113c641"},
]

[package.dependencies]
six = "*"

[[package]]
name = "six"
version = "1.17.0"
description = "Python 2 and 3 compatibility utilities"
optional = false
python-versions = "!=3.0.*,!=3.1.*,!=3.2.*,>=2.7"
groups = ["main"]
markers = "sys_platform == \"win32\""
files = [
    {file = "six-1.17.0-py2.py3-none-any.whl", hash = "sha256:4721f391ed90541fddacab5acf947aa0d3dc7d27b2e1e8eda2be8970586c3274"},
    {file = "six-1.17.0.tar.gz", hash = "sha256:ff70335d468e7eb6ec65b95b99d3a2836546063f63acc5171de367e834932a81"},
]

[metadata]
lock-version = "2.1"
python-versions = ">=3.13"
content-hash = "f05bd817b200790c9d7fdfecc11143473da90202f39a4a185ba66e28b04e079a"
"#;

        let fp_dir = tempdir().unwrap();
        let fp_lock = fp_dir.path().join("poetry.lock");
        let mut file = File::create(&fp_lock).unwrap();
        write!(file, "{}", content).unwrap();
        let dm = DepManifest::from_path(&fp_lock, None).unwrap();

        let vr = sfs.to_validation_report(
            dm,
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json = serde_json::to_string(&vr.to_validation_digest()).unwrap();
        // Both dependencies are gated on `sys_platform == "win32"`, so they are
        // required (and Missing) on Windows but excluded on other platforms.
        #[cfg(windows)]
        let expected = r#"[{"package":null,"dependency":"pathlib2==2.3.7.post1; sys_platform == \"win32\"","explain":"Missing","sites":null},{"package":null,"dependency":"six==1.17.0; python_version != '3.0.*' and python_version != '3.1.*' and python_version != '3.2.*' and python_version >= '2.7' and sys_platform == \"win32\"","explain":"Missing","sites":null}]"#;
        #[cfg(not(windows))]
        let expected = r#"[]"#;
        assert_eq!(json, expected);
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_validation_ignore_a() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.2", None).unwrap(),
            Package::from_name_version_durl("pip", "23.1.2", None).unwrap(),
        ];
        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // hyphen / underscore are normalized
        let dm =
            DepManifest::try_from_iter(["numpy==1.19.3", "static_frame>=2.13.0"].iter())
                .unwrap();

        let vr1 = sfs.to_validation_report(
            dm.clone(),
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json1 = serde_json::to_string(&vr1.to_validation_digest()).unwrap();
        assert_eq!(
            json1,
            r#"[{"package":"pip-23.1.2","dependency":null,"explain":"Unrequired","sites":["/usr/lib/python3/site-packages"]}]"#
        );

        let mut ignore: HashSet<String> = HashSet::new();
        ignore.insert("pip".to_string());

        let vr2 = sfs.to_validation_report(
            dm.clone(),
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            Some(&ignore),
            FlagLog(false),
        );
        let json2 = serde_json::to_string(&vr2.to_validation_digest()).unwrap();
        assert_eq!(json2, r#"[]"#);
    }

    #[test]
    fn test_validation_ignore_b() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.2", None).unwrap(),
        ];
        let mut sfs = ScanFS::from_exe_site_packages(exe, site, packages).unwrap();

        // hyphen / underscore are normalized
        let dm = DepManifest::try_from_iter(
            ["numpy==1.19.3", "static_frame>=2.13.0", "pip==23.1.2"].iter(),
        )
        .unwrap();

        let vr1 = sfs.to_validation_report(
            dm.clone(),
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            None,
            FlagLog(false),
        );
        let json1 = serde_json::to_string(&vr1.to_validation_digest()).unwrap();
        assert_eq!(
            json1,
            r#"[{"package":null,"dependency":"pip==23.1.2","explain":"Missing","sites":null}]"#
        );

        let mut ignore: HashSet<String> = HashSet::new();
        ignore.insert("pip".to_string());

        let vr2 = sfs.to_validation_report(
            dm.clone(),
            ValidationFlags {
                permit_superset: false,
                permit_subset: false,
            },
            Some(&ignore),
            FlagLog(false),
        );
        let json2 = serde_json::to_string(&vr2.to_validation_digest()).unwrap();
        assert_eq!(json2, r#"[]"#);
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_search_a() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages.clone()).unwrap();
        let matched = sfs.search_by_match("*.3", true);
        assert_eq!(matched, vec![packages[2].clone(), packages[0].clone()]);
    }

    #[test]
    fn test_search_b() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages.clone()).unwrap();
        let matched = sfs.search_by_match("*frame*", true);
        assert_eq!(matched, vec![packages[1].clone()]);
    }

    //--------------------------------------------------------------------------

    #[test]
    fn test_serialize_a() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages.clone()).unwrap();
        let json = serde_json::to_string(&sfs).unwrap();
        assert_eq!(json, "[[\"/usr/bin/python3\",\"/usr/lib/python3/site-packages\"],[[\"python\",0]],[[\"python\",\"installed\",1]],[[0,[0]]],[[{\"ecosystem\":\"python\",\"name\":\"flask\",\"key\":\"flask\",\"version\":\"1.1.3\",\"direct_url\":null},[0]],[{\"ecosystem\":\"python\",\"name\":\"numpy\",\"key\":\"numpy\",\"version\":\"1.19.3\",\"direct_url\":null},[0]],[{\"ecosystem\":\"python\",\"name\":\"static-frame\",\"key\":\"static_frame\",\"version\":\"2.13.0\",\"direct_url\":null},[0]]],false,false,\"c0b5205c57aa54df7dcbddb79399a81accfe1198a5a7842cb99ac806fb1524a7\"]");

        let sfsd: ScanFS = serde_json::from_str(&json).unwrap();
        assert_eq!(sfsd.origin_to_sites.len(), 1);
        assert_eq!(sfsd.package_to_sites.len(), 3);
        assert_eq!(sfsd, sfs);
    }

    #[test]
    fn test_serialize_b() {
        let exe1 = PathBuf::from("/usr/bin/python3");
        let exe2 = PathBuf::from("/opt/venv/bin/python");
        let site1 = PathBuf::from("/usr/lib/python3/site-packages");
        let site2 = PathBuf::from("/opt/venv/lib/python3.9/site-packages");

        let pkg_numpy = Package::from_name_version_durl("numpy", "1.21.0", None).unwrap();
        let pkg_pandas =
            Package::from_name_version_durl("pandas", "1.3.0", None).unwrap();
        let pkg_flask = Package::from_name_version_durl("flask", "2.0.1", None).unwrap();
        let pkg_requests =
            Package::from_name_version_durl("requests", "2.25.1", None).unwrap();

        let o1 = Origin::new(Ecosystem::Python, PathShared::from(&exe1));
        let o2 = Origin::new(Ecosystem::Python, PathShared::from(&exe2));
        let s1 = Site::python(PathShared::from(&site1));
        let s2 = Site::python(PathShared::from(&site2));

        let mut origin_to_sites = HashMap::new();
        origin_to_sites.insert(o1.clone(), vec![s1.clone()]);
        // provide sites out of order to test sorting
        origin_to_sites.insert(o2.clone(), vec![s2.clone(), s1.clone()]);

        let mut package_to_sites = HashMap::new();
        package_to_sites.insert(pkg_numpy.clone(), vec![s2.clone(), s1.clone()]);
        package_to_sites.insert(pkg_pandas.clone(), vec![s2.clone()]);
        package_to_sites.insert(pkg_flask.clone(), vec![s1.clone()]);
        package_to_sites.insert(pkg_requests.clone(), vec![s2.clone()]);

        let sfs = ScanFS::from_parts(
            origin_to_sites,
            package_to_sites,
            ScanConfig::new(false, false),
            "hash".to_string(),
        );

        let json = serde_json::to_string(&sfs).unwrap();
        let expected_json = r#"[["/opt/venv/bin/python","/usr/bin/python3","/opt/venv/lib/python3.9/site-packages","/usr/lib/python3/site-packages"],[["python",0],["python",1]],[["python","installed",2],["python","installed",3]],[[0,[0,1]],[1,[1]]],[[{"ecosystem":"python","name":"flask","key":"flask","version":"2.0.1","direct_url":null},[1]],[{"ecosystem":"python","name":"numpy","key":"numpy","version":"1.21.0","direct_url":null},[0,1]],[{"ecosystem":"python","name":"pandas","key":"pandas","version":"1.3.0","direct_url":null},[0]],[{"ecosystem":"python","name":"requests","key":"requests","version":"2.25.1","direct_url":null},[0]]],false,false,"hash"]"#;
        assert_eq!(json, expected_json);

        let sfsd: ScanFS = serde_json::from_str(&json).unwrap();
        assert_eq!(sfsd, sfs);
        // Check deserialized sizes
        assert_eq!(sfsd.origin_to_sites.len(), 2);
        assert_eq!(sfsd.package_to_sites.len(), 4);
        assert_eq!(sfsd.site_to_origins.len(), 2);

        // Check that numpy maps to both sites, sorted by path
        let numpy_sites = sfsd.package_to_sites.get(&pkg_numpy).unwrap();
        assert_eq!(numpy_sites, &vec![s2.clone(), s1.clone()]);

        // Check site_to_origins mapping, sorted by path
        assert_eq!(
            sfsd.site_to_origins.get(&s1),
            Some(&vec![o2.clone(), o1.clone()])
        );
        assert_eq!(sfsd.site_to_origins.get(&s2), Some(&vec![o2.clone()]));
    }

    #[test]
    fn test_to_hash_a() {
        let exe = PathBuf::from("/usr/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages.clone()).unwrap();
        assert_eq!(
            sfs.origins_hash,
            "c0b5205c57aa54df7dcbddb79399a81accfe1198a5a7842cb99ac806fb1524a7"
        );
    }

    #[test]
    fn test_to_hash_b() {
        let exe = PathBuf::from("/usr/local/bin/python3");
        let site = PathBuf::from("/usr/lib/python3/site-packages");
        let packages = vec![
            Package::from_name_version_durl("numpy", "1.19.3", None).unwrap(),
            Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap(),
            Package::from_name_version_durl("flask", "1.1.3", None).unwrap(),
        ];
        let sfs = ScanFS::from_exe_site_packages(exe, site, packages.clone()).unwrap();
        assert_eq!(
            sfs.origins_hash,
            "2a8da48d3b320314e74fa1b06b1b1d03330f4fa801b03ac81e57409726169a94"
        );
    }

    #[test]
    fn test_site_install_a() {
        let site_shared1 = Site::python(PathShared::from("foo"));
        let site_shared2 = Site::python(PathShared::from("bar"));
        let exe1 = PathBuf::from("a");
        let exe2 = PathBuf::from("b");

        let p1 = Package::from_name_version_durl("numpy", "1.19.3", None).unwrap();
        let p2 = Package::from_name_version_durl("static-frame", "2.13.0", None).unwrap();
        let p3 = Package::from_name_version_durl("flask", "1.1.3", None).unwrap();

        let mut origin_to_sites = HashMap::new();
        origin_to_sites.insert(
            Origin::new(Ecosystem::Python, exe1.clone().into()),
            vec![site_shared1.clone()],
        );
        origin_to_sites.insert(
            Origin::new(Ecosystem::Python, exe2.clone().into()),
            vec![site_shared2.clone()],
        );

        let exes = vec![exe1.clone(), exe2.clone()];

        let mut package_to_sites = HashMap::new();
        package_to_sites.insert(p1, vec![site_shared1.clone()]);
        package_to_sites.insert(p2, vec![site_shared1.clone()]);
        package_to_sites.insert(p3, vec![site_shared1.clone(), site_shared2.clone()]);

        let config = ScanConfig::new(false, false);
        let origins_hash = hash_paths(&exes, &config);
        let sfs =
            ScanFS::from_parts(origin_to_sites, package_to_sites, config, origins_hash);

        let bound = PathBuf::from("foo");
        let bound_options = None;
        let vf = ValidationFlags {
            permit_superset: false,
            permit_subset: false,
        };
        let ignore = vec![];
        // ensure this retruns an error when multiple exe are defined
        assert!(sfs
            .site_validate_install(
                &bound,
                &bound_options,
                &ignore,
                &vf,
                None,
                FlagLog(false)
            )
            .is_err());
    }

    #[test]
    fn test_from_exes_a() {
        let exe1 = PathBuf::from("a");
        let exe2 = PathBuf::from("b");
        let exes = vec![exe1, exe2];
        let config = ScanConfig::new(false, false);
        let post = ScanFS::from_exes(&exes, &config, FlagLog(false));
        // error for bad exe
        assert!(post.is_err());
    }

    #[test]
    fn test_from_exes_equality_comparison() {
        // Use a bare interpreter name so exe_path_normalize resolves it to a real,
        // existing path on every platform (a hardcoded /usr/bin/python3 does not
        // exist on Windows).
        let name = crate::util::default_python_exe_names()[0];
        let exe1 = PathBuf::from(name);
        let exe2 = PathBuf::from(name);

        let exes = vec![exe1.clone(), exe2.clone()];

        let config = ScanConfig::new(false, false);
        let scan1 = ScanFS::from_exes(&exes, &config, FlagLog(false))
            .expect("Failed to build ScanFS from the first ordering");
        let scan2 = ScanFS::from_exes(&exes, &config, FlagLog(false))
            .expect("Failed to build ScanFS from the shuffled executables");

        let json1 = serde_json::to_string(&scan1).expect("Failed to serialize scan1");
        let json2 = serde_json::to_string(&scan2).expect("Failed to serialize scan2");

        assert_eq!(
            json1, json2,
            "Non-deterministic serialization! ScanFS outputs differ between runs.
             json1: {}
             json2: {}",
            json1, json2
        );
    }
}
