use crate::util::toml_to_py_marker;
use crate::util::ResultDynError;
use toml::Value;

fn poetry_toml_value_to_string(
    (name, value, marker): (&String, &toml::Value, Option<&String>),
) -> String {
    let version = match value {
        toml::Value::String(v) => v.clone(),
        toml::Value::Table(t) => t
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    };
    if let Some(em) = marker {
        format!("{}{}; {}", name, version, em)
    } else {
        format!("{}{}", name, version)
    }
}

#[derive(Debug)]
pub(crate) struct PyProjectInfo {
    parsed: Value,
    has_project_dep: bool,
    has_project_dep_optional: bool,
    has_poetry_dep: bool,
    has_poetry_dep_group: bool,
}

impl PyProjectInfo {
    pub(crate) fn new(content: &str) -> ResultDynError<Self> {
        // let parsed: Value = toml::from_str(&content)?;
        let parsed: toml::Value = content.parse::<toml::Value>()?;

        let has_project_dep = parsed
            .get("project")
            .and_then(|t| t.get("dependencies"))
            .is_some();

        let has_project_dep_optional = parsed
            .get("project")
            .and_then(|t| t.get("optional-dependencies"))
            .is_some();

        let has_poetry_dep = parsed
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|t| t.get("dependencies"))
            .is_some();

