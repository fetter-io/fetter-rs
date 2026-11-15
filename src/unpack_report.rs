use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::marker::Send;
use std::path::PathBuf;

use rayon::prelude::*;

use crate::eco::scanner_for;
use crate::eco::Artifacts;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Site;
use crate::origin_site::SiteKind;
use crate::package::Package;
use crate::table::ColumnFormat;
use crate::table::Rowable;
use crate::table::RowableContext;
use crate::table::Tableable;
use crate::util::logger;
use crate::util::FlagLog;

//------------------------------------------------------------------------------
/// Remove all artifacts: existing files first, then directories recursively.
fn remove_artifacts(artifacts: &Artifacts, log: FlagLog) {
    for (fp, exists) in &artifacts.files {
        if *exists {
            if let Err(e) = fs::remove_file(fp) {
                logger!(log, module_path!(), "Failed to remove file {:?}: {}", fp, e);
            } else {
                logger!(log, module_path!(), "Removing file: {:?}", fp);
            }
        }
    }
    for dir in &artifacts.dirs {
        if let Err(e) = fs::remove_dir_all(dir) {
            logger!(
                log,
                module_path!(),
                "Failed to remove directory {:?}: {}",
                dir,
                e
            );
        } else {
            logger!(log, module_path!(), "Removing directory: {:?}", dir);
        }
    }
}

//------------------------------------------------------------------------------
trait UnpackRecordTrait {
    /// Return a new record; caller must clone as needed.
    fn new(package: Package, site: Site, artifacts: Artifacts) -> Self;
}

//------------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub(crate) struct UnpackFullRecord {
    package: Package,
    site: Site,
    artifacts: Artifacts,
}

impl UnpackRecordTrait for UnpackFullRecord {
    fn new(package: Package, site: Site, artifacts: Artifacts) -> Self {
        UnpackFullRecord {
            package,
            site,
            artifacts,
        }
    }
}

impl Rowable for UnpackFullRecord {
    fn to_rows(&self, context: &RowableContext) -> Vec<Vec<String>> {
        let is_tty = *context == RowableContext::Tty;

        let mut package_set = false;
        let mut package_display = || {
            if !is_tty || !package_set {
                package_set = true;
                (self.package.ecosystem.to_string(), self.package.to_string())
            } else {
                ("".to_string(), "".to_string())
            }
        };

        let mut site_set = false;
        let mut site_display = || {
            if !is_tty || !site_set {
                site_set = true;
                self.site.to_string()
            } else {
                "".to_string()
            }
        };

        let mut rows: Vec<Vec<String>> = Vec::new();
        for (fp, exists) in &self.artifacts.files {
            let (eco, package) = package_display();
            rows.push(vec![
                eco,
                package,
                site_display(),
                exists.to_string(),
                fp.display().to_string(),
            ]);
        }

        for dir in &self.artifacts.dirs {
            let (eco, package) = package_display();
            rows.push(vec![
                eco,
                package,
                site_display(),
                true.to_string(),
                dir.display().to_string(),
            ]);
        }

        rows
    }
}
//------------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub(crate) struct UnpackCountRecord {
    package: Package,
    site: Site,
    artifacts: Artifacts,
}

impl UnpackRecordTrait for UnpackCountRecord {
    fn new(package: Package, site: Site, artifacts: Artifacts) -> Self {
        UnpackCountRecord {
            package,
            site,
            artifacts,
        }
    }
}

impl Rowable for UnpackCountRecord {
    fn to_rows(&self, _context: &RowableContext) -> Vec<Vec<String>> {
        vec![vec![
            self.package.ecosystem.to_string(),
            self.package.to_string(),
            self.site.to_string(),
            self.artifacts.files.len().to_string(),
            self.artifacts.dirs.len().to_string(),
        ]]
    }
}

//------------------------------------------------------------------------------
/// Read the artifacts of each package at each of its sites, asking each site's scanner once for all packages wanted there. Records are ordered by package, then site.
fn package_to_sites_to_records<R>(
    package_to_sites: &HashMap<Package, Vec<Site>>,
) -> Vec<R>
where
    R: UnpackRecordTrait + Sync + Send,
{
    let mut site_to_wanted: HashMap<&Site, HashSet<&Package>> = HashMap::new();
    for (package, sites) in package_to_sites {
        for site in sites {
            site_to_wanted.entry(site).or_default().insert(package);
        }
    }
    let mut found: Vec<(Package, Site, Artifacts)> = site_to_wanted
        .into_par_iter()
        .flat_map_iter(|(site, wanted)| {
            scanner_for(site.ecosystem)
                .map(|s| s.site_to_artifacts(site, &wanted))
                .unwrap_or_default()
                .into_iter()
                .map(move |(p, a)| (p, site.clone(), a))
        })
        .collect();
    found.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    found
        .into_iter()
        .map(|(p, site, a)| R::new(p, site, a))
        .collect()
}

