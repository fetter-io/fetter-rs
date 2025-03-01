use crate::util::ResultDynError;
use serde_json::Value as JsonValue;
use toml::Value as TomlValue; // Use the `toml` crate for parsing

#[derive(Debug, PartialEq)]
enum LockFileType {
    Requirements, // requirements.txt style, used by uv pip and pip-tools
    UvLock,       // uv.lock native TOML style
    Poetry,       // poetry.lock
    PipfileLock,  // made with Pipenv Pipfile.lock
    PEP751,       // pep still under consideration
}

#[derive(Debug)]
pub(crate) struct LockFile {
    file_type: LockFileType,
    content: String,
}

impl LockFile {
    pub(crate) fn new(content: String) -> Self {
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
                    dependencies.push(format!("{}=={}", name, version));
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
                    dependencies.push(format!("{}=={}", name, version));
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

    /// Extracts dependency specifications from the lock file.
    pub(crate) fn get_dependencies(
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
            vec!["attrs==23.2.0", "cattrs==23.2.3", "numpy==2.0.1"]
        );
    }
}
