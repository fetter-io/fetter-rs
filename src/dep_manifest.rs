use std::borrow::Borrow;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::fs;
use std::fs::File;
use std::io;
use std::io::BufRead;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use tempfile::tempdir;

use crate::table::ColumnFormat;
use crate::table::Rowable;
use crate::table::RowableContext;
use crate::table::Tableable;
use crate::ureq_client::UreqClient;

use crate::dep_spec::DepOperator;
use crate::dep_spec::DepSpec;
use crate::env_marker::EnvMarkerState;
use crate::lock_file::LockFile;
use crate::package::Package;
use crate::pyproject::PyProjectInfo;
use crate::ureq_client::UreqClientLive;
use crate::util::path_normalize;
use crate::util::Anchor;
use crate::util::ResultDynError;

//------------------------------------------------------------------------------
static LOCK_PRIORITY: &[&str] = &[
    "uv.lock",
    "poetry.lock",
    "Pipfile.lock",
    "requirements.lock",
    "pixi.lock",
    "requirements.txt",
    "pyproject.toml",
];

//------------------------------------------------------------------------------
pub struct DepManifestRecord {
    pub dep_spec: DepSpec,
}

impl Rowable for DepManifestRecord {
    fn to_rows(&self, _context: &RowableContext) -> Vec<Vec<String>> {
        vec![vec![self.dep_spec.to_string()]]
    }
}

// Simple report around dep manifest for common display/output needs
pub struct DepManifestReport {
    pub records: Vec<DepManifestRecord>,
}

impl Tableable<DepManifestRecord> for DepManifestReport {
    fn get_header(&self) -> Vec<ColumnFormat> {
        vec![ColumnFormat::new(
            "# via fetter".to_string(),
            false,
            "#666666".to_string(),
        )]
    }
    fn get_records(&self) -> &Vec<DepManifestRecord> {
        &self.records
    }
}

//------------------------------------------------------------------------------

/// DepSpecOneOrMany
#[derive(Debug, Clone)]
enum DepSpecOOM {
    One(DepSpec),
    Many(Vec<DepSpec>),
}

impl DepSpecOOM {
    /// Derivces a new enum of Many if necessary and inserts a new DepSpec
    fn into_many(self, dep: DepSpec) -> Self {
        match self {
            Self::One(existing) => Self::Many(vec![existing, dep]),
            Self::Many(mut vec) => {
                vec.push(dep);
                Self::Many(vec)
            }
        }
    }
}

//------------------------------------------------------------------------------
// A DepManifest is a requirements listing, implemented as HashMap for quick lookup by package name.
#[derive(Debug, Clone)]
pub struct DepManifest {
    dep_specs: HashMap<String, DepSpecOOM>,
    pub(crate) env_marker_active: bool,
}

impl DepManifest {
    //--------------------------------------------------------------------------
    // constructors from internal structs

    /// Core constructor that all constructors must delegate to.
    pub fn try_from_iter<I, S>(ds_iter: I) -> ResultDynError<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut env_marker_active = false;
        let mut dep_specs: HashMap<String, DepSpecOOM> = HashMap::new();