/// Per ecosystem, the number of packages found only at lockfile or manifest sites, which have no installed artifacts.
fn count_without_artifacts(
    package_to_sites: &HashMap<Package, Vec<Site>>,
) -> Vec<(Ecosystem, usize)> {
    let mut counts: BTreeMap<Ecosystem, usize> = BTreeMap::new();
    for (package, sites) in package_to_sites {
        if !sites.iter().any(|s| s.kind == SiteKind::Installed) {
            *counts.entry(package.ecosystem).or_default() += 1;
        }
    }
    counts.into_iter().collect()
}

//------------------------------------------------------------------------------
pub(crate) struct UnpackFullReport {
    records: Vec<UnpackFullRecord>,
}

impl Tableable<UnpackFullRecord> for UnpackFullReport {
    fn get_header(&self) -> Vec<ColumnFormat> {
        vec![
            ColumnFormat::new("Ecosystem".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Package".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Site".to_string(), true, "#666666".to_string()),
            ColumnFormat::new("Exists".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Artifacts".to_string(), true, "#666666".to_string()),
        ]
    }
    fn get_records(&self) -> &Vec<UnpackFullRecord> {
        &self.records
    }
}

//------------------------------------------------------------------------------
pub(crate) struct UnpackCountReport {
    records: Vec<UnpackCountRecord>,
}

impl Tableable<UnpackCountRecord> for UnpackCountReport {
    fn get_header(&self) -> Vec<ColumnFormat> {
        vec![
            ColumnFormat::new("Ecosystem".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Package".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Site".to_string(), true, "#666666".to_string()),
            ColumnFormat::new("Files".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Dirs".to_string(), false, "#666666".to_string()),
        ]
    }
    fn get_records(&self) -> &Vec<UnpackCountRecord> {
        &self.records
    }
}

//------------------------------------------------------------------------------
enum UnpackBody {
    Full(UnpackFullReport),
    Count(UnpackCountReport),
}

pub(crate) struct UnpackReport {
    body: UnpackBody,
    /// Per ecosystem, the number of selected packages known only from lockfiles or manifests, which have no installed artifacts and so no records.
    pub(crate) without_artifacts: Vec<(Ecosystem, usize)>,
}

impl UnpackReport {
    pub(crate) fn from_package_to_sites(
        count: bool,
        package_to_sites: &HashMap<Package, Vec<Site>>,
    ) -> Self {
        let body = if count {
            let records = package_to_sites_to_records(package_to_sites);
            UnpackBody::Count(UnpackCountReport { records })
        } else {
            let records = package_to_sites_to_records(package_to_sites);
            UnpackBody::Full(UnpackFullReport { records })
        };
        UnpackReport {
            body,
            without_artifacts: count_without_artifacts(package_to_sites),
        }
    }

    pub(crate) fn to_writer(&self, stderr: bool) -> io::Result<()> {
        match &self.body {
            UnpackBody::Full(report) => report.to_writer(stderr),
            UnpackBody::Count(report) => report.to_writer(stderr),
        }
    }

    pub(crate) fn to_file(&self, file_path: &PathBuf, delimiter: char) -> io::Result<()> {
        match &self.body {
            UnpackBody::Full(report) => report.to_file(file_path, delimiter),
            UnpackBody::Count(report) => report.to_file(file_path, delimiter),
        }
    }

