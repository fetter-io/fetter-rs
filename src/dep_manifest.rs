use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::io::BufRead;
use std::io::Write;
use std::path::PathBuf;

use crate::dep_spec::DepSpec;
use crate::package::Package;

// A DepManifest is essential a requirements file, implemented as HashMap for quick lookup by package name.
#[derive(Debug)]
pub(crate) struct DepManifest {
    dep_specs: HashMap<String, DepSpec>,
}

impl DepManifest {
    pub fn from_iter<I, S>(ds_iter: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut dep_specs = HashMap::new();
        for spec in ds_iter {
            let dep_spec = DepSpec::from_string(spec.as_ref())?;
            if dep_specs.contains_key(&dep_spec.name) {
                return Err(format!("Duplicate package name found: {}", dep_spec.name));
            }
            dep_specs.insert(dep_spec.name.clone(), dep_spec);
        }
        Ok(DepManifest { dep_specs })
    }
    pub fn from_requirements(file_path: &PathBuf) -> Result<Self, String> {
        let file = File::open(file_path).map_err(|e| format!("Failed to open file: {}", e))?;
        let lines = io::BufReader::new(file).lines();
        let filtered_lines = lines.filter_map(|line| {
            match line {
                Ok(s) => {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with('#') {
                        Some(s) // yield untrimmed string for
                    } else {
                        None
                    }
                }
                Err(_) => None, // Ignore lines that failed to read
            }
        });
        DepManifest::from_iter(filtered_lines)
    }
    pub fn from_dep_specs(dep_specs: &Vec<DepSpec>) -> Result<Self, String> {
        let mut ds: HashMap<String, DepSpec> = HashMap::new();
        for dep_spec in dep_specs {
            if ds.contains_key(&dep_spec.name) {
                return Err(format!("Duplicate DepSpec name found: {}", dep_spec.name));
            }
            ds.insert(dep_spec.name.clone(), dep_spec.clone());
        }
        Ok(DepManifest { dep_specs: ds })
    }
    // pub fn from_pyproject_toml<P: AsRef<Path>>(file_path: P) -> Result<Self, String> {
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

    // pub fn from_git_repo(repo_url: &str) -> Result<Self, String> {
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
    fn get_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.dep_specs.keys().cloned().collect();
        names.sort_by_key(|name| name.to_lowercase());
        names
    }

    pub fn get_dep_spec(&self, key: &str) -> Option<&DepSpec> {
        self.dep_specs.get(key)
    }

    /// Given a writer, write out all dependency specs
    fn to_writer<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writeln!(writer, "# created by fetter")?;
        for name in self.get_names() {
            writeln!(writer, "{}", self.dep_specs.get(&name).unwrap())?;
        }
        Ok(())
    }

    //--------------------------------------------------------------------------
    // Writes to a file
    pub fn to_requirements(&self, file_path: &PathBuf) -> io::Result<()> {
        let file = File::create(file_path)?;
        self.to_writer(file)
    }

    // Prints to stdout
    pub(crate) fn display(&self) {
        let stdout = io::stdout();
        let handle = stdout.lock();
        self.to_writer(handle).unwrap();
    }

    //--------------------------------------------------------------------------
    pub fn len(&self) -> usize {
        self.dep_specs.len()
    }
    pub fn validate(&self, package: &Package) -> bool {
        if let Some(dep_spec) = self.dep_specs.get(&package.name) {
            dep_spec.validate_version(&package.version)
        } else {
            false
        }
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
        let dm = DepManifest::from_iter(vec!["pk1>=0.2,<0.3", "pk2>=1,<3"].iter()).unwrap();

        let p1 = Package::from_dist_info("pk2-2.0.dist-info").unwrap();
        assert_eq!(dm.validate(&p1), true);

        let p2 = Package::from_dist_info("foo-2.0.dist-info").unwrap();
        assert_eq!(dm.validate(&p2), false);

        let p3 = Package::from_dist_info("pk1-0.2.5.dist-info").unwrap();
        assert_eq!(dm.validate(&p3), true);

        let p3 = Package::from_dist_info("pk1-0.3.0.dist-info").unwrap();
        assert_eq!(dm.validate(&p3), false);
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

        let p1 = Package::from_name_and_version("pk2", "2.1").unwrap();
        assert_eq!(dep_manifest.validate(&p1), true);
        let p2 = Package::from_name_and_version("pk2", "0.1").unwrap();
        assert_eq!(dep_manifest.validate(&p2), false);
        let p3 = Package::from_name_and_version("pk1", "0.2.2.999").unwrap();
        assert_eq!(dep_manifest.validate(&p3), true);

        let p4 = Package::from_name_and_version("pk99", "0.2.2.999").unwrap();
        assert_eq!(dep_manifest.validate(&p4), false);
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
        let p1 = Package::from_name_and_version("termcolor", "2.2.0").unwrap();
        assert_eq!(dm1.validate(&p1), true);
        let p2 = Package::from_name_and_version("termcolor", "2.2.1").unwrap();
        assert_eq!(dm1.validate(&p2), false);
        let p3 = Package::from_name_and_version("text-unicide", "1.3").unwrap();
        assert_eq!(dm1.validate(&p3), false);
        let p3 = Package::from_name_and_version("text-unidecode", "1.3").unwrap();
        assert_eq!(dm1.validate(&p3), true);
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
        let p1 = Package::from_name_and_version("opentelemetry-exporter-otlp-proto-grpc", "1.24.0")
            .unwrap();
        assert_eq!(dm1.validate(&p1), true);
        let p2 = Package::from_name_and_version("opentelemetry-exporter-otlp-proto-grpc", "1.24.1")
            .unwrap();
        assert_eq!(dm1.validate(&p2), false);
        let p3 = Package::from_name_and_version("opentelemetry-exporter-otlp-proto-gpc", "1.24.0")
            .unwrap();
        assert_eq!(dm1.validate(&p3), false);
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
        let p1 = Package::from_name_and_version("regex", "2024.4.16").unwrap();
        assert_eq!(dm1.validate(&p1), true);
        let p2 = Package::from_name_and_version("regex", "2024.04.16").unwrap();
        assert_eq!(dm1.validate(&p2), true);
        let p2 = Package::from_name_and_version("regex", "2024.04.17").unwrap();
        assert_eq!(dm1.validate(&p2), false);
    }

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
}
