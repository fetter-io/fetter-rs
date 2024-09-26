use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::io::BufRead;
use std::io::Write;
use std::path::PathBuf;

use crate::dep_spec::DepSpec;
use crate::package::Package;

// A DepManifest is a requirements listing, implemented as HashMap for quick lookup by package name.
#[derive(Debug, Clone)]
pub(crate) struct DepManifest {
    dep_specs: HashMap<String, DepSpec>,
}

impl DepManifest {
    #[allow(dead_code)]
    pub(crate) fn from_iter<I, S>(ds_iter: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut dep_specs = HashMap::new();
        for spec in ds_iter {
            let dep_spec = DepSpec::from_string(spec.as_ref())?;
            if dep_specs.contains_key(&dep_spec.key) {
                return Err(format!("Duplicate package key found: {}", dep_spec.key));
            }
            dep_specs.insert(dep_spec.key.clone(), dep_spec);
        }
        Ok(DepManifest { dep_specs })
    }
    // Create a DepManifest from a requirements.txt file, which might reference onther requirements.txt files.
    pub(crate) fn from_requirements(file_path: &PathBuf) -> Result<Self, String> {
        let mut files: VecDeque<PathBuf> = VecDeque::new();
        files.push_back(file_path.clone());
        let mut dep_specs = HashMap::new();

        while files.len() > 0 {
            let fp = files.pop_front().unwrap();
            let file = File::open(&fp)
                .map_err(|e| format!("Failed to open file: {:?} {}", fp, e))?;
            let lines = io::BufReader::new(file).lines();
            for line in lines {
                if let Ok(s) = line {
                    let t = s.trim();
                    if t.is_empty() || t.starts_with('#') {
                        continue;
                    }
                    if t.starts_with("-r ") {
                        files.push_back(file_path.parent().unwrap().join(&t[3..]));
                    } else {
                        let ds = DepSpec::from_string(&s)?;
                        if dep_specs.contains_key(&ds.key) {
                            return Err(format!(
                                "Duplicate package key found: {}",
                                ds.key
                            ));
                        }
                        dep_specs.insert(ds.key.clone(), ds);
                    }
                }
            }
        }
        Ok(DepManifest { dep_specs })
    }
    pub(crate) fn from_dep_specs(dep_specs: &Vec<DepSpec>) -> Result<Self, String> {
        let mut ds: HashMap<String, DepSpec> = HashMap::new();
        for dep_spec in dep_specs {
            if ds.contains_key(&dep_spec.key) {
                return Err(format!("Duplicate DepSpec key found: {}", dep_spec.key));
            }
            ds.insert(dep_spec.key.clone(), dep_spec.clone());
        }
        Ok(DepManifest { dep_specs: ds })
    }
    // pub(crate) fn from_pyproject_toml<P: AsRef<Path>>(file_path: P) -> Result<Self, String> {
    //     let contents = fs::read_to_string(file_path)
    //         .map_err(|e| format!("Failed to read pyproject.toml file: {}", e))?;
    //     let parsed_toml: toml::Value = toml::from_str(&contents)
    //         .map_err(|e| format!("Failed to parse pyproject.toml file: {}", e))?;

    //     let mut packages = HashMap::new();
    //     if let Some(dependencies) = parsed_toml.get("tool").and_then(|t| t.get("poetry")).and_then(|p| p.get("dependencies")) {
    //         for (name, version) in dependencies.as_table().ok_or("Dependencies is not a table")? {
    //             let spec = format!("{} {}", name, version.as_str().unwrap_or(""));
    //             let dep_spec = DepSpec::from_string(&spec)?;
    //             packages.insert(dep_spec.name.clone(), dep_spec);
    //         }
    //     } else {
    //         return Err("No dependencies found in pyproject.toml".to_string());
    //     }
    //     Ok(DepManifest { packages })
    // }

    // pub(crate) fn from_git_repo(repo_url: &str) -> Result<Self, String> {
    //     // Create a temporary directory
    //     let tmp_dir = tempdir().map_err(|e| format!("Failed to create temporary directory: {}", e))?;
    //     let repo_path = tmp_dir.path().join("repo");

    //     // Shell out to git to perform a shallow clone
    //     let status = Command::new("git")
    //         .args(&["clone", "--depth", "1", repo_url, repo_path.to_str().unwrap()])
    //         .status()
    //         .map_err(|e| format!("Failed to execute git: {}", e))?;

    //     if !status.success() {
    //         return Err("Git clone failed".to_string());
    //     }

    //     // Construct the path to the requirements.txt file
    //     let requirements_path = repo_path.join("requirements.txt");

    //     // Load the requirements.txt file into a DepManifest
    //     let manifest = DepManifest::from_requirements_file(&requirements_path)?;

    //     // TempDir will be cleaned up when it goes out of scope
    //     Ok(manifest)
    // }