    pub(crate) fn remove(&self, log: FlagLog) -> io::Result<()> {
        match &self.body {
            UnpackBody::Full(report) => {
                report.records.par_iter().for_each(|record| {
                    remove_artifacts(&record.artifacts, log);
                });
            }
            UnpackBody::Count(report) => {
                report.records.par_iter().for_each(|record| {
                    remove_artifacts(&record.artifacts, log);
                });
            }
        }
        Ok(())
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    use crate::path_shared::PathShared;
    use std::path::Path;

    fn write(fp: &Path, content: &str) {
        fs::create_dir_all(fp.parent().unwrap()).unwrap();
        let mut f = File::create(fp).unwrap();
        write!(f, "{}", content).unwrap();
    }

    fn pj(name: &str, version: &str) -> String {
        format!(r#"{{"name": "{name}", "version": "{version}"}}"#)
    }

    /// Map every package of each site to that site, as a ScanFS does.
    fn package_to_sites(sites: &[Site]) -> HashMap<Package, Vec<Site>> {
        let mut out: HashMap<Package, Vec<Site>> = HashMap::new();
        for site in sites {
            for p in scanner_for(site.ecosystem).unwrap().site_to_packages(site) {
                out.entry(p).or_default().push(site.clone());
            }
        }
        out
    }

    fn summary(records: &[UnpackFullRecord]) -> Vec<(String, usize, usize)> {
        records
            .iter()
            .map(|r| {
                (
                    r.package.to_string(),
                    r.artifacts.files.len(),
                    r.artifacts.dirs.len(),
                )
            })
            .collect()
    }

    #[test]
    fn test_npm_artifacts_a() {
        let dir = tempdir().unwrap();
        let nm = dir.path().join("node_modules");
        write(&nm.join("a/package.json"), &pj("a", "1.0.0"));
        write(&nm.join("a/index.js"), "");
        write(&nm.join("a/lib/x.js"), "");
        // nested dependency: its files belong to b, not a
        write(&nm.join("a/node_modules/b/package.json"), &pj("b", "2.0.0"));
        write(&nm.join("a/node_modules/b/b.js"), "");
        // the same b installed again at the top level
        write(&nm.join("b/package.json"), &pj("b", "2.0.0"));
        write(&nm.join("@s/c/package.json"), &pj("@s/c", "3.0.0"));
        write(&nm.join(".bin/tool"), "");

        let site = Site::new(
            Ecosystem::Npm,
            SiteKind::Installed,
            PathShared::from(nm.clone()),
        );
        let p2s = package_to_sites(&[site]);
        let records: Vec<UnpackFullRecord> = package_to_sites_to_records(&p2s);
        assert_eq!(
            summary(&records),
            vec![
                ("@s/c-3.0.0".to_string(), 1, 1),
                ("a-1.0.0".to_string(), 3, 1),
                ("b-2.0.0".to_string(), 3, 2),
            ]
        );
        let a = &records[1].artifacts;
        assert_eq!(a.dirs, vec![nm.join("a")]);
        assert!(a
            .files
            .iter()
            .all(|(fp, exists)| *exists && !fp.starts_with(nm.join("a/node_modules"))));
        assert!(count_without_artifacts(&p2s).is_empty());
    }

    #[test]
    fn test_cargo_and_lockfile_artifacts_a() {
        let dir = tempdir().unwrap();
        let home = dir.path().join("cargo");
        write(
            &home.join(".crates2.json"),
            r#"{"installs": {
                "ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)": {"bins": ["rg"]},
                "tool 1.0.0 (registry+https://github.com/rust-lang/crates.io-index)": {"bins": ["a", "b"]}
            }}"#,
        );
        write(&home.join("bin/rg"), "");
        write(&home.join("bin/a"), "");
        let installed = Site::new(
            Ecosystem::Cargo,
            SiteKind::Installed,
            PathShared::from(home.join(".crates2.json")),
        );
        let project = dir.path().join("proj");
        write(
            &project.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
        );
        let lock = Site::new(
            Ecosystem::Cargo,
            SiteKind::Lockfile,
            PathShared::from(project.join("Cargo.lock")),
        );
        let p2s = package_to_sites(&[installed, lock]);
        let records: Vec<UnpackFullRecord> = package_to_sites_to_records(&p2s);
        assert_eq!(
            summary(&records),
            vec![
                ("ripgrep-14.1.0".to_string(), 1, 0),
                ("tool-1.0.0".to_string(), 2, 0),
            ]
        );
        let exists: Vec<bool> =
            records[1].artifacts.files.iter().map(|(_, e)| *e).collect();
        assert_eq!(exists, vec![true, false]);
        assert_eq!(count_without_artifacts(&p2s), vec![(Ecosystem::Cargo, 1)]);
    }

    #[test]
    fn test_vscode_artifacts_a() {
        let dir = tempdir().unwrap();
        let ext_dir = dir.path().join("extensions");
        let ext = ext_dir.join("pub.ext-1.0.0");
        write(
            &ext.join("package.json"),
            r#"{"publisher": "Pub", "name": "ext", "version": "1.0.0"}"#,
        );
        write(&ext.join("out/main.js"), "");
        write(
            &ext.join("node_modules/dep/package.json"),
            &pj("dep", "0.1.0"),
        );
        let site = Site::new(
            Ecosystem::VsCode,
            SiteKind::Installed,
            PathShared::from(ext_dir),
        );
        let p2s = package_to_sites(&[site]);
        let records: Vec<UnpackFullRecord> = package_to_sites_to_records(&p2s);
        // the bundled node_modules is an npm site of its own
        assert_eq!(summary(&records), vec![("Pub.ext-1.0.0".to_string(), 2, 1)]);
        assert_eq!(records[0].artifacts.dirs, vec![ext]);
    }

    #[test]
    fn test_unpack_report_count_rows_a() {
        let dir = tempdir().unwrap();
        let nm = dir.path().join("node_modules");
        write(&nm.join("a/package.json"), &pj("a", "1.0.0"));
        let site = Site::new(
            Ecosystem::Npm,
            SiteKind::Installed,
            PathShared::from(nm.clone()),
        );
        let p2s = package_to_sites(&[site]);
        let report = UnpackReport::from_package_to_sites(true, &p2s);
        let UnpackBody::Count(r) = &report.body else {
            panic!("expected count report");
        };
        assert_eq!(
            r.records[0].to_rows(&RowableContext::Delimited),
            vec![vec![
                "npm".to_string(),
                "a-1.0.0".to_string(),
                nm.display().to_string(),
                "1".to_string(),
                "1".to_string(),
            ]]
        );
    }
}
