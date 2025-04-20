use crate::util::ResultDynError;
use crate::util::{conda_fn_to_name_version, str_to_py_marker, toml_to_py_marker};
use serde_json::Value as JsonValue;
use serde_yaml::Value;
use toml::Value as TomlValue;

#[derive(Debug, PartialEq)]
pub enum LockFileType {
    Requirements, // requirements.txt style, used by uv pip and pip-tools
    UvLock,       // uv.lock native TOML style
    Poetry,       // poetry.lock
    PipfileLock,  // made with Pipenv Pipfile.lock
    PEP751,       // pep still under consideration
    PixiLock,     // pixi.lock (in the process of adding)
}

#[derive(Debug)]
pub struct LockFile {
    pub file_type: LockFileType,
    pub content: String,
}

impl LockFile {
    pub fn new(content: String) -> Self {
        let file_type = Self::detect_type(&content);
        Self { file_type, content }
    }

    fn detect_type(content: &str) -> LockFileType {
        if let Ok(json) = serde_json::from_str::<JsonValue>(content) {
            if json.get("_meta").is_some() && json.get("default").is_some() {
                return LockFileType::PipfileLock;
            }
        }

        let mut count = 0;
        for line in content.lines() {
            let t = line.trim();
            // both uv/requests and toml poetry files use # comments
            if t.is_empty() || t.starts_with('#') {
                continue;
            }

            count += 1;
            if count > 20 {
                break;
            }
            // Poetry TOML format
            if t.starts_with("[metadata]") || t.starts_with("[[package]]") {
                return LockFileType::Poetry;
            }
            // UV TOML format
            if t.starts_with("[[distribution]]") {
                return LockFileType::UvLock;
            }
            // PEP 751 TOML format
            if t.starts_with("[[packages]]") {
                return LockFileType::PEP751;
            }
            // Pixi YAML format
            if t.starts_with("version:") || t.starts_with("environments:") {
                return LockFileType::PixiLock;
            }
        }
        LockFileType::Requirements
    }

    /// Extracts dependencies from a `uv` requirements-style lock file. Unlike with  requirements.txt files, this will not try to load other files
    fn get_uv_requirements_dep(&self) -> ResultDynError<Vec<String>> {
        let dependencies = self
            .content
            .lines()
            .filter_map(|line| {
                let t = line.trim();
                if t.is_empty() || t.starts_with('#') {
                    return None;
                }
                Some(t.to_string())
            })
            .collect();
        Ok(dependencies)
    }

    /// Extracts dependencies from a `uv` native file and formats them as `package==version`.
    fn get_uv_native_dep(&self) -> ResultDynError<Vec<String>> {
        let parsed: TomlValue = self.content.parse()?; // Parse as TOML
        let mut dependencies = Vec::new();

        if let Some(dists) = parsed.get("distribution").and_then(|p| p.as_array()) {
            for d in dists {
                // assume that there is always a version number
                if let (Some(name), Some(version)) = (
                    d.get("name").and_then(|n| n.as_str()),
                    d.get("version").and_then(|v| v.as_str()),
                ) {
                    dependencies.push(format!("{}=={}", name, version));
                }
            }
        }
        Ok(dependencies)
    }

    /// Extracts dependencies from a `Poetry` lock file and formats them as `package==version`.
    fn get_poetry_dep(&self) -> ResultDynError<Vec<String>> {
        let parsed: TomlValue = self.content.parse()?; // Parse as TOML
        let mut dependencies = Vec::new();

        if let Some(packages) = parsed.get("package").and_then(|p| p.as_array()) {
            for package in packages {
                // assume that there is always a version number
                if let (Some(name), Some(version)) = (
                    package.get("name").and_then(|n| n.as_str()),
                    package.get("version").and_then(|v| v.as_str()),
                ) {
                    let mut em = toml_to_py_marker(package, "python-versions");
                    // look for both marker and markers
                    if let Some(markers) = package
                        .get("markers")
                        .or_else(|| package.get("marker"))
                        .and_then(|m| m.as_str())
                    {
                        em.push(markers.to_string()); // Directly push as String
                    }
                    let dep_string = if em.is_empty() {
                        format!("{}=={}", name, version)
                    } else {
                        format!("{}=={}; {}", name, version, em.join(" and "))
                    };
                    dependencies.push(dep_string);
                }
            }
        }
        Ok(dependencies)
    }

    /// Extracts dependencies from a PEP 751 lock file and formats them as `package==version`.
    fn get_pep751_dep(&self) -> ResultDynError<Vec<String>> {
        let parsed: TomlValue = self.content.parse()?; // Parse as TOML
        let mut dependencies = Vec::new();

        if let Some(packages) = parsed.get("packages").and_then(|p| p.as_array()) {
            for package in packages {
                // assume that there is always a version number
                // note there is a "requires-python" field per package
                if let (Some(name), Some(version)) = (
                    package.get("name").and_then(|n| n.as_str()),
                    package.get("version").and_then(|v| v.as_str()),
                ) {
                    let mut em = toml_to_py_marker(package, "requires-python");
                    if let Some(marker) = package.get("marker").and_then(|m| m.as_str()) {
                        em.push(marker.to_string());
                    }
                    let dep_string = if em.is_empty() {
                        format!("{}=={}", name, version)
                    } else {
                        format!("{}=={}; {}", name, version, em.join(" and "))
                    };
                    dependencies.push(dep_string);
                }
            }
        }
        Ok(dependencies)
    }

    fn get_pipfilelock_dep(
        &self,
        options: Option<&Vec<String>>,
    ) -> ResultDynError<Vec<String>> {
        // always include `default`
        let mut groups = vec!["default".to_string()];
        if let Some(opts) = options {
            groups.extend(opts.iter().cloned());
        }

        let parsed: JsonValue = serde_json::from_str(&self.content)?;
        let mut dependencies = Vec::new();
        for group in groups {
            if let Some(packages) = parsed.get(group).and_then(|g| g.as_object()) {
                for (name, details) in packages.iter() {
                    let em = details
                        .get("markers")
                        .map_or_else(|| "".to_string(), |v| format!("; {}", v));
                    if let Some(version) = details.get("version").and_then(|v| v.as_str())
                    {
                        dependencies.push(format!("{}{}{}", name, version, em));
                    }
                }
            }
        }

        Ok(dependencies)
    }

    // Extracts dependencies from a Pixi Lock file
    fn get_pixi_dep(&self) -> ResultDynError<Vec<String>> {
        let parsed_yaml: Value = serde_yaml::from_str(&self.content)?;
        let default_packages: Vec<Value> = Vec::with_capacity(0);
        let packages = parsed_yaml
            .get("packages")
            .and_then(Value::as_sequence)
            .unwrap_or(&default_packages);
        let deps = packages
            .iter()
            .filter_map(|package| {
                let url = package.get("conda")?.as_str()?;
                let filename = url.split('/').next_back()?;
                let (package_name, package_version) = conda_fn_to_name_version(filename)?;

                let marker = package
                    .get("depends")
                    .and_then(Value::as_sequence)
                    .and_then(|deps| {
                        deps.iter().filter_map(Value::as_str).find_map(|dep_str| {
                            dep_str.strip_prefix("python ").map(str_to_py_marker)
                        })
                    })?;
                Some(format!("{}=={}; {}", package_name, package_version, marker))
            })
            .collect();

        Ok(deps)
    }