        for line in ds_iter {
            let spec = line.as_ref().trim();
            if spec.is_empty() {
                continue;
            }
            let dep_spec = DepSpec::from_string(spec)?;
            env_marker_active |= !dep_spec.env_marker.is_empty();

            if let Some(dsoom) = dep_specs.remove(&dep_spec.key) {
                // remove old One dep spec and upgrade it to a Many
                dep_specs.insert(dep_spec.key.clone(), dsoom.into_many(dep_spec));
            } else {
                dep_specs.insert(dep_spec.key.clone(), DepSpecOOM::One(dep_spec));
            }
        }
        Ok(DepManifest {
            dep_specs,
            env_marker_active,
        })
    }

    /// Create DepManifest from a Vec of DepSpec; used for testing.
    pub(crate) fn from_dep_specs(dep_specs: &Vec<DepSpec>) -> ResultDynError<Self> {
        let mut env_marker_active = false;
        let mut ds: HashMap<String, DepSpecOOM> = HashMap::new();

        for dep_spec in dep_specs {
            env_marker_active |= !dep_spec.env_marker.is_empty();

            if let Some(dsoom) = ds.remove(&dep_spec.key) {
                // remove old OOM and upgrade it to a Many
                ds.insert(dep_spec.key.clone(), dsoom.into_many(dep_spec.clone()));
            } else {
                ds.insert(dep_spec.key.clone(), DepSpecOOM::One(dep_spec.clone()));
            }
        }
        Ok(DepManifest {
            dep_specs: ds,
            env_marker_active,
        })
    }

    // Given an iter of Packages, derive a DepManifest
    pub fn from_packages<I>(packages: I, anchor: Anchor) -> ResultDynError<Self>
    where
        I: IntoIterator,
        I::Item: Borrow<Package>,
    {
        let mut package_name_to_package: HashMap<String, Vec<Package>> = HashMap::new();

        for package in packages {
            let pkg: &Package = package.borrow();
            package_name_to_package
                .entry(pkg.name.clone())
                .or_default()
                .push(pkg.clone());
        }
        let names: Vec<String> = package_name_to_package.keys().cloned().collect();
        let mut dep_specs: Vec<DepSpec> = Vec::new();

        for name in names {
            let packages = match package_name_to_package.get_mut(&name) {
                Some(packages) => packages,
                None => continue,
            };
            packages.sort();

            let pkg_min = match packages.first() {
                Some(pkg) => pkg,
                None => continue,
            };
            let pkg_max = match packages.last() {
                Some(pkg) => pkg,
                None => continue,
            };

            let ds = match anchor {
                Anchor::Lower => {
                    DepSpec::from_package(pkg_min, DepOperator::GreaterThanOrEq)
                }
                Anchor::Upper => {
                    DepSpec::from_package(pkg_max, DepOperator::LessThanOrEq)
                }
                Anchor::Both => return Err("Not implemented".into()),
            };
            if let Ok(dep_spec) = ds {
                dep_specs.push(dep_spec);
            }
        }
        DepManifest::from_dep_specs(&dep_specs)
    }

    //--------------------------------------------------------------------------

    // Create a DepManifest from a requirements.txt file, which might reference other requirements.txt files.
    pub(crate) fn from_requirements_file(file_path: &Path) -> ResultDynError<Self> {
        let mut files: VecDeque<PathBuf> = VecDeque::new();
        files.push_back(file_path.to_path_buf());
        let mut dep_specs: Vec<String> = Vec::new();

        while !files.is_empty() {
            let fp = files.pop_front().unwrap();
            let file = File::open(&fp)
                .map_err(|e| format!("Failed to open file: {fp:?} {e}"))?;
            let lines = io::BufReader::new(file).lines();
            for line in lines.map_while(Result::ok) {
                let t = line.trim();
                if t.is_empty() || t.starts_with('#') {
                    continue;
                }
                if let Some(post) = t.strip_prefix("-r ") {
                    if let Some(parent) = fp.parent() {
                        files.push_back(parent.join(post.trim()));
                    }
                } else if let Some(post) = t.strip_prefix("--requirement ") {
                    if let Some(parent) = fp.parent() {
                        files.push_back(parent.join(post.trim()));
                    }
                } else {
                    dep_specs.push(line);
                }
            }
        }
        Self::try_from_iter(dep_specs.iter())
    }

    pub(crate) fn from_pyproject(
        content: &str,
        options: Option<&Vec<String>>,
    ) -> ResultDynError<Self> {
        let ppi = PyProjectInfo::new(content)?;
        Self::try_from_iter(ppi.get_dependencies(options)?.iter())
    }

    pub(crate) fn from_pyproject_file(
        file_path: &PathBuf,
        bound_options: Option<&Vec<String>>,
    ) -> ResultDynError<Self> {
        let content = fs::read_to_string(file_path)
            .map_err(|e| format!("Failed to read file: {e}"))?;
        Self::from_pyproject(&content, bound_options)
    }

    // Create a DepManifest from a URL point to a requirements.txt or pyproject.toml file.
    pub fn from_url<U: UreqClient>(
        client: &U,
        url: &Path,
        bound_options: Option<&Vec<String>>,
    ) -> ResultDynError<Self> {
        let url_str = url.to_str().ok_or("Invalid URL")?;
        let content = client.get(url_str)?;
        if url_str.ends_with("pyproject.toml") {
            Self::from_pyproject(&content, bound_options)
        } else {
            // handle any lock file format, or requirements.txt
            let lf = LockFile::new(content);
            Self::try_from_iter(lf.get_dependencies(bound_options)?)
        }
    }

    //--------------------------------------------------------------------------
    pub(crate) fn from_path(
        file_path: &Path,
        bound_options: Option<&Vec<String>>,
    ) -> ResultDynError<Self> {
        let fp = path_normalize(file_path, true)?;
        match fp.to_str() {
            Some(s) if s.ends_with("pyproject.toml") => {
                Self::from_pyproject_file(&fp, bound_options)
            }
            Some(s) if s.ends_with("requirements.txt") => {
                Self::from_requirements_file(&fp)
            }
            Some(_) => {
                let content = fs::read_to_string(fp)
                    .map_err(|e| format!("Failed to read file: {e}"))?;
                // handle uv.lock, poetry.lock, requirements.lock, Pipfile.lock, or a requirements.txt format (via uv or pip-compile)
                let lf = LockFile::new(content);
                Self::try_from_iter(lf.get_dependencies(bound_options)?)
            }
            None => Err("Path contains invalid UTF-8".into()),
        }
    }

    /// Given a directory, load the first canddiate file based on LOCK_PRIORITY.
    pub(crate) fn from_dir(
        dir: &Path,
        bound_options: Option<&Vec<String>>,
    ) -> ResultDynError<Self> {
        match LOCK_PRIORITY
            .iter()
            .map(|file| dir.join(file))
            .find(|path| path.exists())
        {
            Some(file_path) => Self::from_path(&file_path, bound_options),
            None => {
                Err("Cannot find lock file, requirements file, or pyproject.toml".into())
            }
        }
    }

    pub fn from_git_repo(
        url: &Path,
        bound_options: Option<&Vec<String>>,
    ) -> ResultDynError<Self> {
        let tmp_dir = tempdir()
            .map_err(|e| format!("Failed to create temporary directory: {e}"))?;
        let repo_path = tmp_dir.path().join("repo");

        let status = Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                url.to_str().unwrap(),
                repo_path.to_str().unwrap(),
            ])
            .status()
            .map_err(|e| format!("Failed to execute git: {e}"))?;

        if !status.success() {
            return Err(format!("Git clone failed: {}", url.display()).into());
        }
        Self::from_dir(&repo_path, bound_options)
    }

    pub(crate) fn from_path_or_url(
        file_path: &Path,
        bound_options: Option<&Vec<String>>,
    ) -> ResultDynError<Self> {
        println!("reading: {:?}", file_path);
        match file_path.to_str() {
            Some(s) if s.ends_with(".git") => {
                Self::from_git_repo(file_path, bound_options)
            }
            Some(s) if s.starts_with("http") => {
                Self::from_url(&UreqClientLive, file_path, bound_options)
            }
            Some(_) => Self::from_path(file_path, bound_options),
            None => Err("Path contains invalid UTF-8".into()),
        }
    }

    //--------------------------------------------------------------------------
    /// Iterator that yields all DepSpecs, flattening the OneOrMany enum
    pub(crate) fn iter_dep_specs(&self) -> impl Iterator<Item = &DepSpec> {
        self.dep_specs.values().flat_map(|dsoom| match dsoom {
            DepSpecOOM::One(ds) => vec![ds].into_iter(),
            DepSpecOOM::Many(ds_vec) => ds_vec.iter().collect::<Vec<_>>().into_iter(),
        })
    }

    fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.dep_specs.keys().cloned().collect();
        keys.sort_by_key(|name| name.to_lowercase());
        keys
    }

    // fn has_key(&self, key: &str) -> bool {
    //     self.dep_specs.get(key).is_some()
    // }

    pub(crate) fn has_package(&self, package: &Package) -> bool {
        self.dep_specs.contains_key(&package.key)
    }

    // Return an optional iterator of DepSpecs for the provided key. It is expected there will only be more than one when env markers are used.
    pub(crate) fn get_dep_specs(
        &self,
        key: &str,
    ) -> Option<std::slice::Iter<'_, DepSpec>> {
        self.dep_specs.get(key).map(|dsoom| match dsoom {
            DepSpecOOM::One(ds) => std::slice::from_ref(ds).iter(),
            DepSpecOOM::Many(ds_vec) => ds_vec.iter(),
        })
    }

    // Return all DepSpec in this DepManifest that are not in observed.
    pub(crate) fn get_dep_spec_difference(
        &self,
        observed: &HashSet<&String>,
    ) -> Vec<&String> {
        // iterating over keys, collect those that are not in observed
        let mut dep_specs: Vec<&String> = self
            .dep_specs
            .keys()
            .filter(|key| !observed.contains(key))
            .collect();
        dep_specs.sort();
        dep_specs
    }

    //--------------------------------------------------------------------------
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.dep_specs.len()
    }

    // Given a Package, return true or false if it is valid. This is the main public interface for validation.
    // If we return a DS, it means that have found a DS for this package (which may or may not be validated.
    pub(crate) fn validate(
        &self,
        package: &Package,
        permit_superset: bool,
        env_marker_state: Option<&EnvMarkerState>,
    ) -> (bool, Option<&DepSpec>) {
        // if we have DS for this package
        if let Some(dsoom) = self.dep_specs.get(&package.key) {
            match dsoom {
                DepSpecOOM::One(ds) => {
                    if ds.env_marker.is_empty() {
                        (ds.validate_package(package), Some(ds))
                    } else {
                        // DS has env marker
                        let ems = env_marker_state.expect("EMS should be loaded");
                        if ds.validate_env_marker(ems) {
                            (ds.validate_package(package), Some(ds))
                        } else {
                            // if DS not relevant for this environment, do not return it
                            (permit_superset, None)
                        }
                    }
                }
                DepSpecOOM::Many(dsv) => {
                    // Given many for the same package, take the first that matches this environment.
                    if dsv.iter().any(|ds| !ds.env_marker.is_empty()) {
                        // if any has env_marker
                        let ems = env_marker_state.expect("EMS should be loaded");
                        for ds in dsv {
                            if ds.validate_env_marker(ems) {
                                return (ds.validate_package(package), Some(ds));
                            }
                        }
                    } else {
                        // if multiple DS for package but no env markers: find one that passes
                        // this could be an invalid state
                        for ds in dsv {
                            if ds.validate_package(package) {
                                return (true, Some(ds));
                            }
                        }
                    }
                    // no DS match for this environment
                    (permit_superset, None)
                }
            }
        } else {
            // no DS for this package; if permit_superset, we deem this as valid; no DS to return
            (permit_superset, None)
        }
    }

    //--------------------------------------------------------------------------

    pub fn to_dep_manifest_report(&self) -> DepManifestReport {
        let mut records = Vec::new();
        for key in self.keys() {
            if let Some(dsoom) = self.dep_specs.get(&key) {
                match dsoom {
                    DepSpecOOM::One(ds) => {
                        records.push(DepManifestRecord {
                            dep_spec: ds.clone(),
                        });
                    }
                    DepSpecOOM::Many(dsv) => {
                        for ds in dsv {
                            records.push(DepManifestRecord {
                                dep_spec: ds.clone(),
                            });
                        }
                    }
                };
            }
        }
        DepManifestReport { records }
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::package_durl::DirectURL;
    use crate::ureq_client::UreqClientMock;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_dep_spec_a() {
        let dm =
            DepManifest::try_from_iter(["pk1>=0.2,<0.3", "pk2>=1,<3"].iter()).unwrap();

        let p1 = Package::from_dist_info("pk2-2.0.dist-info", None, None).unwrap();
        assert!(dm.validate(&p1, false, None).0);

        let p2 = Package::from_dist_info("foo-2.0.dist-info", None, None).unwrap();
        assert!(!dm.validate(&p2, false, None).0);

        let p3 = Package::from_dist_info("pk1-0.2.5.dist-info", None, None).unwrap();
        assert!(dm.validate(&p3, false, None).0);

        let p3 = Package::from_dist_info("pk1-0.3.0.dist-info", None, None).unwrap();
        assert!(!dm.validate(&p3, false, None).0);
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_from_dep_specs_a() {
        let ds = vec![
            DepSpec::from_string("numpy==1.19.1").unwrap(),
            DepSpec::from_string("requests>=1.4").unwrap(),
        ];
        let dm = DepManifest::from_dep_specs(&ds).unwrap();
        assert_eq!(dm.len(), 2);
    }
    #[test]
    fn test_from_requirements_a() {
        // Create a temporary directory and file
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("requirements.txt");

        // Write test content to the temp file
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "# comment").unwrap();
        writeln!(file, "pk1>=0.2,  <0.3    ").unwrap();
        writeln!(file).unwrap();
        writeln!(file, "pk2>=1,<3").unwrap();
        writeln!(file, "# ").unwrap();

        let dep_manifest = DepManifest::from_requirements_file(&file_path).unwrap();
        assert_eq!(dep_manifest.len(), 2);

        let p1 = Package::from_name_version_durl("pk2", "2.1", None).unwrap();
        assert!(dep_manifest.validate(&p1, false, None).0);
        let p2 = Package::from_name_version_durl("pk2", "0.1", None).unwrap();
        assert!(!dep_manifest.validate(&p2, false, None).0);
        let p3 = Package::from_name_version_durl("pk1", "0.2.2.999", None).unwrap();
        assert!(dep_manifest.validate(&p3, false, None).0);

        let p4 = Package::from_name_version_durl("pk99", "0.2.2.999", None).unwrap();
        assert!(!dep_manifest.validate(&p4, false, None).0);
    }

    #[test]
    fn test_from_requirements_b() {
        let content = r#"
termcolor==2.2.0
    # via
    #   invsys (pyproject.toml)
    #   apache-airflow
terminado==0.18.1
    # via notebook
testpath==0.6.0
    # via nbconvert
text-unidecode==1.3
    # via python-slugify
threadpoolctl==3.4.0
    # via scikit-learn
toml==0.10.2
    # via
    #   coverage
    #   pre-commit
tomlkit==0.12.4
    # via pylint
"#;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("requirements.txt");
        let mut file = File::create(&file_path).unwrap();
        write!(file, "{}", content).unwrap();

        let dm1 = DepManifest::from_requirements_file(&file_path).unwrap();
        assert_eq!(dm1.len(), 7);
        let p1 = Package::from_name_version_durl("termcolor", "2.2.0", None).unwrap();
        assert!(dm1.validate(&p1, false, None).0);
        let p2 = Package::from_name_version_durl("termcolor", "2.2.1", None).unwrap();
        assert!(!dm1.validate(&p2, false, None).0);
        let p3 = Package::from_name_version_durl("text-unicide", "1.3", None).unwrap();
        assert!(!dm1.validate(&p3, false, None).0);
        let p3 = Package::from_name_version_durl("text-unidecode", "1.3", None).unwrap();
        assert!(dm1.validate(&p3, false, None).0);
    }

    #[test]
    fn test_from_requirements_c() {
        let content = r#"
opentelemetry-api==1.24.0
    # via
    #   apache-airflow
    #   opentelemetry-exporter-otlp-proto-grpc
    #   opentelemetry-exporter-otlp-proto-http
    #   opentelemetry-sdk
opentelemetry-exporter-otlp==1.24.0
    # via apache-airflow
opentelemetry-exporter-otlp-proto-common==1.24.0
    # via
    #   opentelemetry-exporter-otlp-proto-grpc
    #   opentelemetry-exporter-otlp-proto-http
opentelemetry-exporter-otlp-proto-grpc==1.24.0
    # via opentelemetry-exporter-otlp
opentelemetry-exporter-otlp-proto-http==1.24.0
    # via opentelemetry-exporter-otlp
opentelemetry-proto==1.24.0
    # via
    #   opentelemetry-exporter-otlp-proto-common
    #   opentelemetry-exporter-otlp-proto-grpc
    #   opentelemetry-exporter-otlp-proto-http
opentelemetry-sdk==1.24.0
    # via
    #   opentelemetry-exporter-otlp-proto-grpc
    #   opentelemetry-exporter-otlp-proto-http
opentelemetry-semantic-conventions==0.45b0
    # via opentelemetry-sdk
"#;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("requirements.txt");
        let mut file = File::create(&file_path).unwrap();
        write!(file, "{}", content).unwrap();

        let dm1 = DepManifest::from_requirements_file(&file_path).unwrap();
        assert_eq!(dm1.len(), 8);
        let p1 = Package::from_name_version_durl(
            "opentelemetry-exporter-otlp-proto-grpc",
            "1.24.0",
            None,
        )
        .unwrap();
        assert!(dm1.validate(&p1, false, None).0);
        let p2 = Package::from_name_version_durl(
            "opentelemetry-exporter-otlp-proto-grpc",
            "1.24.1",
            None,
        )
        .unwrap();
        assert!(!dm1.validate(&p2, false, None).0);
        let p3 = Package::from_name_version_durl(
            "opentelemetry-exporter-otlp-proto-gpc",
            "1.24.0",
            None,
        )
        .unwrap();
        assert!(!dm1.validate(&p3, false, None).0);
    }

    #[test]
    fn test_from_requirements_d1() {
        let content = r#"
python-slugify==8.0.4
    # via
    #   apache-airflow
    #   python-nvd3
pytz==2023.3
pytzdata==2020.1
    # via pendulum
pyyaml==6.0
pyzmq==26.0.0
readme-renderer==43.0
    # via twine
redshift-connector==2.1.1
    # via apache-airflow-providers-amazon
referencing==0.34.0
    # via
    #   jsonschema
    #   jsonschema-specifications
regex==2024.4.16
"#;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("requirements.txt");
        let mut file = File::create(&file_path).unwrap();
        write!(file, "{}", content).unwrap();

        let dm1 = DepManifest::from_requirements_file(&file_path).unwrap();
        assert_eq!(dm1.len(), 9);
        let p1 = Package::from_name_version_durl("regex", "2024.4.16", None).unwrap();
        assert!(dm1.validate(&p1, false, None).0);
        let p2 = Package::from_name_version_durl("regex", "2024.04.16", None).unwrap();
        assert!(dm1.validate(&p2, false, None).0);
        let p2 = Package::from_name_version_durl("regex", "2024.04.17", None).unwrap();
        assert!(!dm1.validate(&p2, false, None).0);
    }

    #[test]
    fn test_from_requirements_d2() {
        let content = r#"
python-slugify==8.0.4
pytzdata==2020.1    # foo
pyzmq==26.0.0
readme-renderer==43.0      # bar
https://files.pythonhosted.org/packages/01/bb/7f594a891b8e2d9f26e07fa79bd63bb9e426c8dddf0dedf5429f008cdcda/arraykit-1.2.0-cp313-cp313-musllinux_1_2_x86_64.whl # BAZ
"#;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("requirements.txt");
        let mut file = File::create(&file_path).unwrap();
        write!(file, "{}", content).unwrap();

        let dm1 = DepManifest::from_requirements_file(&file_path).unwrap();
        let names: Vec<String> = dm1.keys().to_vec();
        assert_eq!(
            names,
            vec![
                "arraykit",
                "python_slugify",
                "pytzdata",
                "pyzmq",
                "readme_renderer"
            ]
        )
    }

    #[test]
    fn test_from_requirements_e() {
        let content1 = r#"
python-slugify==8.0.4
pytz==2023.3
pytzdata==2020.1
pyyaml==6.0
pyzmq==26.0.0
"#;
        let dir = tempdir().unwrap();
        let fp1 = dir.path().join("requirements-a.txt");
        let mut f1 = File::create(&fp1).unwrap();
        write!(f1, "{}", content1).unwrap();

        let content2 = r#"
readme-renderer==43.0
redshift-connector==2.1.1
referencing==0.34.0
regex==2024.4.16
-r requirements-a.txt
"#;
        let fp2 = dir.path().join("requirements-b.txt");
        let mut f2 = File::create(&fp2).unwrap();
        write!(f2, "{}", content2).unwrap();

        let dm1 = DepManifest::from_requirements_file(&fp2).unwrap();
        assert_eq!(dm1.len(), 9);
    }

    #[test]
    fn test_from_requirements_f() {
        let content1 = r#"
python-slugify==8.0.4
pytz==2023.3
pytzdata==2020.1
pyyaml==6.0
pyzmq==26.0.0
"#;
        let dir = tempdir().unwrap();
        let fp1 = dir.path().join("requirements-a.txt");
        let mut f1 = File::create(&fp1).unwrap();
        write!(f1, "{}", content1).unwrap();

        let content2 = r#"
readme-renderer==43.0
redshift-connector==2.1.1
--requirement  requirements-a.txt
"#;
        let fp2 = dir.path().join("requirements-b.txt");
        let mut f2 = File::create(&fp2).unwrap();
        write!(f2, "{}", content2).unwrap();

        let content3 = r#"
referencing==0.34.0
regex==2024.4.16
--requirement    requirements-b.txt
"#;
        let fp3 = dir.path().join("requirements-c.txt");
        let mut f3 = File::create(&fp3).unwrap();
        write!(f3, "{}", content3).unwrap();

        let dm1 = DepManifest::from_requirements_file(&fp3).unwrap();
        assert_eq!(dm1.len(), 9);
    }

    //--------------------------------------------------------------------------

    #[test]
    fn test_from_pyproject_a() {
        let content = r#"
[project]
name = "example_package_YOUR_USERNAME_HERE"
version = "0.0.1"
authors = [
  { name="Example Author", email="author@example.com" },
]
description = "A small example package"
readme = "README.md"
requires-python = ">=3.8"
classifiers = [
    "Programming Language :: Python :: 3",
    "License :: OSI Approved :: MIT License",
    "Operating System :: OS Independent",
]
dependencies = [
  "httpx",
  "gidgethub[httpx]>4.0.0",
  "django>2.1; os_name != 'nt'",
]
"#;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("pyproject.toml");
        let mut file = File::create(&file_path).unwrap();
        write!(file, "{}", content).unwrap();

        let dm = DepManifest::from_pyproject_file(&file_path, None).unwrap();
        assert_eq!(dm.keys(), vec!["django", "gidgethub", "httpx"])
    }

    #[test]
    fn test_from_pyproject_b1() {
        let content = r#"
[project]
name = "example_package_YOUR_USERNAME_HERE"
version = "0.0.1"
authors = [
  { name="Example Author", email="author@example.com" },
]
description = "A small example package"
readme = "README.md"
requires-python = ">=3.8"
classifiers = [
    "Programming Language :: Python :: 3",
    "License :: OSI Approved :: MIT License",
    "Operating System :: OS Independent",
]
dependencies = [
  "httpx",
  "gidgethub[httpx]>4.0.0",
  "django>2.1; os_name != 'nt'",
]
[project.optional-dependencies]
gui = ["PyQt5"]
cli = [
  "rich",
  "click",
]
"#;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("pyproject.toml");
        let mut file = File::create(&file_path).unwrap();
        write!(file, "{}", content).unwrap();

        let bound_options = vec!["cli".to_string()];
        let dm1 =
            DepManifest::from_pyproject_file(&file_path, Some(&bound_options)).unwrap();
        assert_eq!(
            dm1.keys(),
            vec!["click", "django", "gidgethub", "httpx", "rich"]
        );

        let bound_options = vec!["cli".to_string(), "gui".to_string()];
        let dm2 =
            DepManifest::from_pyproject_file(&file_path, Some(&bound_options)).unwrap();
        assert_eq!(
            dm2.keys(),
            vec!["click", "django", "gidgethub", "httpx", "pyqt5", "rich"]
        );

        let bound_options = vec!["gui".to_string()];
        let dm3 =
            DepManifest::from_pyproject_file(&file_path, Some(&bound_options)).unwrap();
        assert_eq!(dm3.keys(), vec!["django", "gidgethub", "httpx", "pyqt5"]);
    }

    #[test]
    fn test_from_pyproject_b2() {
        let content = r#"
[project]
name = "example_package_YOUR_USERNAME_HERE"
version = "0.0.1"
authors = [
  { name="Example Author", email="author@example.com" },
]
description = "A small example package"
readme = "README.md"
requires-python = ">=3.8"
classifiers = [
    "Programming Language :: Python :: 3",
    "License :: OSI Approved :: MIT License",
    "Operating System :: OS Independent",
]
dependencies = [
  "httpx",
  "gidgethub[httpx]>4.0.0",
  "django>2.1; os_name != 'nt'",
]
[project.optional-dependencies]
gui = ["PyQt5"]
cli = [
  "rich",
  "click",
]
"#;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("pyproject.toml");
        let mut file = File::create(&file_path).unwrap();
        write!(file, "{}", content).unwrap();

        let bo = vec!["cli".to_string()];
        let dm1 = DepManifest::from_pyproject_file(&file_path, Some(&bo)).unwrap();
        assert_eq!(
            dm1.keys(),
            vec!["click", "django", "gidgethub", "httpx", "rich"]
        );

        let bo1 = vec!["cli".to_string(), "gu".to_string()];
        assert!(DepManifest::from_pyproject_file(&file_path, Some(&bo1)).is_err());

        let bo2 = vec!["cli".to_string(), "gui".to_string()];
        assert!(DepManifest::from_pyproject_file(&file_path, Some(&bo2)).is_ok());

        let bo3 = vec!["cli".to_string(), "gui".to_string(), "foo".to_string()];
        assert!(DepManifest::from_pyproject_file(&file_path, Some(&bo3)).is_err());
    }

    #[test]
    fn test_from_pyproject_c1() {
        let content = r#"
[tool.poetry]
name = "poetry"
readme = "README.md"

[tool.poetry.urls]
Changelog = "https://python-poetry.org/history/"

[tool.poetry.dependencies]
python = "==3.9"

poetry-core = { git = "https://github.com/python-poetry/poetry-core.git", branch = "main" }
build = "==1.2.1"
cachecontrol = { version = "==0.14.0", extras = ["filecache"] }
cleo = "==2.1.0"
dulwich = "==0.22.1"
fastjsonschema = "==2.18.0"
importlib-metadata = { version = ">=4.4", python = "<3.10" }
installer = "==0.7.0"
keyring = "==25.1.0"
# packaging uses calver, so version is unclamped
packaging = ">=24.0"
pkginfo = "==1.10"
platformdirs = ">=3.0.0,<5"
pyproject-hooks = "==1.0.0"
requests = "==2.26"
requests-toolbelt = "==1.0.0"
shellingham = "==1.5"
tomli = { version = "==2.0.1", python = "<3.11" }
tomlkit = ">=0.11.4,<1.0.0"
# trove-classifiers uses calver, so version is unclamped
trove-classifiers = ">=2022.5.19"
virtualenv = "==20.26.6"
xattr = { version = "==1.0.0", markers = "sys_platform == 'darwin'" }

[tool.poetry.group.dev.dependencies]
pre-commit = ">=2.10"
setuptools = { version = ">=60", python = "<3.10" }

[tool.poetry.group.test.dependencies]
coverage = ">=7.2.0"
deepdiff = ">=6.3"
httpretty = ">=1.1"
jaraco-classes = ">=3.3.1"
pytest = ">=8.0"
pytest-cov = ">=4.0"
pytest-mock = ">=3.9"
pytest-randomly = ">=3.12"
pytest-xdist = { version = ">=3.1", extras = ["psutil"] }

[tool.poetry.group.typing.dependencies]
mypy = ">=1.8.0"
types-requests = ">=2.28.8"

# only used in github actions
[tool.poetry.group.github-actions]
optional = true
[tool.poetry.group.github-actions.dependencies]
pytest-github-actions-annotate-failures = "==0.1.7"
    "#;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("pyproject.toml");
        let mut file = File::create(&file_path).unwrap();
        write!(file, "{}", content).unwrap();

        let dm = DepManifest::from_pyproject_file(&file_path, None).unwrap();
        assert_eq!(
            dm.keys(),
            vec![
                "build",
                "cachecontrol",
                "cleo",
                "dulwich",
                "fastjsonschema",
                "importlib_metadata",
                "installer",
                "keyring",
                "packaging",
                "pkginfo",
                "platformdirs",
                "poetry_core",
                "pyproject_hooks",
                "requests",
                "requests_toolbelt",
                "shellingham",
                "tomli",
                "tomlkit",
                "trove_classifiers",
                "virtualenv",
                "xattr"
            ]
        );

        assert_eq!(
            dm.get_dep_specs("cachecontrol")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "cachecontrol==0.14.0; python_version == '3.9'"
        );
        assert_eq!(
            dm.get_dep_specs("dulwich")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "dulwich==0.22.1; python_version == '3.9'"
        );
        assert_eq!(
            dm.get_dep_specs("tomli")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "tomli==2.0.1; python_version == '3.9'"
        );
        assert_eq!(
            dm.get_dep_specs("platformdirs")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "platformdirs>=3.0.0,<5; python_version == '3.9'"
        );
        assert_eq!(
            dm.get_dep_specs("importlib_metadata")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "importlib-metadata>=4.4; python_version == '3.9'"
        );
    }

    #[test]
    fn test_from_pyproject_c2() {
        let content = r#"
[tool.poetry]
name = "poetry"
include = [{ path = "tests", format = "sdist" }]
homepage = "https://python-poetry.org/"

[tool.poetry.urls]
Changelog = "https://python-poetry.org/history/"

[tool.poetry.dependencies]
python = "==3.9"

poetry-core = { git = "https://github.com/python-poetry/poetry-core.git", branch = "main" }
build = "==1.2.1"
cachecontrol = { version = "==0.14.0", extras = ["filecache"] }
cleo = "==2.1.0"
importlib-metadata = { version = ">=4.4", python = "<3.10" }
installer = "==0.7.0"
keyring = "==25.1.0"
trove-classifiers = ">=2022.5.19"
xattr = { version = "==1.0.0", markers = "sys_platform == 'darwin'" }

[tool.poetry.group.dev.dependencies]
pre-commit = ">=2.10"
setuptools = { version = ">=60", python = "<3.10" }

[tool.poetry.group.test.dependencies]
coverage = ">=7.2.0"
deepdiff = ">=6.3"
httpretty = ">=1.1"
jaraco-classes = ">=3.3.1"
pytest = ">=8.0"
pytest-cov = ">=4.0"
pytest-xdist = { version = ">=3.1", extras = ["psutil"] }

[tool.poetry.group.typing.dependencies]
mypy = ">=1.8.0"
types-requests = ">=2.28.8"

# only used in github actions
[tool.poetry.group.github-actions]
optional = true
[tool.poetry.group.github-actions.dependencies]
pytest-github-actions-annotate-failures = "==0.1.7"
    "#;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("pyproject.toml");
        let mut file = File::create(&file_path).unwrap();
        write!(file, "{}", content).unwrap();

        let dm1 = DepManifest::from_pyproject_file(&file_path, None).unwrap();
        assert_eq!(
            dm1.keys(),
            vec![
                "build",
                "cachecontrol",
                "cleo",
                "importlib_metadata",
                "installer",
                "keyring",
                "poetry_core",
                "trove_classifiers",
                "xattr"
            ]
        );

        let opts2 = vec!["test".to_string()];
        let dm2 = DepManifest::from_pyproject_file(&file_path, Some(&opts2)).unwrap();
        assert_eq!(
            dm2.keys(),
            vec![
                "build",
                "cachecontrol",
                "cleo",
                "coverage",
                "deepdiff",
                "httpretty",
                "importlib_metadata",
                "installer",
                "jaraco_classes",
                "keyring",
                "poetry_core",
                "pytest",
                "pytest_cov",
                "pytest_xdist",
                "trove_classifiers",
                "xattr"
            ]
        );
        assert_eq!(
            dm2.get_dep_specs("coverage")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "coverage>=7.2.0"
        );

        let opts3 = vec!["typing".to_string()];
        let dm3 = DepManifest::from_pyproject_file(&file_path, Some(&opts3)).unwrap();
        assert_eq!(
            dm3.keys(),
            vec![
                "build",
                "cachecontrol",
                "cleo",
                "importlib_metadata",
                "installer",
                "keyring",
                "mypy",
                "poetry_core",
                "trove_classifiers",
                "types_requests",
                "xattr"
            ]
        );
        assert_eq!(
            dm3.get_dep_specs("mypy")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "mypy>=1.8.0"
        );
        let opts4 = vec!["typing".to_string(), "test".to_string()];
        let dm4 = DepManifest::from_pyproject_file(&file_path, Some(&opts4)).unwrap();
        assert_eq!(
            dm4.keys(),
            vec![
                "build",
                "cachecontrol",
                "cleo",
                "coverage",
                "deepdiff",
                "httpretty",
                "importlib_metadata",
                "installer",
                "jaraco_classes",
                "keyring",
                "mypy",
                "poetry_core",
                "pytest",
                "pytest_cov",
                "pytest_xdist",
                "trove_classifiers",
                "types_requests",
                "xattr"
            ]
        );
        let opts5 = vec!["typing".to_string(), "test".to_string(), "foo".to_string()];
        assert!(DepManifest::from_pyproject_file(&file_path, Some(&opts5)).is_err());
    }

    #[test]
    fn test_from_pyproject_d1() {
        let content = r#"
    [tool.poetry]
    name = "poetry"
    include = [{ path = "tests", format = "sdist" }]

    [tool.poetry.dependencies]
    python = "^3.9"

    poetry-core = { git = "https://github.com/python-poetry/poetry-core.git", branch = "main" }
    build = "^1.2.1"
    cachecontrol = { version = "^0.14.0", extras = ["filecache"] }
    cleo = "^2.1.0"
    dulwich = "^0.22.1"
    fastjsonschema = "^2.18.0"
    importlib-metadata = { version = ">=4.4", python = "<3.10" }
    installer = "^0.7.0"
    keyring = "^25.1.0"
    # packaging uses calver, so version is unclamped
    packaging = ">=24.0"
    pkginfo = "^1.10"
    platformdirs = ">=3.0.0,<5"
    pyproject-hooks = "^1.0.0"
    requests = "^2.26"
    requests-toolbelt = "^1.0.0"
    shellingham = "^1.5"
    tomli = { version = "^2.0.1", python = "<3.11" }
    tomlkit = ">=0.11.4,<1.0.0"
    # trove-classifiers uses calver, so version is unclamped
    trove-classifiers = ">=2022.5.19"
    virtualenv = "^20.26.6"
    xattr = { version = "^1.0.0", markers = "sys_platform == 'darwin'" }

    [tool.poetry.group.dev.dependencies]
    pre-commit = ">=2.10"
    setuptools = { version = ">=60", python = "<3.10" }

    [tool.poetry.group.test.dependencies]
    coverage = ">=7.2.0"
    deepdiff = ">=6.3"
    httpretty = ">=1.1"
    jaraco-classes = ">=3.3.1"
    pytest = ">=8.0"
    pytest-cov = ">=4.0"
    pytest-mock = ">=3.9"
    pytest-randomly = ">=3.12"
    pytest-xdist = { version = ">=3.1", extras = ["psutil"] }

    [tool.poetry.group.typing.dependencies]
    mypy = ">=1.8.0"
    types-requests = ">=2.28.8"

    # only used in github actions
    [tool.poetry.group.github-actions]
    optional = true
    [tool.poetry.group.github-actions.dependencies]
    pytest-github-actions-annotate-failures = "^0.1.7"
        "#;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("pyproject.toml");
        let mut file = File::create(&file_path).unwrap();
        write!(file, "{}", content).unwrap();

        let dm = DepManifest::from_pyproject_file(&file_path, None).unwrap();
        assert_eq!(
            dm.keys(),
            vec![
                "build",
                "cachecontrol",
                "cleo",
                "dulwich",
                "fastjsonschema",
                "importlib_metadata",
                "installer",
                "keyring",
                "packaging",
                "pkginfo",
                "platformdirs",
                "poetry_core",
                "pyproject_hooks",
                "requests",
                "requests_toolbelt",
                "shellingham",
                "tomli",
                "tomlkit",
                "trove_classifiers",
                "virtualenv",
                "xattr"
            ]
        );
        assert_eq!(
            dm.get_dep_specs("cachecontrol")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "cachecontrol^0.14.0; python_version ^ '3.9'"
        );
        assert_eq!(
            dm.get_dep_specs("platformdirs")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "platformdirs>=3.0.0,<5; python_version ^ '3.9'"
        );
        assert_eq!(
            dm.get_dep_specs("build")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "build^1.2.1; python_version ^ '3.9'"
        );
        assert_eq!(
            dm.get_dep_specs("dulwich")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "dulwich^0.22.1; python_version ^ '3.9'"
        );
        assert_eq!(
            dm.get_dep_specs("platformdirs")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "platformdirs>=3.0.0,<5; python_version ^ '3.9'"
        );
    }

    #[test]
    fn test_from_pyproject_d2() {
        let content = r#"
    [tool.poetry]
    name = "poetry"
    include = [{ path = "tests", format = "sdist" }]

    [tool.poetry.dependencies]
    python = "^3.9"

    poetry-core = { git = "https://github.com/python-poetry/poetry-core.git", branch = "main" }
    build = "^1.2.1"
    cachecontrol = { version = "^0.14.0", extras = ["filecache"] }
    cleo = "^2.1.0"
    dulwich = "^0.22.1"
    fastjsonschema = "^2.18.0"
    importlib-metadata = { version = ">=4.4", python = "<3.10" }
    installer = "^0.7.0"
    keyring = "^25.1.0"
    # packaging uses calver, so version is unclamped
    packaging = ">=24.0"
    pkginfo = "^1.10"
    platformdirs = ">=3.0.0,<5"
    pyproject-hooks = "^1.0.0"
    requests = "^2.26"
    requests-toolbelt = "^1.0.0"
    shellingham = "^1.5"
    tomli = { version = "^2.0.1", python = "<3.11" }
    tomlkit = ">=0.11.4,<1.0.0"
    # trove-classifiers uses calver, so version is unclamped
    trove-classifiers = ">=2022.5.19"
    virtualenv = "^20.26.6"
    xattr = { version = "^1.0.0", markers = "sys_platform == 'darwin'" }

    [tool.poetry.group.dev.dependencies]
    pre-commit = ">=2.10"
    setuptools = { version = ">=60", python = "<3.10" }

    [tool.poetry.group.typing.dependencies]
    mypy = ">=1.8.0"
    types-requests = ">=2.28.8"

    # only used in github actions
    [tool.poetry.group.github-actions]
    optional = true
    [tool.poetry.group.github-actions.dependencies]
    pytest-github-actions-annotate-failures = "^0.1.7"
        "#;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("pyproject.toml");
        let mut file = File::create(&file_path).unwrap();
        write!(file, "{}", content).unwrap();

        let bo = vec!["github-actions".to_string()];
        let dm = DepManifest::from_pyproject_file(&file_path, Some(&bo)).unwrap();
        assert_eq!(
            dm.keys(),
            vec![
                "build",
                "cachecontrol",
                "cleo",
                "dulwich",
                "fastjsonschema",
                "importlib_metadata",
                "installer",
                "keyring",
                "packaging",
                "pkginfo",
                "platformdirs",
                "poetry_core",
                "pyproject_hooks",
                "pytest_github_actions_annotate_failures",
                "requests",
                "requests_toolbelt",
                "shellingham",
                "tomli",
                "tomlkit",
                "trove_classifiers",
                "virtualenv",
                "xattr"
            ]
        );
        assert_eq!(
            dm.get_dep_specs("cachecontrol")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "cachecontrol^0.14.0; python_version ^ '3.9'"
        );
        assert_eq!(
            dm.get_dep_specs("platformdirs")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "platformdirs>=3.0.0,<5; python_version ^ '3.9'"
        );
        assert_eq!(
            dm.get_dep_specs("build")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "build^1.2.1; python_version ^ '3.9'"
        );
        assert_eq!(
            dm.get_dep_specs("dulwich")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "dulwich^0.22.1; python_version ^ '3.9'"
        );
        assert_eq!(
            dm.get_dep_specs("platformdirs")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "platformdirs>=3.0.0,<5; python_version ^ '3.9'"
        );
        assert_eq!(
            dm.get_dep_specs("pytest_github_actions_annotate_failures")
                .unwrap()
                .next()
                .unwrap()
                .to_string(),
            "pytest-github-actions-annotate-failures^0.1.7"
        );
    }

    #[test]
    fn test_from_pyproject_e1() {
        let content = r#"
[project]
name = "example_package_YOUR_USERNAME_HERE"
version = "0.0.1"
description = "A small example package"
dependencies = [
  "httpx",
  "gidgethub[httpx]>4.0.0",
  "django>2.1; os_name != 'nt'",
]

[tool.poetry.group.dev.dependencies]
pre-commit = ">=2.10"
setuptools = { version = ">=60", python = "<3.10" }

"#;

        let bo = vec!["dev".to_string()];
        let dm1 = DepManifest::from_pyproject(content, Some(&bo)).unwrap();
        assert_eq!(
            dm1.keys(),
            vec!["django", "gidgethub", "httpx", "pre_commit", "setuptools"]
        );
        let dm2 = DepManifest::from_pyproject(content, None).unwrap();
        assert_eq!(dm2.keys(), vec!["django", "gidgethub", "httpx"]);
    }

    //--------------------------------------------------------------------------

    #[test]
    fn test_from_url_a() {
        let mock_get = r#"
dill>=0.3.9
six>=1.15.0
numpy>= 2.0
        "#;

        let client = UreqClientMock {
            mock_post: None,
            mock_get: Some(mock_get.to_string()),
        };

        let url = PathBuf::from("http://example.com/requirements.txt");
        let dm = DepManifest::from_url(&client, &url, None).unwrap();
        assert_eq!(dm.keys(), vec!["dill", "numpy", "six"])
    }

    //--------------------------------------------------------------------------

    #[test]
    fn test_to_requirements_a() {
        let ds = vec![
            DepSpec::from_string("numpy==1.19.1").unwrap(),
            DepSpec::from_string("requests>=1.4").unwrap(),
            DepSpec::from_string("static-frame>2.0,!=1.3").unwrap(),
        ];
        let dm1 = DepManifest::from_dep_specs(&ds).unwrap();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("requirements.txt");
        let dmr1 = dm1.to_dep_manifest_report();
        dmr1.to_file(&file_path, ' ').unwrap();

        let dm2 = DepManifest::from_requirements_file(&file_path).unwrap();
        assert_eq!(dm2.len(), 3)
    }

    //--------------------------------------------------------------------------

    #[test]
    fn test_get_dep_spec_a() {
        let ds = vec![
            DepSpec::from_string("numpy==1.19.1").unwrap(),
            DepSpec::from_string("requests>=1.4").unwrap(),
            DepSpec::from_string("static-frame>2.0,!=1.3").unwrap(),
        ];
        let dm1 = DepManifest::from_dep_specs(&ds).unwrap();
        let ds1 = dm1.get_dep_specs("requests").unwrap().next().unwrap();
        assert_eq!(format!("{}", ds1), "requests>=1.4");
    }

    #[test]
    fn test_get_dep_spec_b() {
        let ds = vec![
            DepSpec::from_string("numpy==1.19.1").unwrap(),
            DepSpec::from_string("requests>=1.4").unwrap(),
            DepSpec::from_string("static-frame>2.0,!=1.3").unwrap(),
        ];
        let dm1 = DepManifest::from_dep_specs(&ds).unwrap();
        assert!(dm1.get_dep_specs("foo").is_none());
    }

    #[test]
    fn test_get_dep_spec_c() {
        let ds = vec![
            DepSpec::from_string("numpy==1.19.1").unwrap(),
            DepSpec::from_string("Cython==3.0.11").unwrap(),
        ];
        let dm1 = DepManifest::from_dep_specs(&ds).unwrap();
        let ds1 = dm1.get_dep_specs("cython").unwrap().next().unwrap();
        assert_eq!(format!("{}", ds1), "Cython==3.0.11");
    }

    //--------------------------------------------------------------------------

    #[test]
    fn test_get_dep_spec_difference_a() {
        let ds = vec![
            DepSpec::from_string("numpy==1.19.1").unwrap(),
            DepSpec::from_string("requests>=1.4").unwrap(),
            DepSpec::from_string("static-frame>2.0,!=1.3").unwrap(),
        ];
        let dm1 = DepManifest::from_dep_specs(&ds).unwrap();
        let mut observed = HashSet::new();
        let n1 = "static_frame".to_string();
        observed.insert(&n1);

        let post = dm1.get_dep_spec_difference(&observed);

        assert_eq!(post, vec!["numpy", "requests"]);
    }

    //--------------------------------------------------------------------------

    #[test]
    fn test_validate_a() {
        // if we install as "packaging @ git+https://github.com/pypa/packaging.git@cf2cbe2aec28f87c6228a6fb136c27931c9af407"
        // in site packages we get packaging-24.2.dev0.dist-info
        // and writes this in direct_url.json
        // {"url": "https://github.com/pypa/packaging.git", "vcs_info": {"commit_id": "cf2cbe2aec28f87c6228a6fb136c27931c9af407", "revision": "cf2cbe2aec28f87c6228a6fb136c27931c9af407", "vcs": "git"}}

        let json_str = r#"
        {"url": "https://github.com/pypa/packaging.git", "vcs_info": {"commit_id": "cf2cbe2aec28f87c6228a6fb136c27931c9af407", "revision": "cf2cbe2aec28f87c6228a6fb136c27931c9af407", "vcs": "git"}}
        "#;
        let durl: DirectURL = serde_json::from_str(json_str).unwrap();
        let p1 =
            Package::from_dist_info("packaging-24.2.dev0.dist-info", None, Some(durl))
                .unwrap();

        let ds1 = DepSpec::from_string("packaging @ git+https://github.com/pypa/packaging.git@cf2cbe2aec28f87c6228a6fb136c27931c9af407").unwrap();
        let specs = vec![
            DepSpec::from_string("numpy==1.19.1").unwrap(),
            DepSpec::from_string("requests>=1.4").unwrap(),
            ds1,
        ];
        let dm1 = DepManifest::from_dep_specs(&specs).unwrap();
        // ds1 has no version information, while p1 does: meaning version passes
        // ds1 has url of git+https://github.com/pypa/packaging.git@cf2cbe2aec28f87c6228a6fb136c27931c9af407
        // DirectURL: git+https://github.com/pypa/packaging.git@cf2cbe2aec28f87c6228a6fb136c27931c9af407
        assert!(dm1.validate(&p1, false, None).0);
    }

    #[test]
    fn test_validate_b() {
        // if we install as "packaging @ git+https://foo@github.com/pypa/packaging.git@cf2cbe2aec28f87c6228a6fb136c27931c9af407"
        // in site packages we get packaging-24.2.dev0.dist-info
        // and writes this in direct_url.json, without the user part of the url
        // {"url": "https://github.com/pypa/packaging.git", "vcs_info": {"commit_id": "cf2cbe2aec28f87c6228a6fb136c27931c9af407", "revision": "cf2cbe2aec28f87c6228a6fb136c27931c9af407", "vcs": "git"}}

        let json_str = r#"
        {"url": "https://github.com/pypa/packaging.git", "vcs_info": {"commit_id": "cf2cbe2aec28f87c6228a6fb136c27931c9af407", "revision": "cf2cbe2aec28f87c6228a6fb136c27931c9af407", "vcs": "git"}}
        "#;
        let durl: DirectURL = serde_json::from_str(json_str).unwrap();
        let p1 =
            Package::from_dist_info("packaging-24.2.dev0.dist-info", None, Some(durl))
                .unwrap();

        let ds1 = DepSpec::from_string("packaging @ git+https://foo@github.com/pypa/packaging.git@cf2cbe2aec28f87c6228a6fb136c27931c9af407").unwrap();
        let specs = vec![
            DepSpec::from_string("numpy==1.19.1").unwrap(),
            DepSpec::from_string("requests>=1.4").unwrap(),
            ds1,
        ];
        let dm1 = DepManifest::from_dep_specs(&specs).unwrap();
        assert!(!dm1.env_marker_active);
        assert!(dm1.validate(&p1, false, None).0);
    }

    //--------------------------------------------------------------------------

    #[test]
    fn test_from_dir_a() {
        let content = r#"
[project]
name = "example_package_YOUR_USERNAME_HERE"
version = "0.0.1"
description = "A small example package"
requires-python = ">=3.8"
dependencies = [
  "httpx",
  "gidgethub[httpx]>4.0.0",
  "django>2.1; os_name != 'nt'",
]
"#;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("pyproject.toml");
        let mut file = File::create(&file_path).unwrap();
        write!(file, "{}", content).unwrap();
        let dm = DepManifest::from_dir(dir.path(), None).unwrap();
        assert!(dm.env_marker_active);
        assert_eq!(dm.keys(), vec!["django", "gidgethub", "httpx"]);
    }

    #[test]
    fn test_from_dir_b() {
        let dir = tempdir().unwrap();

        let content1 = r#"
[project]
name = "example_package_YOUR_USERNAME_HERE"
version = "0.0.1"
description = "A small example package"
requires-python = ">=3.8"
dependencies = [
  "httpx",
  "gidgethub[httpx]>4.0.0",
  "django>2.1; os_name != 'nt'",
]
"#;
        let fp1 = dir.path().join("pyproject.toml");
        let mut file1 = File::create(&fp1).unwrap();
        write!(file1, "{}", content1).unwrap();

        let content2 = r#"
python-slugify==8.0.4
    # via
    #   apache-airflow
    #   python-nvd3
pytz==2023.3
pytzdata==2020.1
    # via pendulum
pyyaml==6.0
pyzmq==26.0.0
        "#;

        let fp2 = dir.path().join("requirements.txt");
        let mut file2 = File::create(&fp2).unwrap();
        write!(file2, "{}", content2).unwrap();

        let dm = DepManifest::from_dir(dir.path(), None);
        assert_eq!(
            dm.unwrap().keys(),
            vec!["python_slugify", "pytz", "pytzdata", "pyyaml", "pyzmq"]
        );
    }

    #[test]
    fn test_from_dir_c() {
        let dir = tempdir().unwrap();

        let content1 = r#"
[project]
name = "example_package_YOUR_USERNAME_HERE"
version = "0.0.1"
description = "A small example package"
requires-python = ">=3.8"
dependencies = [
  "httpx",
  "gidgethub[httpx]>4.0.0",
  "django>2.1; os_name != 'nt'",
]
"#;
        let fp1 = dir.path().join("pyproject.toml");
        let mut file1 = File::create(&fp1).unwrap();
        write!(file1, "{}", content1).unwrap();

        let content2 = r#"
python-slugify==8.0.4
    # via
    #   apache-airflow
    #   python-nvd3
pytz==2023.3
pytzdata==2020.1
    # via pendulum
pyyaml==6.0
pyzmq==26.0.0
        "#;

        let fp2 = dir.path().join("requirements.lock");
        let mut file2 = File::create(&fp2).unwrap();
        write!(file2, "{}", content2).unwrap();

        let dm = DepManifest::from_dir(dir.path(), None);
        assert_eq!(
            dm.unwrap().keys(),
            vec!["python_slugify", "pytz", "pytzdata", "pyyaml", "pyzmq"]
        );
    }

    #[test]
    fn test_from_dir_d() {
        let dir = tempdir().unwrap();

        let content1 = r#"
[project]
name = "example_package_YOUR_USERNAME_HERE"
version = "0.0.1"
description = "A small example package"
requires-python = ">=3.8"
dependencies = [
  "httpx",
  "gidgethub[httpx]>4.0.0",
  "django>2.1; os_name != 'nt'",
]
"#;
        let fp1 = dir.path().join("pyproject.toml");
        let mut file1 = File::create(&fp1).unwrap();
        write!(file1, "{}", content1).unwrap();

        let content2 = r#"
python-slugify==8.0.4
    # via
    #   apache-airflow
    #   python-nvd3
pytz==2023.3
pytzdata==2020.1
    # via pendulum
pyyaml==6.0
pyzmq==26.0.0
        "#;

        let fp2 = dir.path().join("requirements.txt");
        let mut file2 = File::create(&fp2).unwrap();
        write!(file2, "{}", content2).unwrap();

        let content3 = r#"
version = 1
requires-python = ">=3.12"

[[distribution]]
name = "arraykit"
version = "0.10.0"
source = { registry = "https://pypi.org/simple" }
dependencies = [
    { name = "numpy" },
]
sdist = { url = "https://files.pythonhosted.org/packages/d0/35/d1d6cc29d930eff913e49fe0081149f5cb630a630cf35b329d811dc390e2/arraykit-0.10.0.tar.gz", hash = "sha256:ee890b71c6e60505a9a77ad653ecb9c879e0f1a887980359d7fbaf29d33d5446", size = 83187 }


[[distribution]]
name = "arraymap"
version = "0.4.0"
source = { registry = "https://pypi.org/simple" }
dependencies = [
    { name = "numpy" },
]
sdist = { url = "https://files.pythonhosted.org/packages/6c/89/1d8b77225282b1a37029755ff53f63b1566bab8da1ac0e88f2fb8187c490/arraymap-0.4.0.tar.gz", hash = "sha256:af1aa15f9f0c799888326561275052b4ea709b0a3a2ff58d01c55a447f8b1213", size = 24770 }
        "#;

        let fp3 = dir.path().join("uv.lock");
        let mut file3 = File::create(&fp3).unwrap();
        write!(file3, "{}", content3).unwrap();

        let dm = DepManifest::from_dir(dir.path(), None);
        assert_eq!(dm.unwrap().keys(), vec!["arraykit", "arraymap"]);
    }

    #[test]
    fn test_from_dir_e() {
        let dir = tempdir().unwrap();

        let content1 = r#"
[project]
name = "example_package_YOUR_USERNAME_HERE"
version = "0.0.1"
description = "A small example package"
requires-python = ">=3.8"
dependencies = [
  "httpx",
  "gidgethub[httpx]>4.0.0",
  "django>2.1; os_name != 'nt'",
]
"#;
        let fp1 = dir.path().join("pyproject.toml");
        let mut file1 = File::create(&fp1).unwrap();
        write!(file1, "{}", content1).unwrap();

        let content2 = r#"
python-slugify==8.0.4
    # via
    #   apache-airflow
    #   python-nvd3
pytz==2023.3
pytzdata==2020.1
    # via pendulum
pyyaml==6.0
pyzmq==26.0.0
        "#;

        let fp2 = dir.path().join("requirements.txt");
        let mut file2 = File::create(&fp2).unwrap();
        write!(file2, "{}", content2).unwrap();

        let content3 = r#"
# This file is automatically @generated by Poetry 2.0.1 and should not be changed by hand.

[[package]]
name = "certifi"
version = "2025.1.31"
description = "Python package for providing Mozilla's CA Bundle."
optional = false
python-versions = ">=3.6"
groups = ["main"]
files = [
    {file = "certifi-2025.1.31-py3-none-any.whl", hash = "sha256:ca78db4565a652026a4db2bcdf68f2fb589ea80d0be70e03929ed730746b84fe"},
    {file = "certifi-2025.1.31.tar.gz", hash = "sha256:3d5da6925056f6f18f119200434a4780a94263f10d1c21d032a6f6b2baa20651"},
]

[[package]]
name = "charset-normalizer"
version = "3.4.1"
description = "The Real First Universal Charset Detector. Open, modern and actively maintained alternative to Chardet."
optional = false
python-versions = ">=3.7"
groups = ["main"]
        "#;

        let fp3 = dir.path().join("poetry.lock");
        let mut file3 = File::create(&fp3).unwrap();
        write!(file3, "{}", content3).unwrap();

        let dm = DepManifest::from_dir(dir.path(), None);
        assert_eq!(dm.unwrap().keys(), vec!["certifi", "charset_normalizer"]);
    }
}