        let has_poetry_dep_group = parsed
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("group"))
            .and_then(|groups| groups.as_table())
            .is_some_and(|groups| {
                groups
                    .values()
                    .any(|group| group.get("dependencies").is_some())
            });
        if has_project_dep_optional && has_poetry_dep_group {
            return Err("Cannot define optional dependencies in both project and tool.poetry.group".into());
        }
        Ok(Self {
            parsed,
            has_project_dep,
            has_project_dep_optional,
            has_poetry_dep,
            has_poetry_dep_group,
        })
    }

    //--------------------------------------------------------------------------
    fn get_project_dep(&self) -> ResultDynError<Vec<String>> {
        if let Some(dependencies) = self
            .parsed
            .get("project")
            .and_then(|project| project.get("dependencies"))
            .and_then(|deps| deps.as_array())
        {
            Ok(dependencies
                .iter()
                .filter_map(|dep| dep.as_str().map(String::from))
                .collect::<Vec<_>>())
        } else {
            Err("Could not extract from toml project.dependencies".into())
        }
    }

    /// Extracts `[project.optional-dependencies.<key>]`
    fn get_project_dep_optional(&self, key: &str) -> ResultDynError<Vec<String>> {
        if let Some(optional_deps) = self
            .parsed
            .get("project")
            .and_then(|project| project.get("optional-dependencies"))
            .and_then(|opt_deps| opt_deps.get(key))
            .and_then(|deps| deps.as_array())
        {
            Ok(optional_deps
                .iter()
                .filter_map(|dep| dep.as_str().map(String::from))
                .collect::<Vec<_>>())
        } else {
            Err(format!(
                "Could not extract from toml project.optional-dependencies.{}",
                key
            )
            .into())
        }
    }

    /// Extracts `[tool.poetry.dependencies]`
    fn get_poetry_dep(&self) -> ResultDynError<Vec<String>> {
        if let Some(dependencies) = self
            .parsed
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|poetry| poetry.get("dependencies"))
            .and_then(|deps| deps.as_table())
        {
            // get one value or None
            let py_em =
                toml_to_py_marker(&toml::Value::Table(dependencies.clone()), "python")
                    .into_iter()
                    .next();

            Ok(dependencies
                .iter()
                .filter(|(name, _)| *name != "python")
                .map(|(name, value)| {
                    poetry_toml_value_to_string((name, value, py_em.as_ref()))
                })
                .collect::<Vec<_>>())
        } else {
            Err("Could not extract from toml tool.poetry.dependencies".into())
        }
    }

    /// Extracts `[tool.poetry.group.<key>.dependencies]`
    fn get_poetry_dep_group(&self, key: &str) -> ResultDynError<Vec<String>> {
        if let Some(dependencies) = self
            .parsed
            .get("tool")
            .and_then(|t| t.get("poetry"))
            .and_then(|p| p.get("group"))
            .and_then(|groups| groups.get(key))
            .and_then(|group| group.get("dependencies"))
            .and_then(|deps| deps.as_table())
        {
            Ok(dependencies
                .iter()
                .map(|(name, value)| poetry_toml_value_to_string((name, value, None)))
                .collect::<Vec<_>>())
        } else {
            Err(format!(
                "Could not extract from toml tool.poetry.group.{}.dependencies",
                key
            )
            .into())
        }
    }

    //--------------------------------------------------------------------------
    // Public interface to get all dependencies, including poetry defined dependencies.
    pub(crate) fn get_dependencies(
        &self,
        options: Option<&Vec<String>>,
    ) -> ResultDynError<Vec<String>> {
        let mut deps_list: Vec<String> = Vec::new();

        // if present, always sources
        // [project.dependencies]: take for project and poetry
        if self.has_project_dep {
            deps_list.extend(self.get_project_dep().unwrap());
        }

        // only this or [tool.poetry.group.*.dependencies] is set
        // [project.optional-dependencies]
        if self.has_project_dep_optional {
            if let Some(opt) = options {
                for o in opt {
                    deps_list.extend(self.get_project_dep_optional(o)?);
                }
            }
        }
        // if present, always source
        // [tool.poetry.dependencies]
        if self.has_poetry_dep {
            deps_list.extend(self.get_poetry_dep().unwrap());
        }

        // only this or [project.optional-dependencies] is set
        // [tool.poetry.group.*.dependencies]
        if self.has_poetry_dep_group {
            if let Some(opt) = options {
                for o in opt {
                    deps_list.extend(self.get_poetry_dep_group(o)?);
                }
            }
        }
        Ok(deps_list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pyprojectinfo_new_a() {
        let contents = r#"
        [project]
        dependencies = ["requests", "numpy"]
        optional-dependencies = { dev = ["pytest", "black"] }

        [tool.poetry]
        dependencies = { flask = "^2.0.0" }

        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");
        assert!(info.has_project_dep);
        assert!(info.has_project_dep_optional);
        assert!(info.has_poetry_dep);
        assert!(!info.has_poetry_dep_group);
    }

    #[test]
    fn test_pyprojectinfo_new_b() {
        let contents = r#"
        [project]
        dependencies = ["requests", "numpy"]
        optional-dependencies = { dev = ["pytest", "black"] }

        [tool.poetry]
        dependencies = { flask = "^2.0.0" }

        [tool.poetry.group.test.dependencies]
        pytest = "^6.2.5"
        "#
        .to_string();
        // cannot define both optional and group
        let info = PyProjectInfo::new(&contents);
        assert!(info.is_err())
    }

    #[test]
    fn test_detects_project_dependencies() {
        let contents = r#"
        [project]
        dependencies = ["requests", "numpy"]
        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");
        assert!(info.has_project_dep);
        assert!(!info.has_project_dep_optional);
        assert!(!info.has_poetry_dep);
        assert!(!info.has_poetry_dep_group);
    }

    #[test]
    fn test_detects_optional_project_dependencies() {
        let contents = r#"
        [project]
        optional-dependencies = { dev = ["pytest", "black"] }
        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");
        assert!(!info.has_project_dep);
        assert!(info.has_project_dep_optional);
        assert!(!info.has_poetry_dep);
        assert!(!info.has_poetry_dep_group);
    }

    #[test]
    fn test_detects_poetry_dependencies() {
        let contents = r#"
        [tool.poetry]
        dependencies = { requests = "^2.25.1", numpy = "^1.21.0" }
        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");
        assert!(!info.has_project_dep);
        assert!(!info.has_project_dep_optional);
        assert!(info.has_poetry_dep);
        assert!(!info.has_poetry_dep_group);
    }

    #[test]
    fn test_detects_poetry_dependency_groups() {
        let contents = r#"
        [tool.poetry.group.dev.dependencies]
        pytest = "^6.2.5"
        black = "^21.7b0"

        [tool.poetry.group.docs.dependencies]
        sphinx = "^4.0.0"
        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");
        assert!(!info.has_project_dep);
        assert!(!info.has_project_dep_optional);
        assert!(!info.has_poetry_dep);
        assert!(info.has_poetry_dep_group);
    }

    #[test]
    fn test_no_dependencies_detected() {
        let contents = r#"
        [build-system]
        requires = ["setuptools", "wheel"]
        build-backend = "setuptools.build_meta"
        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");
        assert!(!info.has_project_dep);
        assert!(!info.has_project_dep_optional);
        assert!(!info.has_poetry_dep);
        assert!(!info.has_poetry_dep_group);
    }
    //--------------------------------------------------------------------------

    #[test]
    fn test_get_project_dep_a() {
        let contents = r#"
        [project]
        dependencies = ["requests", "numpy"]
        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");
        assert_eq!(info.get_project_dep().unwrap(), vec!["requests", "numpy"]);
    }

    #[test]
    fn test_get_project_dep_b_empty() {
        let contents = r#"
        [project]
        dependencies = []
        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");
        assert_eq!(info.get_project_dep().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn test_get_project_dep_optional_a() {
        let contents = r#"
        [project.optional-dependencies]
        dev = ["pytest", "black"]
        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");
        assert_eq!(
            info.get_project_dep_optional("dev").unwrap(),
            vec!["pytest", "black"]
        );
    }

    #[test]
    fn test_get_project_dep_optional_b_missing_key() {
        let contents = r#"
        [project.optional-dependencies]
        dev = ["pytest"]
        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");
        assert!(info.get_project_dep_optional("docs").is_err());
    }

    #[test]
    fn test_get_poetry_dep_a() {
        let contents = r#"
        [tool.poetry.dependencies]
        requests = "^2.25.1"
        cachecontrol = { version = "==0.14.0", extras = ["filecache"] }
        flask = ">=2.0.0"
        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");

        let mut actual_deps = info.get_poetry_dep().unwrap();
        let mut expected_deps = vec![
            "requests^2.25.1".to_string(),
            "cachecontrol==0.14.0".to_string(),
            "flask>=2.0.0".to_string(),
        ];
        actual_deps.sort();
        expected_deps.sort();
        assert_eq!(actual_deps, expected_deps);
    }

    #[test]
    fn test_get_poetry_dep_b_empty() {
        let contents = r#"
        [tool.poetry]
        dependencies = {}
        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");
        assert_eq!(info.get_poetry_dep().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn test_get_poetry_dep_group_a() {
        let contents = r#"
        [tool.poetry.group.dev.dependencies]
        black = "^21.7b0"
        pytest = "==6.2.5"
        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");
        assert_eq!(
            info.get_poetry_dep_group("dev").unwrap(),
            vec!["black^21.7b0", "pytest==6.2.5"]
        );
    }

    #[test]
    fn test_get_poetry_dep_group_b_missing_group() {
        let contents = r#"
        [tool.poetry.group.docs.dependencies]
        sphinx = "^4.0.0"
        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");
        assert!(info.get_poetry_dep_group("dev").is_err());
    }

    #[test]
    fn test_get_poetry_dep_group_c_empty() {
        let contents = r#"
        [tool.poetry.group.dev]
        dependencies = {}
        "#
        .to_string();

        let info = PyProjectInfo::new(&contents).expect("Failed to parse toml");
        assert_eq!(info.get_poetry_dep_group("dev").unwrap().len(), 0);
    }
}