    /// Extracts dependency specifications from the lock file.
    pub fn get_dependencies(
        &self,
        options: Option<&Vec<String>>,
    ) -> ResultDynError<Vec<String>> {
        if options.is_some() && self.file_type != LockFileType::PipfileLock {
            return Err("Options can only be used with Pipfile.lock".into());
        }

        match self.file_type {
            LockFileType::Requirements => self.get_uv_requirements_dep(),
            LockFileType::UvLock => self.get_uv_native_dep(),
            LockFileType::Poetry => self.get_poetry_dep(),
            LockFileType::PipfileLock => self.get_pipfilelock_dep(options),
            LockFileType::PEP751 => self.get_pep751_dep(),
            LockFileType::PixiLock => self.get_pixi_dep(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_dependencies_uv_a() {
        let content = r#"
opentelemetry-api==1.24.0
    # via
    #   apache-airflow
    #   opentelemetry-exporter-otlp-proto-grpc
    #   opentelemetry-exporter-otlp-proto-http
opentelemetry-exporter-otlp==1.24.0
    # via apache-airflow
apache-airflow
"#;
        let lockfile = LockFile::new(content.to_string());
        let dependencies = lockfile.get_dependencies(None).unwrap();

        assert_eq!(
            dependencies,
            vec![
                "opentelemetry-api==1.24.0".to_string(),
                "opentelemetry-exporter-otlp==1.24.0".to_string(),
                "apache-airflow".to_string(),
            ]
        );
    }

    #[test]
    fn test_get_dependencies_uv_b() {
        let content = r#"
# This file was autogenerated by uv via the following command:
#    uv pip compile pyproject.toml -o requiremnts.lock
arraykit==0.10.0
    # via static-frame
arraymap==0.4.0
    # via static-frame
certifi==2025.1.31
    # via requests
charset-normalizer==3.4.1
    # via requests
idna==3.10
    # via requests
jinja2==3.1.3
    # via test-poetry (pyproject.toml)
markupsafe==3.0.2
    # via jinja2
numpy==2.2.2
    # via
    #   arraykit
    #   arraymap
    #   static-frame
requests==2.32.3
    # via test-poetry (pyproject.toml)
static-frame==2.16.1
    # via test-poetry (pyproject.toml)
typing-extensions==4.12.2
    # via static-frame
urllib3==2.3.0
    # via requests
zipp==3.18.1
    # via test-poetry (pyproject.toml)
"#;
        let lockfile = LockFile::new(content.to_string());
        let dependencies = lockfile.get_dependencies(None).unwrap();

        assert_eq!(
            dependencies,
            vec![
                "arraykit==0.10.0",
                "arraymap==0.4.0",
                "certifi==2025.1.31",
                "charset-normalizer==3.4.1",
                "idna==3.10",
                "jinja2==3.1.3",
                "markupsafe==3.0.2",
                "numpy==2.2.2",
                "requests==2.32.3",
                "static-frame==2.16.1",
                "typing-extensions==4.12.2",
                "urllib3==2.3.0",
                "zipp==3.18.1"
            ]
        );
    }

    #[test]
    fn test_get_dependencies_uv_c() {
        let content = r#"
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


[[distribution]]
name = "certifi"
version = "2025.1.31"
source = { registry = "https://pypi.org/simple" }
sdist = { url = "https://files.pythonhosted.org/packages/1c/ab/c9f1e32b7b1bf505bf26f0ef697775960db7932abeb7b516de930ba2705f/certifi-2025.1.31.tar.gz", hash = "sha256:3d5da6925056f6f18f119200434a4780a94263f10d1c21d032a6f6b2baa20651", size = 167577 }
wheels = [
    { url = "https://files.pythonhosted.org/packages/38/fc/bce832fd4fd99766c04d1ee0eead6b0ec6486fb100ae5e74c1d91292b982/certifi-2025.1.31-py3-none-any.whl", hash = "sha256:ca78db4565a652026a4db2bcdf68f2fb589ea80d0be70e03929ed730746b84fe", size = 166393 },
]

[[distribution]]
name = "charset-normalizer"
version = "3.4.1"
source = { registry = "https://pypi.org/simple" }
sdist = { url = "https://files.pythonhosted.org/packages/16/b0/572805e227f01586461c80e0fd25d65a2115599cc9dad142fee4b747c357/charset_normalizer-3.4.1.tar.gz", hash = "sha256:44251f18cd68a75b56585dd00dae26183e102cd5e0f9f1466e6df5da2ed64ea3", size = 123188 }


[[distribution]]
name = "idna"
version = "3.10"
source = { registry = "https://pypi.org/simple" }
sdist = { url = "https://files.pythonhosted.org/packages/f1/70/7703c29685631f5a7590aa73f1f1d3fa9a380e654b86af429e0934a32f7d/idna-3.10.tar.gz", hash = "sha256:12f65c9b470abda6dc35cf8e63cc574b1c52b11df2c86030af0ac09b01b13ea9", size = 190490 }
wheels = [
    { url = "https://files.pythonhosted.org/packages/76/c6/c88e154df9c4e1a2a66ccf0005a88dfb2650c1dffb6f5ce603dfbd452ce3/idna-3.10-py3-none-any.whl", hash = "sha256:946d195a0d259cbba61165e88e65941f16e9b36ea6ddb97f00452bae8b1287d3", size = 70442 },
]

[[distribution]]
name = "jinja2"
version = "3.1.3"
source = { registry = "https://pypi.org/simple" }
dependencies = [
    { name = "markupsafe" },
]
sdist = { url = "https://files.pythonhosted.org/packages/b2/5e/3a21abf3cd467d7876045335e681d276ac32492febe6d98ad89562d1a7e1/Jinja2-3.1.3.tar.gz", hash = "sha256:ac8bd6544d4bb2c9792bf3a159e80bba8fda7f07e81bc3aed565432d5925ba90", size = 268261 }
wheels = [
    { url = "https://files.pythonhosted.org/packages/30/6d/6de6be2d02603ab56e72997708809e8a5b0fbfee080735109b40a3564843/Jinja2-3.1.3-py3-none-any.whl", hash = "sha256:7d6d50dd97d52cbc355597bd845fabfbac3f551e1f99619e39a35ce8c370b5fa", size = 133236 },
]

[[distribution]]
name = "markupsafe"
version = "3.0.2"
source = { registry = "https://pypi.org/simple" }
sdist = { url = "https://files.pythonhosted.org/packages/b2/97/5d42485e71dfc078108a86d6de8fa46db44a1a9295e89c5d6d4a06e23a62/markupsafe-3.0.2.tar.gz", hash = "sha256:ee55d3edf80167e48ea11a923c7386f4669df67d7994554387f84e7d8b0a2bf0", size = 20537 }


[[distribution]]
name = "numpy"
version = "2.2.2"
source = { registry = "https://pypi.org/simple" }
sdist = { url = "https://files.pythonhosted.org/packages/ec/d0/c12ddfd3a02274be06ffc71f3efc6d0e457b0409c4481596881e748cb264/numpy-2.2.2.tar.gz", hash = "sha256:ed6906f61834d687738d25988ae117683705636936cc605be0bb208b23df4d8f", size = 20233295 }


[[distribution]]
name = "requests"
version = "2.32.3"
source = { registry = "https://pypi.org/simple" }
dependencies = [
    { name = "certifi" },
    { name = "charset-normalizer" },
    { name = "idna" },
    { name = "urllib3" },
]
sdist = { url = "https://files.pythonhosted.org/packages/63/70/2bf7780ad2d390a8d301ad0b550f1581eadbd9a20f896afe06353c2a2913/requests-2.32.3.tar.gz", hash = "sha256:55365417734eb18255590a9ff9eb97e9e1da868d4ccd6402399eaf68af20a760", size = 131218 }
wheels = [
    { url = "https://files.pythonhosted.org/packages/f9/9b/335f9764261e915ed497fcdeb11df5dfd6f7bf257d4a6a2a686d80da4d54/requests-2.32.3-py3-none-any.whl", hash = "sha256:70761cfe03c773ceb22aa2f671b4757976145175cdfca038c02654d061d6dcc6", size = 64928 },
]

[[distribution]]
name = "static-frame"
version = "2.16.1"
source = { registry = "https://pypi.org/simple" }
dependencies = [
    { name = "arraykit" },
    { name = "arraymap" },
    { name = "numpy" },
    { name = "typing-extensions" },
]
sdist = { url = "https://files.pythonhosted.org/packages/51/12/747c89bd6fcdd5d03c5565ca76448b5d6f641e436ea9f948ccaa7af20a15/static-frame-2.16.1.tar.gz", hash = "sha256:0f0e5e1c09c06891d71dca9b24e04526e99407dfae9d25d79e2011cb69c9ed9a", size = 733653 }
wheels = [
    { url = "https://files.pythonhosted.org/packages/f5/bf/d71b2bf7504437ce5365ad6ad975d7773adc55da6bde9b3ebd5c28a85d0b/static_frame-2.16.1-py3-none-any.whl", hash = "sha256:9f330f0672a6491bb9be0f64c64ee8abf06ffb9ee1b253f1f7695719538cc4df", size = 791035 },
]

[[distribution]]
name = "typing-extensions"
version = "4.12.2"
source = { registry = "https://pypi.org/simple" }
sdist = { url = "https://files.pythonhosted.org/packages/df/db/f35a00659bc03fec321ba8bce9420de607a1d37f8342eee1863174c69557/typing_extensions-4.12.2.tar.gz", hash = "sha256:1a7ead55c7e559dd4dee8856e3a88b41225abfe1ce8df57b7c13915fe121ffb8", size = 85321 }
wheels = [
    { url = "https://files.pythonhosted.org/packages/26/9f/ad63fc0248c5379346306f8668cda6e2e2e9c95e01216d2b8ffd9ff037d0/typing_extensions-4.12.2-py3-none-any.whl", hash = "sha256:04e5ca0351e0f3f85c6853954072df659d0d13fac324d0072316b67d7794700d", size = 37438 },
]

[[distribution]]
name = "urllib3"
version = "2.3.0"
source = { registry = "https://pypi.org/simple" }
sdist = { url = "https://files.pythonhosted.org/packages/aa/63/e53da845320b757bf29ef6a9062f5c669fe997973f966045cb019c3f4b66/urllib3-2.3.0.tar.gz", hash = "sha256:f8c5449b3cf0861679ce7e0503c7b44b5ec981bec0d1d3795a07f1ba96f0204d", size = 307268 }
wheels = [
    { url = "https://files.pythonhosted.org/packages/c8/19/4ec628951a74043532ca2cf5d97b7b14863931476d117c471e8e2b1eb39f/urllib3-2.3.0-py3-none-any.whl", hash = "sha256:1cee9ad369867bfdbbb48b7dd50374c0967a0bb7710050facf0dd6911440e3df", size = 128369 },
]

[[distribution]]
name = "zipp"
version = "3.18.1"
source = { registry = "https://pypi.org/simple" }
sdist = { url = "https://files.pythonhosted.org/packages/3e/ef/65da662da6f9991e87f058bc90b91a935ae655a16ae5514660d6460d1298/zipp-3.18.1.tar.gz", hash = "sha256:2884ed22e7d8961de1c9a05142eb69a247f120291bc0206a00a7642f09b5b715", size = 21220 }
wheels = [
    { url = "https://files.pythonhosted.org/packages/c2/0a/ba9d0ee9536d3ef73a3448e931776e658b36f128d344e175bc32b092a8bf/zipp-3.18.1-py3-none-any.whl", hash = "sha256:206f5a15f2af3dbaee80769fb7dc6f249695e940acca08dfb2a4769fe61e538b", size = 8247 },
]
"#;
        let lockfile = LockFile::new(content.to_string());
        let dependencies = lockfile.get_dependencies(None).unwrap();

        assert_eq!(
            dependencies,
            vec![
                "arraykit==0.10.0",
                "arraymap==0.4.0",
                "certifi==2025.1.31",
                "charset-normalizer==3.4.1",
                "idna==3.10",
                "jinja2==3.1.3",
                "markupsafe==3.0.2",
                "numpy==2.2.2",
                "requests==2.32.3",
                "static-frame==2.16.1",
                "typing-extensions==4.12.2",
                "urllib3==2.3.0",
                "zipp==3.18.1"
            ]
        );
    }

    #[test]
    fn test_get_dependencies_poetry_a() {
        let poetry_content = r#"
            [[package]]
            name = "packaging"
            version = "24.2"

            [[package]]
            name = "requests"
            version = "2.31.0"
        "#;
        let lockfile = LockFile::new(poetry_content.to_string());
        let dependencies = lockfile.get_dependencies(None).unwrap();
        assert_eq!(dependencies, vec!["packaging==24.2", "requests==2.31.0"]);
    }

    #[test]
    fn test_get_dependencies_poetry_b() {
        let poetry_content = r#"
# This file is automatically @generated by Poetry 2.0.1 and should not be changed by hand.

[[package]]
name = "arraykit"
version = "0.10.0"
description = "Array utilities for StaticFrame"
optional = false
python-versions = ">=3.9"
groups = ["main"]

[package.dependencies]
numpy = ">=1.19.5"

[[package]]
name = "arraymap"
version = "0.4.0"
description = "Dictionary-like lookup from NumPy array values to their integer positions"
optional = false
python-versions = ">=3.9"
groups = ["main"]

[package.dependencies]
numpy = ">=1.19.5"

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

[[package]]
name = "idna"
version = "3.10"
description = "Internationalized Domain Names in Applications (IDNA)"
optional = false
python-versions = ">=3.6"
groups = ["main"]
files = [
    {file = "idna-3.10-py3-none-any.whl", hash = "sha256:946d195a0d259cbba61165e88e65941f16e9b36ea6ddb97f00452bae8b1287d3"},
    {file = "idna-3.10.tar.gz", hash = "sha256:12f65c9b470abda6dc35cf8e63cc574b1c52b11df2c86030af0ac09b01b13ea9"},
]

[package.extras]
all = ["flake8 (>=7.1.1)", "mypy (>=1.11.2)", "pytest (>=8.3.2)", "ruff (>=0.6.2)"]

[[package]]
name = "jinja2"
version = "3.1.3"
description = "A very fast and expressive template engine."
optional = false
python-versions = ">=3.7"
groups = ["main"]
files = [
    {file = "Jinja2-3.1.3-py3-none-any.whl", hash = "sha256:7d6d50dd97d52cbc355597bd845fabfbac3f551e1f99619e39a35ce8c370b5fa"},
    {file = "Jinja2-3.1.3.tar.gz", hash = "sha256:ac8bd6544d4bb2c9792bf3a159e80bba8fda7f07e81bc3aed565432d5925ba90"},
]

[package.dependencies]
MarkupSafe = ">=2.0"

[package.extras]
i18n = ["Babel (>=2.7)"]

[[package]]
name = "markupsafe"
version = "3.0.2"
description = "Safely add untrusted strings to HTML/XML markup."
optional = false
python-versions = ">=3.9"
groups = ["main"]

[[package]]
name = "numpy"
version = "2.2.2"
description = "Fundamental package for array computing in Python"
optional = false
python-versions = ">=3.10"
groups = ["main"]

[[package]]
name = "requests"
version = "2.32.3"
description = "Python HTTP for Humans."
optional = false
python-versions = ">=3.8"
groups = ["main"]
files = [
    {file = "requests-2.32.3-py3-none-any.whl", hash = "sha256:70761cfe03c773ceb22aa2f671b4757976145175cdfca038c02654d061d6dcc6"},
    {file = "requests-2.32.3.tar.gz", hash = "sha256:55365417734eb18255590a9ff9eb97e9e1da868d4ccd6402399eaf68af20a760"},
]

[package.dependencies]
certifi = ">=2017.4.17"
charset-normalizer = ">=2,<4"
idna = ">=2.5,<4"
urllib3 = ">=1.21.1,<3"

[package.extras]
socks = ["PySocks (>=1.5.6,!=1.5.7)"]
use-chardet-on-py3 = ["chardet (>=3.0.2,<6)"]

[[package]]
name = "static-frame"
version = "2.16.1"
description = "Immutable and statically-typeable DataFrames with runtime type and data validation."
optional = false
python-versions = ">=3.9"
groups = ["main"]
files = [
    {file = "static-frame-2.16.1.tar.gz", hash = "sha256:0f0e5e1c09c06891d71dca9b24e04526e99407dfae9d25d79e2011cb69c9ed9a"},
    {file = "static_frame-2.16.1-py3-none-any.whl", hash = "sha256:9f330f0672a6491bb9be0f64c64ee8abf06ffb9ee1b253f1f7695719538cc4df"},
]

[package.dependencies]
arraykit = "0.10.0"
arraymap = "0.4.0"
numpy = ">=1.23.5"
typing-extensions = ">=4.12.0"

[package.extras]
extras = ["duckdb (>=1.0.0)", "msgpack (>=1.0.4)", "msgpack-numpy (>=0.4.8)", "openpyxl (>=3.0.9)", "pandas (>=1.1.5)", "pyarrow (>=3.0.0)", "tables (>=3.9.1)", "visidata (>=2.4)", "xarray (>=0.13.0)", "xlsxwriter (>=1.1.2)"]

[[package]]
name = "typing-extensions"
version = "4.12.2"
description = "Backported and Experimental Type Hints for Python 3.8+"
optional = false
python-versions = ">=3.8"
groups = ["main"]
files = [
    {file = "typing_extensions-4.12.2-py3-none-any.whl", hash = "sha256:04e5ca0351e0f3f85c6853954072df659d0d13fac324d0072316b67d7794700d"},
    {file = "typing_extensions-4.12.2.tar.gz", hash = "sha256:1a7ead55c7e559dd4dee8856e3a88b41225abfe1ce8df57b7c13915fe121ffb8"},
]

[[package]]
name = "urllib3"
version = "2.3.0"
description = "HTTP library with thread-safe connection pooling, file post, and more."
optional = false
python-versions = ">=3.9"
groups = ["main"]
files = [
    {file = "urllib3-2.3.0-py3-none-any.whl", hash = "sha256:1cee9ad369867bfdbbb48b7dd50374c0967a0bb7710050facf0dd6911440e3df"},
    {file = "urllib3-2.3.0.tar.gz", hash = "sha256:f8c5449b3cf0861679ce7e0503c7b44b5ec981bec0d1d3795a07f1ba96f0204d"},
]

[package.extras]
brotli = ["brotli (>=1.0.9)", "brotlicffi (>=0.8.0)"]
h2 = ["h2 (>=4,<5)"]
socks = ["pysocks (>=1.5.6,!=1.5.7,<2.0)"]
zstd = ["zstandard (>=0.18.0)"]

[[package]]
name = "zipp"
version = "3.18.1"
description = "Backport of pathlib-compatible object wrapper for zip files"
optional = false
python-versions = ">=3.8"
groups = ["main"]
files = [
    {file = "zipp-3.18.1-py3-none-any.whl", hash = "sha256:206f5a15f2af3dbaee80769fb7dc6f249695e940acca08dfb2a4769fe61e538b"},
    {file = "zipp-3.18.1.tar.gz", hash = "sha256:2884ed22e7d8961de1c9a05142eb69a247f120291bc0206a00a7642f09b5b715"},
]

[package.extras]
docs = ["furo", "jaraco.packaging (>=9.3)", "jaraco.tidelift (>=1.4)", "rst.linker (>=1.9)", "sphinx (>=3.5)", "sphinx-lint"]
testing = ["big-O", "jaraco.functools", "jaraco.itertools", "more-itertools", "pytest (>=6)", "pytest-checkdocs (>=2.4)", "pytest-cov", "pytest-enabler (>=2.2)", "pytest-ignore-flaky", "pytest-mypy", "pytest-ruff (>=0.2.1)"]

[metadata]
lock-version = "2.1"
python-versions = ">=3.12"
content-hash = "88d4af2d19b75cf5d80ba6b72bbee80790fa9757747e24304c4b1c51e86f3837"
        "#;
        let lockfile = LockFile::new(poetry_content.to_string());
        let dependencies = lockfile.get_dependencies(None).unwrap();
        assert_eq!(
            dependencies,
            vec![
                "arraykit==0.10.0; python_version >= '3.9'",
                "arraymap==0.4.0; python_version >= '3.9'",
                "certifi==2025.1.31; python_version >= '3.6'",
                "charset-normalizer==3.4.1; python_version >= '3.7'",
                "idna==3.10; python_version >= '3.6'",
                "jinja2==3.1.3; python_version >= '3.7'",
                "markupsafe==3.0.2; python_version >= '3.9'",
                "numpy==2.2.2; python_version >= '3.10'",
                "requests==2.32.3; python_version >= '3.8'",
                "static-frame==2.16.1; python_version >= '3.9'",
                "typing-extensions==4.12.2; python_version >= '3.8'",
                "urllib3==2.3.0; python_version >= '3.9'",
                "zipp==3.18.1; python_version >= '3.8'"
            ]
        );
    }

    #[test]
    fn test_get_dependencies_poetry_c() {
        let poetry_content = r#"
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
        let lockfile = LockFile::new(poetry_content.to_string());
        let dependencies = lockfile.get_dependencies(None).unwrap();
        assert_eq!(
            dependencies,
            vec!["pathlib2==2.3.7.post1; sys_platform == \"win32\"",
            "six==1.17.0; python_version != '3.0.*' and python_version != '3.1.*' and python_version != '3.2.*' and python_version >= '2.7' and sys_platform == \"win32\""]
        );
    }

    //--------------------------------------------------------------------------

    #[test]
    fn test_get_dependencies_pipfilelock_a() {
        let pipfile_lock_content = r#"
        {
            "_meta": { "hash": { "sha256": "abc123" } },
            "default": {
                "asgiref": { "version": "==3.6.0" },
                "django": { "version": "==4.1.7" }
            },
            "develop": {
                "attrs": { "version": "==22.2.0" }
            }
        }
        "#;

        let lockfile = LockFile::new(pipfile_lock_content.to_string());

        let dependencies_default = lockfile.get_dependencies(None).unwrap();
        assert_eq!(
            dependencies_default,
            vec!["asgiref==3.6.0", "django==4.1.7"]
        );

        let dependencies_with_develop = lockfile
            .get_dependencies(Some(&vec!["develop".to_string()]))
            .unwrap();
        assert_eq!(
            dependencies_with_develop,
            vec!["asgiref==3.6.0", "django==4.1.7", "attrs==22.2.0"]
        );
    }

    #[test]
    fn test_get_dependencies_pipfilelock_b() {
        let pipfile_lock_content = r#"
        {
            "_meta": { "hash": { "sha256": "abc123" } },
            "default": {
                "asgiref": { "markers": "python_version < '3.4'", "version": "==3.6.0" },
                "django": { "markers": "python_version >= '3.4'", "version": "==4.1.7" }
            },
            "develop": {
                "attrs": { "markers": "python_version >= '3.4'", "version": "==22.2.0" }
            }
        }
        "#;

        let lockfile = LockFile::new(pipfile_lock_content.to_string());

        let dependencies_default = lockfile.get_dependencies(None).unwrap();
        assert_eq!(
            dependencies_default,
            vec![
                "asgiref==3.6.0; \"python_version < '3.4'\"",
                "django==4.1.7; \"python_version >= '3.4'\""
            ]
        );

        let dependencies_with_develop = lockfile
            .get_dependencies(Some(&vec!["develop".to_string()]))
            .unwrap();
        assert_eq!(
            dependencies_with_develop,
            vec![
                "asgiref==3.6.0; \"python_version < '3.4'\"",
                "django==4.1.7; \"python_version >= '3.4'\"",
                "attrs==22.2.0; \"python_version >= '3.4'\""
            ]
        );
    }
    //--------------------------------------------------------------------------

    #[test]
    fn test_get_dependencies_pip_tools_a() {
        let content = r#"
#
# This file is autogenerated by pip-compile with Python 3.13
# by the following command:
#
#    pip-compile --output-file=requirements.lock requirements.txt
#
certifi==2025.1.31
    # via requests
charset-normalizer==3.4.1
    # via requests
idna==3.10
    # via requests
jinja2==3.1.3
    # via -r requirements.txt
markupsafe==3.0.2
    # via jinja2
requests==2.32.3
    # via -r requirements.txt
urllib3==2.3.0
    # via requests
zipp==3.18.1
    # via -r requirements.txt
"#;
        let lockfile = LockFile::new(content.to_string());
        let dependencies = lockfile.get_dependencies(None).unwrap();

        assert_eq!(
            dependencies,
            vec![
                "certifi==2025.1.31",
                "charset-normalizer==3.4.1",
                "idna==3.10",
                "jinja2==3.1.3",
                "markupsafe==3.0.2",
                "requests==2.32.3",
                "urllib3==2.3.0",
                "zipp==3.18.1"
            ]
        );
    }

    #[test]
    fn test_get_dependencies_pep751_a() {
        let content = r#"
metadata-version = "1.0"
requires-python = ">=3.9"
created-by = "PEP 751"

[[packages]]
name = "attrs"
version = "23.2.0"
requires-python = ">=3.7"
index = "https://pypi.org/simple/"
wheels = [
    {name = "attrs-23.2.0-py3-none-any.whl", upload-time = 2023-12-31T06:30:30.772444Z, url = "https://files.pythonhosted.org/packages/e0/44/827b2a91a5816512fcaf3cc4ebc465ccd5d598c45cefa6703fcf4a79018f/attrs-23.2.0-py3-none-any.whl", size = 60752, hashes = {sha256 = "99b87a485a5820b23b879f04c2305b44b951b502fd64be915879d77a7e8fc6f1"} }
]

[[packages]]
name = "cattrs"
version = "23.2.3"
requires-python = ">=3.8"
index = "https://pypi.org/simple/"
wheels = [
    {name = "cattrs-23.2.3-py3-none-any.whl", upload-time = 2023-11-30T22:19:19.163763Z, url = "https://files.pythonhosted.org/packages/b3/0d/cd4a4071c7f38385dc5ba91286723b4d1090b87815db48216212c6c6c30e/cattrs-23.2.3-py3-none-any.whl", size = 57474, hashes = {sha256 = "0341994d94971052e9ee70662542699a3162ea1e0c62f7ce1b4a57f563685108"} }
]

[[packages]]
name = "numpy"
version = "2.0.1"
requires-python = ">=3.9"
index = "https://pypi.org/simple/"
wheels = [
    {name = "numpy-2.0.1-cp312-cp312-macosx_10_9_x86_64.whl", upload-time = 2024-07-21T13:37:15.810939Z, url = "https://files.pythonhosted.org/packages/64/1c/401489a7e92c30db413362756c313b9353fb47565015986c55582593e2ae/numpy-2.0.1-cp312-cp312-macosx_10_9_x86_64.whl", size = 20965374, hashes = {sha256 = "6bf4e6f4a2a2e26655717a1983ef6324f2664d7011f6ef7482e8c0b3d51e82ac"} },
    {name = "numpy-2.0.1-cp312-cp312-win_amd64.whl", upload-time = 2024-07-21T13:40:17.532627Z, url = "https://files.pythonhosted.org/packages/b5/59/f6ad378ad85ed9c2785f271b39c3e5b6412c66e810d2c60934c9f/numpy-2.0.1-cp312-cp312-win_amd64.whl", size = 16255757, hashes = {sha256 = "bb2124fdc6e62baae159ebcfa368708867eb56806804d005860b6007388df171"} },
]
"#;
        let lockfile = LockFile::new(content.to_string());
        let dependencies = lockfile.get_dependencies(None).unwrap();

        assert_eq!(
            dependencies,
            vec![
                "attrs==23.2.0; python_version >= '3.7'",
                "cattrs==23.2.3; python_version >= '3.8'",
                "numpy==2.0.1; python_version >= '3.9'"
            ]
        );
    }

    #[test]
    fn test_get_dependencies_pep751_b() {
        let content = r#"
metadata-version = "1.0"
requires-python = ">=3.9"
created-by = "PEP 751"

[[packages]]
name = "attrs"
version = "23.2.0"
requires-python = ">=3.7"
marker = "python_version > '3.0'"
index = "https://pypi.org/simple/"
wheels = [
    {name = "attrs-23.2.0-py3-none-any.whl", upload-time = 2023-12-31T06:30:30.772444Z, url = "https://files.pythonhosted.org/packages/e0/44/827b2a91a5816512fcaf3cc4ebc465ccd5d598c45cefa6703fcf4a79018f/attrs-23.2.0-py3-none-any.whl", size = 60752, hashes = {sha256 = "99b87a485a5820b23b879f04c2305b44b951b502fd64be915879d77a7e8fc6f1"} }
]

[[packages]]
name = "cattrs"
version = "23.2.3"
requires-python = ">=3.8"
marker = "python_version > '3.1'"
index = "https://pypi.org/simple/"
wheels = [
    {name = "cattrs-23.2.3-py3-none-any.whl", upload-time = 2023-11-30T22:19:19.163763Z, url = "https://files.pythonhosted.org/packages/b3/0d/cd4a4071c7f38385dc5ba91286723b4d1090b87815db48216212c6c6c30e/cattrs-23.2.3-py3-none-any.whl", size = 57474, hashes = {sha256 = "0341994d94971052e9ee70662542699a3162ea1e0c62f7ce1b4a57f563685108"} }
]

"#;
        let lockfile = LockFile::new(content.to_string());
        let dependencies = lockfile.get_dependencies(None).unwrap();

        assert_eq!(
            dependencies,
            vec![
                "attrs==23.2.0; python_version >= '3.7' and python_version > '3.0'",
                "cattrs==23.2.3; python_version >= '3.8' and python_version > '3.1'"
            ]
        );
    }

    #[test]
    fn test_get_dependencies_pixi_lock() {
        let content = r#"
version: 6
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages:
      linux-64:
      - conda: https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2
      - conda: https://conda.anaconda.org/conda-forge/linux-64/_openmp_mutex-4.5-2_gnu.tar.bz2
      - conda: https://conda.anaconda.org/conda-forge/linux-64/bzip2-1.0.8-h4bc722e_7.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/ca-certificates-2025.1.31-hbcca054_0.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/ld_impl_linux-64-2.43-h712a8e2_4.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libblas-3.9.0-31_h59b9bed_openblas.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libcblas-3.9.0-31_he106b2a_openblas.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libffi-3.4.6-h2dba641_0.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libgcc-14.2.0-h767d61c_2.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libgcc-ng-14.2.0-h69a702a_2.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libgfortran-14.2.0-h69a702a_2.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libgfortran5-14.2.0-hf1ad2bd_2.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libgomp-14.2.0-h767d61c_2.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/liblapack-3.9.0-31_h7ac8fdf_openblas.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/liblzma-5.6.4-hb9d3cd8_0.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libnsl-2.0.1-hd590300_0.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libopenblas-0.3.29-pthreads_h94d23a6_0.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libsqlite-3.49.1-hee588c1_2.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libstdcxx-14.2.0-h8f9b012_2.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libuuid-2.38.1-h0b41bf4_0.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libxcrypt-4.4.36-hd590300_1.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/libzlib-1.3.1-hb9d3cd8_2.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/ncurses-6.5-h2d0b736_3.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/numpy-2.0.2-py39h9cb892a_1.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/openssl-3.4.1-h7b32b05_0.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/pandas-2.2.3-py39h3b40f6f_2.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/python-3.9.21-h9c0c6dc_1_cpython.conda
      - conda: https://conda.anaconda.org/conda-forge/noarch/python-dateutil-2.9.0.post0-pyhff2d567_1.conda
      - conda: https://conda.anaconda.org/conda-forge/noarch/python-tzdata-2025.1-pyhd8ed1ab_0.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/python_abi-3.9-5_cp39.conda
      - conda: https://conda.anaconda.org/conda-forge/noarch/pytz-2024.1-pyhd8ed1ab_0.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/readline-8.2-h8c095d6_2.conda
      - conda: https://conda.anaconda.org/conda-forge/noarch/six-1.17.0-pyhd8ed1ab_0.conda
      - conda: https://conda.anaconda.org/conda-forge/linux-64/tk-8.6.13-noxft_h4845f30_101.conda
      - conda: https://conda.anaconda.org/conda-forge/noarch/tzdata-2025a-h78e105d_0.conda
packages:
- conda: https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2
  sha256: fe51de6107f9edc7aa4f786a70f4a883943bc9d39b3bb7307c04c41410990726
  md5: d7c89558ba9fa0495403155b64376d81
  license: None
  size: 2562
  timestamp: 1578324546067
- conda: https://conda.anaconda.org/conda-forge/linux-64/_openmp_mutex-4.5-2_gnu.tar.bz2
  build_number: 16
  sha256: fbe2c5e56a653bebb982eda4876a9178aedfc2b545f25d0ce9c4c0b508253d22
  md5: 73aaf86a425cc6e73fcf236a5a46396d
  depends:
  - _libgcc_mutex 0.1 conda_forge
  - libgomp >=7.5.0
  constrains:
  - openmp_impl 9999
  license: BSD-3-Clause
  license_family: BSD
  size: 23621
  timestamp: 1650670423406
- conda: https://conda.anaconda.org/conda-forge/linux-64/bzip2-1.0.8-h4bc722e_7.conda
  sha256: 5ced96500d945fb286c9c838e54fa759aa04a7129c59800f0846b4335cee770d
  md5: 62ee74e96c5ebb0af99386de58cf9553
  depends:
  - __glibc >=2.17,<3.0.a0
  - libgcc-ng >=12
  license: bzip2-1.0.6
  license_family: BSD
  size: 252783
  timestamp: 1720974456583
- conda: https://conda.anaconda.org/conda-forge/linux-64/ca-certificates-2025.1.31-hbcca054_0.conda
  sha256: bf832198976d559ab44d6cdb315642655547e26d826e34da67cbee6624cda189
  md5: 19f3a56f68d2fd06c516076bff482c52
  license: ISC
  size: 158144
  timestamp: 1738298224464
- conda: https://conda.anaconda.org/conda-forge/linux-64/ld_impl_linux-64-2.43-h712a8e2_4.conda
  sha256: db73f38155d901a610b2320525b9dd3b31e4949215c870685fd92ea61b5ce472
  md5: 01f8d123c96816249efd255a31ad7712
  depends:
  - __glibc >=2.17,<3.0.a0
  constrains:
  - binutils_impl_linux-64 2.43
  license: GPL-3.0-only
  license_family: GPL
  size: 671240
  timestamp: 1740155456116
- conda: https://conda.anaconda.org/conda-forge/linux-64/libblas-3.9.0-31_h59b9bed_openblas.conda
  build_number: 31
  sha256: 9839fc4ac0cbb0aa3b9eea520adfb57311838959222654804e58f6f2d1771db5
  md5: 728dbebd0f7a20337218beacffd37916
  depends:
  - libopenblas >=0.3.29,<0.3.30.0a0
  - libopenblas >=0.3.29,<1.0a0
  constrains:
  - liblapacke =3.9.0=31*_openblas
  - liblapack =3.9.0=31*_openblas
  - blas =2.131=openblas
  - mkl <2025
  - libcblas =3.9.0=31*_openblas
  license: BSD-3-Clause
  license_family: BSD
  size: 16859
  timestamp: 1740087969120
- conda: https://conda.anaconda.org/conda-forge/linux-64/libcblas-3.9.0-31_he106b2a_openblas.conda
  build_number: 31
  sha256: ede8545011f5b208b151fe3e883eb4e31d495ab925ab7b9ce394edca846e0c0d
  md5: abb32c727da370c481a1c206f5159ce9
  depends:
  - libblas 3.9.0 31_h59b9bed_openblas
  constrains:
  - liblapacke =3.9.0=31*_openblas
  - liblapack =3.9.0=31*_openblas
  - blas =2.131=openblas
  license: BSD-3-Clause
  license_family: BSD
  size: 16796
  timestamp: 1740087984429
- conda: https://conda.anaconda.org/conda-forge/linux-64/libffi-3.4.6-h2dba641_0.conda
  sha256: 67a6c95e33ebc763c1adc3455b9a9ecde901850eb2fceb8e646cc05ef3a663da
  md5: e3eb7806380bc8bcecba6d749ad5f026
  depends:
  - __glibc >=2.17,<3.0.a0
  - libgcc >=13
  license: MIT
  license_family: MIT
  size: 53415
  timestamp: 1739260413716
- conda: https://conda.anaconda.org/conda-forge/linux-64/libgcc-14.2.0-h767d61c_2.conda
  sha256: 3a572d031cb86deb541d15c1875aaa097baefc0c580b54dc61f5edab99215792
  md5: ef504d1acbd74b7cc6849ef8af47dd03
  depends:
  - __glibc >=2.17,<3.0.a0
  - _openmp_mutex >=4.5
  constrains:
  - libgomp 14.2.0 h767d61c_2
  - libgcc-ng ==14.2.0=*_2
  license: GPL-3.0-only WITH GCC-exception-3.1
  license_family: GPL
  size: 847885
  timestamp: 1740240653082
- conda: https://conda.anaconda.org/conda-forge/linux-64/libgcc-ng-14.2.0-h69a702a_2.conda
  sha256: fb7558c328b38b2f9d2e412c48da7890e7721ba018d733ebdfea57280df01904
  md5: a2222a6ada71fb478682efe483ce0f92
  depends:
  - libgcc 14.2.0 h767d61c_2
  license: GPL-3.0-only WITH GCC-exception-3.1
  license_family: GPL
  size: 53758
  timestamp: 1740240660904
- conda: https://conda.anaconda.org/conda-forge/linux-64/libgfortran-14.2.0-h69a702a_2.conda
  sha256: e05263e8960da03c341650f2a3ffa4ccae4e111cb198e8933a2908125459e5a6
  md5: fb54c4ea68b460c278d26eea89cfbcc3
  depends:
  - libgfortran5 14.2.0 hf1ad2bd_2
  constrains:
  - libgfortran-ng ==14.2.0=*_2
  license: GPL-3.0-only WITH GCC-exception-3.1
  license_family: GPL
  size: 53733
  timestamp: 1740240690977
- conda: https://conda.anaconda.org/conda-forge/linux-64/libgfortran5-14.2.0-hf1ad2bd_2.conda
  sha256: c17b7cf3073a1f4e1f34d50872934fa326346e104d3c445abc1e62481ad6085c
  md5: 556a4fdfac7287d349b8f09aba899693
  depends:
  - __glibc >=2.17,<3.0.a0
  - libgcc >=14.2.0
  constrains:
  - libgfortran 14.2.0
  license: GPL-3.0-only WITH GCC-exception-3.1
  license_family: GPL
  size: 1461978
  timestamp: 1740240671964
- conda: https://conda.anaconda.org/conda-forge/linux-64/libgomp-14.2.0-h767d61c_2.conda
  sha256: 1a3130e0b9267e781b89399580f3163632d59fe5b0142900d63052ab1a53490e
  md5: 06d02030237f4d5b3d9a7e7d348fe3c6
  depends:
  - __glibc >=2.17,<3.0.a0
  license: GPL-3.0-only WITH GCC-exception-3.1
  license_family: GPL
  size: 459862
  timestamp: 1740240588123
- conda: https://conda.anaconda.org/conda-forge/linux-64/liblapack-3.9.0-31_h7ac8fdf_openblas.conda
  build_number: 31
  sha256: f583661921456e798aba10972a8abbd9d33571c655c1f66eff450edc9cbefcf3
  md5: 452b98eafe050ecff932f0ec832dd03f
  depends:
  - libblas 3.9.0 31_h59b9bed_openblas
  constrains:
  - libcblas =3.9.0=31*_openblas
  - liblapacke =3.9.0=31*_openblas
  - blas =2.131=openblas
  license: BSD-3-Clause
  license_family: BSD
  size: 16790
  timestamp: 1740087997375
- conda: https://conda.anaconda.org/conda-forge/linux-64/liblzma-5.6.4-hb9d3cd8_0.conda
  sha256: cad52e10319ca4585bc37f0bc7cce99ec7c15dc9168e42ccb96b741b0a27db3f
  md5: 42d5b6a0f30d3c10cd88cb8584fda1cb
  depends:
  - __glibc >=2.17,<3.0.a0
  - libgcc >=13
  license: 0BSD
  size: 111357
  timestamp: 1738525339684
- conda: https://conda.anaconda.org/conda-forge/linux-64/libnsl-2.0.1-hd590300_0.conda
  sha256: 26d77a3bb4dceeedc2a41bd688564fe71bf2d149fdcf117049970bc02ff1add6
  md5: 30fd6e37fe21f86f4bd26d6ee73eeec7
  depends:
  - libgcc-ng >=12
  license: LGPL-2.1-only
  license_family: GPL
  size: 33408
  timestamp: 1697359010159
- conda: https://conda.anaconda.org/conda-forge/linux-64/libopenblas-0.3.29-pthreads_h94d23a6_0.conda
  sha256: cc5389ea254f111ef17a53df75e8e5209ef2ea6117e3f8aced88b5a8e51f11c4
  md5: 0a4d0252248ef9a0f88f2ba8b8a08e12
  depends:
  - __glibc >=2.17,<3.0.a0
  - libgcc >=14
  - libgfortran
  - libgfortran5 >=14.2.0
  constrains:
  - openblas >=0.3.29,<0.3.30.0a0
  license: BSD-3-Clause
  license_family: BSD
  size: 5919288
  timestamp: 1739825731827
- conda: https://conda.anaconda.org/conda-forge/linux-64/libsqlite-3.49.1-hee588c1_2.conda
  sha256: a086289bf75c33adc1daed3f1422024504ffb5c3c8b3285c49f025c29708ed16
  md5: 962d6ac93c30b1dfc54c9cccafd1003e
  depends:
  - __glibc >=2.17,<3.0.a0
  - libgcc >=13
  - libzlib >=1.3.1,<2.0a0
  license: Unlicense
  size: 918664
  timestamp: 1742083674731
- conda: https://conda.anaconda.org/conda-forge/linux-64/libstdcxx-14.2.0-h8f9b012_2.conda
  sha256: 8f5bd92e4a24e1d35ba015c5252e8f818898478cb3bc50bd8b12ab54707dc4da
  md5: a78c856b6dc6bf4ea8daeb9beaaa3fb0
  depends:
  - __glibc >=2.17,<3.0.a0
  - libgcc 14.2.0 h767d61c_2
  license: GPL-3.0-only WITH GCC-exception-3.1
  license_family: GPL
  size: 3884556
  timestamp: 1740240685253
- conda: https://conda.anaconda.org/conda-forge/linux-64/libuuid-2.38.1-h0b41bf4_0.conda
  sha256: 787eb542f055a2b3de553614b25f09eefb0a0931b0c87dbcce6efdfd92f04f18
  md5: 40b61aab5c7ba9ff276c41cfffe6b80b
  depends:
  - libgcc-ng >=12
  license: BSD-3-Clause
  license_family: BSD
  size: 33601
  timestamp: 1680112270483
- conda: https://conda.anaconda.org/conda-forge/linux-64/libxcrypt-4.4.36-hd590300_1.conda
  sha256: 6ae68e0b86423ef188196fff6207ed0c8195dd84273cb5623b85aa08033a410c
  md5: 5aa797f8787fe7a17d1b0821485b5adc
  depends:
  - libgcc-ng >=12
  license: LGPL-2.1-or-later
  size: 100393
  timestamp: 1702724383534
- conda: https://conda.anaconda.org/conda-forge/linux-64/libzlib-1.3.1-hb9d3cd8_2.conda
  sha256: d4bfe88d7cb447768e31650f06257995601f89076080e76df55e3112d4e47dc4
  md5: edb0dca6bc32e4f4789199455a1dbeb8
  depends:
  - __glibc >=2.17,<3.0.a0
  - libgcc >=13
  constrains:
  - zlib 1.3.1 *_2
  license: Zlib
  license_family: Other
  size: 60963
  timestamp: 1727963148474
- conda: https://conda.anaconda.org/conda-forge/linux-64/ncurses-6.5-h2d0b736_3.conda
  sha256: 3fde293232fa3fca98635e1167de6b7c7fda83caf24b9d6c91ec9eefb4f4d586
  md5: 47e340acb35de30501a76c7c799c41d7
  depends:
  - __glibc >=2.17,<3.0.a0
  - libgcc >=13
  license: X11 AND BSD-3-Clause
  size: 891641
  timestamp: 1738195959188
- conda: https://conda.anaconda.org/conda-forge/linux-64/numpy-2.0.2-py39h9cb892a_1.conda
  sha256: cac3d9a87db5a3b54f8a97c77ee1cf35af6a7f9c725b6911bc5f1d6c6d101637
  md5: be95cf76ebd05d08be67e50e88d3cd49
  depends:
  - __glibc >=2.17,<3.0.a0
  - libblas >=3.9.0,<4.0a0
  - libcblas >=3.9.0,<4.0a0
  - libgcc >=13
  - liblapack >=3.9.0,<4.0a0
  - libstdcxx >=13
  - python >=3.9,<3.10.0a0
  - python_abi 3.9.* *_cp39
  constrains:
  - numpy-base <0a0
  license: BSD-3-Clause
  license_family: BSD
  size: 7925462
  timestamp: 1732314760363
- conda: https://conda.anaconda.org/conda-forge/linux-64/openssl-3.4.1-h7b32b05_0.conda
  sha256: cbf62df3c79a5c2d113247ddea5658e9ff3697b6e741c210656e239ecaf1768f
  md5: 41adf927e746dc75ecf0ef841c454e48
  depends:
  - __glibc >=2.17,<3.0.a0
  - ca-certificates
  - libgcc >=13
  license: Apache-2.0
  license_family: Apache
  size: 2939306
  timestamp: 1739301879343
- conda: https://conda.anaconda.org/conda-forge/linux-64/pandas-2.2.3-py39h3b40f6f_2.conda
  sha256: 5cf1fd8e385f9a2a2cd5bff29a22fbcca6b2107d17efb7f8dd5bea8b158bc3a1
  md5: 8fbcaa8f522b0d2af313db9e3b4b05b9
  depends:
  - __glibc >=2.17,<3.0.a0
  - libgcc >=13
  - libstdcxx >=13
  - numpy >=1.19,<3
  - numpy >=1.22.4
  - python >=3.9,<3.10.0a0
  - python-dateutil >=2.8.1
  - python-tzdata >=2022a
  - python_abi 3.9.* *_cp39
  - pytz >=2020.1,<2024.2
  constrains:
  - openpyxl >=3.1.0
  - zstandard >=0.19.0
  - numexpr >=2.8.4
  - pyxlsb >=1.0.10
  - lxml >=4.9.2
  - tzdata >=2022.7
  - qtpy >=2.3.0
  - bottleneck >=1.3.6
  - pyqt5 >=5.15.8
  - matplotlib >=3.6.3
  - numba >=0.56.4
  - beautifulsoup4 >=4.11.2
  - pytables >=3.8.0
  - xarray >=2022.12.0
  - s3fs >=2022.11.0
  - tabulate >=0.9.0
  - xlsxwriter >=3.0.5
  - psycopg2 >=2.9.6
  - sqlalchemy >=2.0.0
  - gcsfs >=2022.11.0
  - pyreadstat >=1.2.0
  - fastparquet >=2022.12.0
  - blosc >=1.21.3
  - scipy >=1.10.0
  - fsspec >=2022.11.0
  license: BSD-3-Clause
  license_family: BSD
  size: 12897186
  timestamp: 1736811288486
- conda: https://conda.anaconda.org/conda-forge/linux-64/python-3.9.21-h9c0c6dc_1_cpython.conda
  build_number: 1
  sha256: 06042ce946a64719b5ce1676d02febc49a48abcab16ef104e27d3ec11e9b1855
  md5: b4807744af026fdbe8c05131758fb4be
  depends:
  - __glibc >=2.17,<3.0.a0
  - bzip2 >=1.0.8,<2.0a0
  - ld_impl_linux-64 >=2.36.1
  - libffi >=3.4,<4.0a0
  - libgcc >=13
  - liblzma >=5.6.3,<6.0a0
  - libnsl >=2.0.1,<2.1.0a0
  - libsqlite >=3.47.0,<4.0a0
  - libuuid >=2.38.1,<3.0a0
  - libxcrypt >=4.4.36
  - libzlib >=1.3.1,<2.0a0
  - ncurses >=6.5,<7.0a0
  - openssl >=3.4.0,<4.0a0
  - readline >=8.2,<9.0a0
  - tk >=8.6.13,<8.7.0a0
  - tzdata
  constrains:
  - python_abi 3.9.* *_cp39
  license: Python-2.0
  size: 23622848
  timestamp: 1733407924273
- conda: https://conda.anaconda.org/conda-forge/noarch/python-dateutil-2.9.0.post0-pyhff2d567_1.conda
  sha256: a50052536f1ef8516ed11a844f9413661829aa083304dc624c5925298d078d79
  md5: 5ba79d7c71f03c678c8ead841f347d6e
  depends:
  - python >=3.9
  - six >=1.5
  license: Apache-2.0
  license_family: APACHE
  size: 222505
  timestamp: 1733215763718
- conda: https://conda.anaconda.org/conda-forge/noarch/python-tzdata-2025.1-pyhd8ed1ab_0.conda
  sha256: 1597d6055d34e709ab8915091973552a0b8764c8032ede07c4e99670da029629
  md5: 392c91c42edd569a7ec99ed8648f597a
  depends:
  - python >=3.9
  license: Apache-2.0
  license_family: APACHE
  size: 143794
  timestamp: 1737541204030
- conda: https://conda.anaconda.org/conda-forge/linux-64/python_abi-3.9-5_cp39.conda
  build_number: 5
  sha256: 019e2f8bca1d1f1365fbb9965cd95bb395c92c89ddd03165db82f5ae89a20812
  md5: 40363a30db350596b5f225d0d5a33328
  constrains:
  - python 3.9.* *_cpython
  license: BSD-3-Clause
  license_family: BSD
  size: 6193
  timestamp: 1723823354399
- conda: https://conda.anaconda.org/conda-forge/noarch/pytz-2024.1-pyhd8ed1ab_0.conda
  sha256: 1a7d6b233f7e6e3bbcbad054c8fd51e690a67b129a899a056a5e45dd9f00cb41
  md5: 3eeeeb9e4827ace8c0c1419c85d590ad
  depends:
  - python >=3.7
  license: MIT
  license_family: MIT
  size: 188538
  timestamp: 1706886944988
- conda: https://conda.anaconda.org/conda-forge/linux-64/readline-8.2-h8c095d6_2.conda
  sha256: 2d6d0c026902561ed77cd646b5021aef2d4db22e57a5b0178dfc669231e06d2c
  md5: 283b96675859b20a825f8fa30f311446
  depends:
  - libgcc >=13
  - ncurses >=6.5,<7.0a0
  license: GPL-3.0-only
  license_family: GPL
  size: 282480
  timestamp: 1740379431762
- conda: https://conda.anaconda.org/conda-forge/noarch/six-1.17.0-pyhd8ed1ab_0.conda
  sha256: 41db0180680cc67c3fa76544ffd48d6a5679d96f4b71d7498a759e94edc9a2db
  md5: a451d576819089b0d672f18768be0f65
  depends:
  - python >=3.9
  license: MIT
  license_family: MIT
  size: 16385
  timestamp: 1733381032766
- conda: https://conda.anaconda.org/conda-forge/linux-64/tk-8.6.13-noxft_h4845f30_101.conda
  sha256: e0569c9caa68bf476bead1bed3d79650bb080b532c64a4af7d8ca286c08dea4e
  md5: d453b98d9c83e71da0741bb0ff4d76bc
  depends:
  - libgcc-ng >=12
  - libzlib >=1.2.13,<2.0.0a0
  license: TCL
  license_family: BSD
  size: 3318875
  timestamp: 1699202167581
- conda: https://conda.anaconda.org/conda-forge/noarch/tzdata-2025a-h78e105d_0.conda
  sha256: c4b1ae8a2931fe9b274c44af29c5475a85b37693999f8c792dad0f8c6734b1de
  md5: dbcace4706afdfb7eb891f7b37d07c04
  license: LicenseRef-Public-Domain
  size: 122921
  timestamp: 1737119101255
    "#;
        let lockfile = LockFile::new(content.to_string());
        let dependencies = lockfile.get_dependencies(None).unwrap();

        assert_eq!(
            dependencies,
            vec![
                "numpy==2.0.2; python_version >= '3.9' and python_version < '3.10.0a0'",
                "pandas==2.2.3; python_version >= '3.9' and python_version < '3.10.0a0'",
                "python-dateutil==2.9.0.post0; python_version >= '3.9'",
                "python-tzdata==2025.1; python_version >= '3.9'",
                "pytz==2024.1; python_version >= '3.7'",
                "six==1.17.0; python_version >= '3.9'",
            ]
        );
    }
}