    //--------------------------------------------------------------------------
    fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.dep_specs.keys().cloned().collect();
        keys.sort_by_key(|name| name.to_lowercase());
        keys
    }

    // Return an optional DepSpec reference.
    pub(crate) fn get_dep_spec(&self, key: &str) -> Option<&DepSpec> {
        self.dep_specs.get(key)
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

    /// Given a writer, write out all dependency specs
    fn to_writer<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writeln!(writer, "# created by fetter")?;
        for key in self.keys() {
            writeln!(writer, "{}", self.dep_specs.get(&key).unwrap())?;
        }
        Ok(())
    }

    //--------------------------------------------------------------------------
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.dep_specs.len()
    }

    pub(crate) fn validate(
        &self,
        package: &Package,
        permit_superset: bool,
    ) -> (bool, Option<&DepSpec>) {
        if let Some(ds) = self.dep_specs.get(&package.key) {
            let valid =
                ds.validate_version(&package.version) && ds.validate_url(&package);
            (valid, Some(ds))
        } else {
            (permit_superset, None) // cannot get a dep spec
        }
    }

    //--------------------------------------------------------------------------
    // Writes to a file
    pub(crate) fn to_requirements(&self, file_path: &PathBuf) -> io::Result<()> {
        let file = File::create(file_path)?;
        self.to_writer(file)
    }

    // Prints to stdout
    pub(crate) fn to_stdout(&self) {
        let stdout = io::stdout();
        let handle = stdout.lock();
        self.to_writer(handle).unwrap();
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_dep_spec_a() {
        let dm =
            DepManifest::from_iter(vec!["pk1>=0.2,<0.3", "pk2>=1,<3"].iter()).unwrap();

        let p1 = Package::from_dist_info("pk2-2.0.dist-info", None).unwrap();
        assert_eq!(dm.validate(&p1, false).0, true);

        let p2 = Package::from_dist_info("foo-2.0.dist-info", None).unwrap();
        assert_eq!(dm.validate(&p2, false).0, false);

        let p3 = Package::from_dist_info("pk1-0.2.5.dist-info", None).unwrap();
        assert_eq!(dm.validate(&p3, false).0, true);

        let p3 = Package::from_dist_info("pk1-0.3.0.dist-info", None).unwrap();
        assert_eq!(dm.validate(&p3, false).0, false);
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
        writeln!(file, "").unwrap();
        writeln!(file, "pk2>=1,<3").unwrap();
        writeln!(file, "# ").unwrap();

        let dep_manifest = DepManifest::from_requirements(&file_path).unwrap();
        assert_eq!(dep_manifest.len(), 2);

        let p1 = Package::from_name_version_durl("pk2", "2.1", None).unwrap();
        assert_eq!(dep_manifest.validate(&p1, false).0, true);
        let p2 = Package::from_name_version_durl("pk2", "0.1", None).unwrap();
        assert_eq!(dep_manifest.validate(&p2, false).0, false);
        let p3 = Package::from_name_version_durl("pk1", "0.2.2.999", None).unwrap();
        assert_eq!(dep_manifest.validate(&p3, false).0, true);

        let p4 = Package::from_name_version_durl("pk99", "0.2.2.999", None).unwrap();
        assert_eq!(dep_manifest.validate(&p4, false).0, false);
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

        let dm1 = DepManifest::from_requirements(&file_path).unwrap();
        assert_eq!(dm1.len(), 7);
        let p1 = Package::from_name_version_durl("termcolor", "2.2.0", None).unwrap();
        assert_eq!(dm1.validate(&p1, false).0, true);
        let p2 = Package::from_name_version_durl("termcolor", "2.2.1", None).unwrap();
        assert_eq!(dm1.validate(&p2, false).0, false);
        let p3 = Package::from_name_version_durl("text-unicide", "1.3", None).unwrap();
        assert_eq!(dm1.validate(&p3, false).0, false);
        let p3 = Package::from_name_version_durl("text-unidecode", "1.3", None).unwrap();
        assert_eq!(dm1.validate(&p3, false).0, true);
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

        let dm1 = DepManifest::from_requirements(&file_path).unwrap();
        assert_eq!(dm1.len(), 8);
        let p1 = Package::from_name_version_durl(
            "opentelemetry-exporter-otlp-proto-grpc",
            "1.24.0",
            None,
        )
        .unwrap();
        assert_eq!(dm1.validate(&p1, false).0, true);
        let p2 = Package::from_name_version_durl(
            "opentelemetry-exporter-otlp-proto-grpc",
            "1.24.1",
            None,
        )
        .unwrap();
        assert_eq!(dm1.validate(&p2, false).0, false);
        let p3 = Package::from_name_version_durl(
            "opentelemetry-exporter-otlp-proto-gpc",
            "1.24.0",
            None,
        )
        .unwrap();
        assert_eq!(dm1.validate(&p3, false).0, false);
    }

    #[test]
    fn test_from_requirements_d() {
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

        let dm1 = DepManifest::from_requirements(&file_path).unwrap();
        assert_eq!(dm1.len(), 9);
        let p1 = Package::from_name_version_durl("regex", "2024.4.16", None).unwrap();
        assert_eq!(dm1.validate(&p1, false).0, true);
        let p2 = Package::from_name_version_durl("regex", "2024.04.16", None).unwrap();
        assert_eq!(dm1.validate(&p2, false).0, true);
        let p2 = Package::from_name_version_durl("regex", "2024.04.17", None).unwrap();
        assert_eq!(dm1.validate(&p2, false).0, false);
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

        let dm1 = DepManifest::from_requirements(&fp2).unwrap();
        assert_eq!(dm1.len(), 9);
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
        dm1.to_requirements(&file_path).unwrap();

        let dm2 = DepManifest::from_requirements(&file_path).unwrap();
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
        let ds1 = dm1.get_dep_spec("requests").unwrap();
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
        assert!(dm1.get_dep_spec("foo").is_none());
    }

    #[test]
    fn test_get_dep_spec_c() {
        let ds = vec![
            DepSpec::from_string("numpy==1.19.1").unwrap(),
            DepSpec::from_string("Cython==3.0.11").unwrap(),
        ];
        let dm1 = DepManifest::from_dep_specs(&ds).unwrap();
        let ds1 = dm1.get_dep_spec("cython").unwrap();
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
}
