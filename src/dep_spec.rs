use pest::Parser;
use pest_derive::Parser;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::env_marker::marker_eval;
use crate::env_marker::EnvMarkerExpr;
use crate::env_marker::EnvMarkerState;
use crate::package::Package;
use crate::util::name_to_key;
use crate::util::url_strip_user;
use crate::util::url_trim;
use crate::util::ResultDynError;
use crate::version_spec::VersionSpec;

// This is a grammar for https://packaging.python.org/en/latest/specifications/dependency-specifiers/
#[derive(Parser)]
#[grammar = "dep_spec.pest"]
struct DepSpecParser;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum DepOperator {
    LessThan,
    LessThanOrEq,
    Eq,
    NotEq,
    GreaterThan,
    GreaterThanOrEq,
    Compatible,
    ArbitraryEq,
    Caret,
    Tilde,
}

impl FromStr for DepOperator {
    type Err = Box<dyn Error>;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "<" => Ok(DepOperator::LessThan),
            "<=" => Ok(DepOperator::LessThanOrEq),
            "==" => Ok(DepOperator::Eq),
            "!=" => Ok(DepOperator::NotEq),
            ">" => Ok(DepOperator::GreaterThan),
            ">=" => Ok(DepOperator::GreaterThanOrEq),
            "~=" => Ok(DepOperator::Compatible),
            "===" => Ok(DepOperator::ArbitraryEq),
            "^" => Ok(DepOperator::Caret),
            "~" => Ok(DepOperator::Tilde),
            _ => Err(format!("Unknown operator: {s}").into()),
        }
    }
}

impl fmt::Display for DepOperator {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let op_str = match self {
            DepOperator::LessThan => "<",
            DepOperator::LessThanOrEq => "<=",
            DepOperator::Eq => "==",
            DepOperator::NotEq => "!=",
            DepOperator::GreaterThan => ">",
            DepOperator::GreaterThanOrEq => ">=",
            DepOperator::Compatible => "~=",
            DepOperator::ArbitraryEq => "===",
            DepOperator::Caret => "^",
            DepOperator::Tilde => "~",
        };
        write!(f, "{op_str}")
    }
}

fn extract_marker_component(pair: pest::iterators::Pair<Rule>) -> String {
    let s = pair.as_str().trim();
    if (s.starts_with('"') && s.ends_with('"'))
        || (s.starts_with('\'') && s.ends_with('\''))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

// Collect all environment marker expression into structured binary expressions
fn extract_marker_expr(
    pair: pest::iterators::Pair<Rule>,
    exprs: &mut HashMap<String, EnvMarkerExpr>,
) {
    match pair.as_rule() {
        Rule::marker_expr => {
            let mut inner_pairs = pair.clone().into_inner();

            // contains only one parenthesized marker_or or marker_and
            if inner_pairs.len() == 1 {
                let inner = inner_pairs.next().unwrap();
                if matches!(inner.as_rule(), Rule::marker_or | Rule::marker_and) {
                    extract_marker_expr(inner, exprs);
                    return;
                }
            }
            let left = inner_pairs.next().map(extract_marker_component);
            let operator = inner_pairs.next().map(extract_marker_component);
            let right = inner_pairs.next().map(extract_marker_component);
            if let (Some(left), Some(operator), Some(right)) = (left, operator, right) {
                exprs.insert(
                    // normalize all whitespace to one space
                    pair.as_str()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                    EnvMarkerExpr {
                        left,
                        operator,
                        right,
                    },
                );
            }
        }
        Rule::marker_or | Rule::marker_and | Rule::marker => {
            for inner in pair.into_inner() {
                extract_marker_expr(inner, exprs);
            }
        }
        _ => {}
    }
}

// Dependency Specification: A model of a specification for one package with pairs of versions and operators, such as "numpy>1.18,<2.0". The length of `operators` and `versions` must always be the same.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DepSpec {
    pub name: String,
    pub key: String,
    pub url: Option<String>,
    operators: Vec<DepOperator>, // use to_spec() to get version, operators as a String
    versions: Vec<VersionSpec>,
    pub(crate) env_marker: String,
    pub(crate) env_marker_expr: Option<HashMap<String, EnvMarkerExpr>>,
}

impl DepSpec {
    /// Given a URL to a whl file, parse the name and version and return a DepSpec
    fn from_whl(input: &str) -> ResultDynError<Self> {
        let input = input.trim();
        if input.starts_with("http://")
            || input.starts_with("https://")
            || input.starts_with("file://") && input.ends_with(".whl")
        {
            // extract the last path component
            let name = Path::new(input)
                .file_stem()
                .ok_or_else(|| "Invalid .whl".to_string())?
                .to_str()
                .unwrap();

            let parts: Vec<_> = name.split('-').collect();
            if parts.len() >= 2 {
                let package_name = parts[0].to_string();
                let versions = vec![VersionSpec::new(parts[1])];
                let operators = vec![DepOperator::Eq];
                return Ok(DepSpec {
                    key: name_to_key(&package_name),
                    name: package_name,
                    url: Some(input.to_string()),
                    operators,
                    versions,
                    env_marker: String::new(),
                    env_marker_expr: None,
                });
            }
        }
        Err("Invalid .whl".into())
    }

    /// Given a string as found in a requirements.txt or similar, create a DepSpec.
    pub fn from_string(input: &str) -> ResultDynError<Self> {
        if let Ok(ds) = DepSpec::from_whl(input) {
            return Ok(ds);
        }
        let mut parsed = DepSpecParser::parse(Rule::name_req, input).map_err(
            |e| -> Box<dyn std::error::Error> { format!("Parsing error: {e}").into() },
        )?;

        let parse_result = parsed.next().ok_or("Parsing error: No results")?;
        // check for unconsumed input
        if parse_result.as_str() != input {
            return Err(format!(
                "Unrecognized input: {:?}",
                input[parse_result.as_str().len()..].to_string()
            )
            .into());
        }

        let mut package_name = None;
        let mut url = None;
        let mut operators = Vec::new();
        let mut versions = Vec::new();
        let mut env_marker = String::new();
        let mut env_marker_expr: Option<HashMap<String, EnvMarkerExpr>> = None;

        let inner_pairs: Vec<_> = parse_result.into_inner().collect();
        for pair in inner_pairs {
            match pair.as_rule() {
                Rule::identifier => {
                    // grammar permits only one
                    package_name = Some(pair.as_str().to_string());
                }
                Rule::url_reference => {
                    url = Some(url_trim(pair.as_str().to_string()));
                }
                Rule::version_many => {
                    for version_pair in pair.into_inner() {
                        let mut inner_pairs = version_pair.into_inner();
                        // operator
                        let op_pair = inner_pairs.next().ok_or("Expected version_cmp")?;
                        if op_pair.as_rule() != Rule::version_cmp {
                            return Err("Expected version_cmp".into());
                        }
                        let op = op_pair.as_str().trim().parse::<DepOperator>().map_err(
                            |e| -> Box<dyn std::error::Error> {
                                format!("Invalid operator: {e}").into()
                            },
                        )?;
                        // version
                        let version_pair =
                            inner_pairs.next().ok_or("Expected version")?;
                        if version_pair.as_rule() != Rule::version {
                            return Err("Expected version".into());
                        }
                        let version = version_pair.as_str().trim().to_string();

                        operators.push(op);
                        versions.push(VersionSpec::new(&version));
                    }
                }
                Rule::quoted_marker => {
                    // normalize all whitespace to one space
                    env_marker = pair
                        .as_str()
                        .trim_start_matches(';')
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !env_marker.is_empty() {
                        // only create hashmap if we hae content
                        let mut me: HashMap<String, EnvMarkerExpr> = HashMap::new();
                        for inner_pair in pair.into_inner() {
                            extract_marker_expr(inner_pair, &mut me);
                        }
                        env_marker_expr = Some(me);
                    }
                }
                _ => {}
            }
        }
        let package_name = package_name.ok_or("Missing package name")?;
        let key = name_to_key(&package_name);
        // if url is defined and it is wheel, take definition from the wheel
        if let Some(ref url) = url {
            if let Ok(ds) = DepSpec::from_whl(url) {
                if ds.key != key {
                    return Err(format!(
                        "Provided name {} does not match whl name {}",
                        ds.name, package_name
                    )
                    .into());
                }
                return Ok(ds);
            }
        }
        Ok(DepSpec {
            name: package_name,
            key,
            url,
            operators,
            versions,
            env_marker,
            env_marker_expr,
        })
    }
    /// Create a DepSpec from a Package struct.
    pub(crate) fn from_package(
        package: &Package,
        operator: DepOperator,
    ) -> ResultDynError<Self> {
        let mut operators = Vec::new();
        let mut versions = Vec::new();
        operators.push(operator);
        versions.push(package.version.clone());
        Ok(DepSpec {
            name: package.name.clone(),
            key: package.key.clone(),
            url: None,
            operators,
            versions,
            env_marker: String::new(),
            env_marker_expr: None,
        })
    }

    // pub(crate) fn from_dep_specs(dep_specs: Vec<&DepSpec>) -> ResultDynError<Self> {
    //     let mut names = HashSet::new();
    //     let mut keys = HashSet::new();
    //     let mut operators = Vec::new();
    //     let mut versions = Vec::new();
    //     for ds in &dep_specs {
    //         names.insert(&ds.name);
    //         keys.insert(&ds.key);
    //         operators.extend(ds.operators.iter().cloned());
    //         versions.extend(ds.versions.iter().cloned());
    //     }
    //     if keys.len() == 1 {
    //         let name = names.iter().next().unwrap();
    //         let key = keys.iter().next().unwrap();
    //         return Ok(DepSpec {
    //             name: name.to_string(),
    //             key: key.to_string(),
    //             url: None,
    //             operators,
    //             versions,
    //             env_marker: String::new(), // if these are defined, can we merge?
    //             env_marker_expr: None,
    //         });
    //     }
    //     Err(format!("Unreconcilable dependency specifiers: {:?}", dep_specs).into())
    // }

    //--------------------------------------------------------------------------
    pub fn validate_version(&self, version: &VersionSpec) -> bool {
        // operators and versions are always the same length
        for (op, spec_version) in self.operators.iter().zip(&self.versions) {
            let valid = match op {
                DepOperator::LessThan => version < spec_version,
                DepOperator::LessThanOrEq => version <= spec_version,
                DepOperator::Eq => version == spec_version,
                DepOperator::NotEq => version != spec_version,
                DepOperator::GreaterThan => version > spec_version,
                DepOperator::GreaterThanOrEq => version >= spec_version,
                DepOperator::Compatible => spec_version.is_compatible(version),
                DepOperator::ArbitraryEq => spec_version.is_arbitrary_equal(version),
                DepOperator::Caret => spec_version.is_caret(version),
                DepOperator::Tilde => spec_version.is_tilde(version),
            };
            if !valid {
                return false;
            }
        }
        true
    }

    fn validate_url(&self, package: &Package) -> bool {
        // if the DepSpec has a URL (the requirements specfied a URL) we have to validate that the installed package has a direct url.
        if let Some(url) = &self.url {
            if let Some(durl) = &package.direct_url {
                // compare this url to package.direct_url
                return durl.validate(url);
            }
            // Package does not have durl data
            return false;
        }
        true
    }

    //--------------------------------------------------------------------------
    // public validators

    // Primary public interfaced for validation
    pub fn validate_package(&self, package: &Package) -> bool {
        self.key == package.key
            && self.validate_version(&package.version)
            && self.validate_url(package)
    }

    // Given an EnvMarkerState, determine if this DepSpec is applied on this envirionment.
    pub fn validate_env_marker(&self, ems: &EnvMarkerState) -> bool {
        if let Some(me) = &self.env_marker_expr {
            return marker_eval(&self.env_marker, me, ems).unwrap();
        }
        true
    }

    //--------------------------------------------------------------------------

    /// If the DepSpec specifies an exact version, return that VersionSpec. We assume this only applyes to "==" dependencies, not arbitrary equal "===" dependencies
    pub fn get_exact(&self) -> Option<VersionSpec> {
        match self.versions.len() == 1
            && !self.versions[0].has_wildcard()
            && self.operators.len() == 1
            && self.operators[0] == DepOperator::Eq
        {
            true => Some(self.versions[0].clone()),
            false => None,
        }
    }

    /// Return the dependency specification; either the version or URL as string
    pub fn to_spec(&self) -> String {
        let marker = match self.env_marker.is_empty() {
            true => "".to_string(),
            false => format!("; {}", &self.env_marker),
        };
        // if we have versions, we do not need URL
        if !self.versions.is_empty() {
            let mut parts = Vec::new();
            for (op, ver) in self.operators.iter().zip(self.versions.iter()) {
                parts.push(format!("{op}{ver}"));
            }
            format!("{}{}", parts.join(","), marker)
        } else if let Some(url) = &self.url {
            format!("{}{}", url_strip_user(url), marker)
        } else {
            marker
        }
    }
}

impl fmt::Display for DepSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let marker = match self.env_marker.is_empty() {
            true => "".to_string(),
            false => format!("; {}", &self.env_marker),
        };
        let mut parts = Vec::new();
        // if we have versions, we do not need URL
        if !self.versions.is_empty() {
            for (op, ver) in self.operators.iter().zip(self.versions.iter()) {
                parts.push(format!("{op}{ver}"));
            }
            write!(f, "{}{}{}", self.name, parts.join(","), marker)
        } else if let Some(url) = &self.url {
            write!(f, "{} @ {}{}", self.name, url_strip_user(url), marker)
        } else {
            write!(f, "{}{}", self.name, marker)
        }
    }
}

//------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::package_durl::DirectURL;

    use super::*;

    #[test]
    fn test_dep_spec_a() {
        let input = "package>=0.2,<0.3";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.name, "package");
        assert_eq!(ds1.operators[0], DepOperator::GreaterThanOrEq);
        assert_eq!(ds1.operators[1], DepOperator::LessThan);
        assert_eq!(ds1.to_spec(), ">=0.2,<0.3");
    }
    #[test]
    fn test_dep_spec_b() {
        let input = "package[foo]>=0.2; python_version < '2.7'";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.name, "package");
        assert_eq!(ds1.operators[0], DepOperator::GreaterThanOrEq);
        assert_eq!(ds1.versions[0], VersionSpec::new("0.2"));
        assert_eq!(ds1.to_spec(), ">=0.2; python_version < '2.7'");
    }
    #[test]
    fn test_dep_spec_c() {
        let input = "package==0.2<=";
        assert!(DepSpec::from_string(input).is_err());
    }
    #[test]
    fn test_dep_spec_d() {
        let input = "==0.2<=";
        assert!(DepSpec::from_string(input).is_err());
    }
    #[test]
    fn test_dep_spec_e() {
        assert!(DepSpec::from_string("foo+==3").is_err());
    }
    #[test]
    fn test_dep_spec_f() {
        let ds1 = DepSpec::from_string("   foo==3    ").unwrap();
        assert_eq!(ds1.versions[0], VersionSpec::new("3"));
        assert_eq!(ds1.to_string(), "foo==3");
    }

    #[test]
    fn test_dep_spec_g() {
        let ds1 = DepSpec::from_string("   foo==3 ,  <  4  ,  != 3.5   ").unwrap();
        // assert_eq!(ds1.versions[0], VersionSpec::new("3    "));
        assert_eq!(ds1.to_string(), "foo==3,<4,!=3.5");
    }

    #[test]
    fn test_dep_spec_h1() {
        let ds1 = DepSpec::from_string(
            "foo @ git+https://xxxxxxxxxx:x-xx-xx@xx.com/xxxx/xxxx.git@xxxxxx",
        )
        .unwrap();
        assert_eq!(
            ds1.to_string(),
            "foo @ git+https://xx.com/xxxx/xxxx.git@xxxxxx"
        );
        assert_eq!(ds1.to_spec(), "git+https://xx.com/xxxx/xxxx.git@xxxxxx");
    }
    #[test]
    fn test_dep_spec_h2() {
        let ds1 =
            DepSpec::from_string("package-two@git+https://github.com/owner/repo@41b95ec")
                .unwrap();
        assert_eq!(
            ds1.to_string(),
            "package-two @ git+https://github.com/owner/repo@41b95ec"
        );
    }
    #[test]
    fn test_dep_spec_h3() {
        let ds1 =
            DepSpec::from_string("package-four @ git+ssh://example.com/owner/repo@main")
                .unwrap();
        assert_eq!(
            ds1.to_string(),
            "package-four @ git+ssh://example.com/owner/repo@main"
        );
    }
    #[test]
    fn test_dep_spec_h4() {
        let ds1 =
            DepSpec::from_string("pip @ file:///localbuilds/pip-1.3.1-py33-none-any.whl")
                .unwrap();
        assert_eq!(ds1.to_string(), "pip==1.3.1");
        assert_eq!(
            ds1.url.unwrap(),
            "file:///localbuilds/pip-1.3.1-py33-none-any.whl"
        );
    }
    #[test]
    fn test_dep_spec_h5() {
        let ds1 = DepSpec::from_string("pip @ https://github.com/pypa/pip/archive/1.3.1.zip#sha1=da9234ee9982d4bbb3c72346a6de940a148ea686").unwrap();
        assert_eq!(ds1.to_string(), "pip @ https://github.com/pypa/pip/archive/1.3.1.zip#sha1=da9234ee9982d4bbb3c72346a6de940a148ea686");
    }

    #[test]
    fn test_dep_spec_i1() {
        let ds1 = DepSpec::from_string("foo >= 3.4; os_name == 'posix' and platform_python_implementation == 'CPython' and   implementation_name ==   'cpython' ").unwrap();
        assert_eq!(ds1.to_string(), "foo>=3.4; os_name == 'posix' and platform_python_implementation == 'CPython' and implementation_name == 'cpython'");
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_dep_spec_validate_version_a() {
        let input = "package>0.2,<2.0";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.name, "package");
        assert!(ds1.validate_version(&VersionSpec::new("0.3")));
        assert!(!ds1.validate_version(&VersionSpec::new("0.2")));
        assert!(ds1.validate_version(&VersionSpec::new("0.2.1")));
        assert!(!ds1.validate_version(&VersionSpec::new("2.2")));
    }
    #[test]
    fn test_dep_spec_validate_version_b() {
        let input = "package>0.2,<2.0";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert!(!ds1.validate_version(&VersionSpec::new("2.0.1")));
        assert!(!ds1.validate_version(&VersionSpec::new("2.0.0")));
        assert!(ds1.validate_version(&VersionSpec::new("1.9.99.99999")));
    }
    #[test]
    fn test_dep_spec_validate_version_c() {
        let input = "package>=2.0,<=3.0";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert!(ds1.validate_version(&VersionSpec::new("2.0")));
        assert!(!ds1.validate_version(&VersionSpec::new("1.9.99.99999")),);
        assert!(ds1.validate_version(&VersionSpec::new("3.0")));
    }
    #[test]
    fn test_dep_spec_validate_version_d() {
        let input = "package==2.*";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert!(ds1.validate_version(&VersionSpec::new("2.4")));
        assert!(!ds1.validate_version(&VersionSpec::new("1.9.99.99999")),);
        assert!(!ds1.validate_version(&VersionSpec::new("3.0")));
        assert!(ds1.validate_version(&VersionSpec::new("2.3")));
    }
    #[test]
    fn test_dep_spec_validate_version_e() {
        let input =
            "requests [security,tests] >= 2.8.1, == 2.8.*, < 3; python_version < '2.7'";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert!(ds1.validate_version(&VersionSpec::new("2.8.1")));
        assert!(!ds1.validate_version(&VersionSpec::new("2.2.1")));
        assert!(!ds1.validate_version(&VersionSpec::new("2.8.0")));
        assert!(ds1.validate_version(&VersionSpec::new("2.8.99")));
    }
    #[test]
    fn test_dep_spec_validate_version_f() {
        let input = "name>=3,<2";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert!(!ds1.validate_version(&VersionSpec::new("2")));
        assert!(!ds1.validate_version(&VersionSpec::new("3")));
        assert!(!ds1.validate_version(&VersionSpec::new("4")));
    }
    #[test]
    fn test_dep_spec_validate_version_g() {
        let input = "name==1.1.post1";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert!(!ds1.validate_version(&VersionSpec::new("1.1")));
        assert!(ds1.validate_version(&VersionSpec::new("1.1.post1")));
        assert!(ds1.validate_version(&VersionSpec::new("1.1.*")));
    }
    #[test]
    fn test_dep_spec_validate_version_h() {
        let input = "name==1.1a1";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert!(!ds1.validate_version(&VersionSpec::new("1.1")));
        assert!(ds1.validate_version(&VersionSpec::new("1.1a1")));
        // this is supposed to match...
        // assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.*")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_i() {
        let input = "name==1.1";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert!(ds1.validate_version(&VersionSpec::new("1.1")));
        assert!(ds1.validate_version(&VersionSpec::new("1.1.0")));
        assert!(ds1.validate_version(&VersionSpec::new("1.1.0.0")));
        assert!(!ds1.validate_version(&VersionSpec::new("1.1.dev1")));
        assert!(!ds1.validate_version(&VersionSpec::new("1.1a1")));
        assert!(!ds1.validate_version(&VersionSpec::new("1.1.post1")));
        assert!(ds1.validate_version(&VersionSpec::new("1.1.*")));
    }
    #[test]
    fn test_dep_spec_validate_version_j1() {
        let input = "name===12";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert!(ds1.validate_version(&VersionSpec::new("12")));
    }
    #[test]
    fn test_dep_spec_validate_version_j2() {
        let input = "name===12++";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert!(ds1.validate_version(&VersionSpec::new("12++")));
    }
    #[test]
    fn test_dep_spec_validate_version_k() {
        let input = "name";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert!(ds1.validate_version(&VersionSpec::new("foo++")));
    }
    #[test]
    fn test_dep_spec_validate_version_l1() {
        let input = "name==1.*,<1.10";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert!(ds1.validate_version(&VersionSpec::new("1.0")));
        assert!(ds1.validate_version(&VersionSpec::new("1.9")));
        assert!(!ds1.validate_version(&VersionSpec::new("2.1")));
    }
    #[test]
    fn test_dep_spec_validate_version_l2() {
        let input = "name<1.10,==1.*";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert!(ds1.validate_version(&VersionSpec::new("1.0")));
        assert!(ds1.validate_version(&VersionSpec::new("1.9")));
        assert!(!ds1.validate_version(&VersionSpec::new("2.1")));
    }

    #[test]
    fn test_dep_spec_validate_version_m() {
        let input = "name";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert!(ds1.validate_version(&VersionSpec::new("1.0")));
        assert!(ds1.validate_version(&VersionSpec::new("100.0.0.0")));
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_dep_spec_validate_package_a() {
        let p1 = Package::from_name_version_durl("package", "1.0", None).unwrap();
        let ds1 = DepSpec::from_string("package>0.5,<1.5").unwrap();
        assert!(ds1.validate_package(&p1));
    }
    #[test]
    fn test_dep_spec_validate_package_b() {
        let p1 = Package::from_name_version_durl("package", "1.5", None).unwrap();
        let ds1 = DepSpec::from_string("package>0.5,<1.5").unwrap();
        assert!(!ds1.validate_package(&p1));
    }
    #[test]
    fn test_dep_spec_validate_package_c() {
        let p1 = Package::from_name_version_durl("package", "1.0", None).unwrap();
        let ds1 = DepSpec::from_string("package>0.5,<1.5,!=1.0").unwrap();
        assert!(!ds1.validate_package(&p1));
    }
    #[test]
    fn test_dep_spec_validate_package_d() {
        let p1 = Package::from_name_version_durl("package", "1.0.0.0.1", None).unwrap();
        let ds1 = DepSpec::from_string("package>0.5,<1.5,!=1.0").unwrap();
        assert!(ds1.validate_package(&p1));
    }
    #[test]
    fn test_dep_spec_validate_package_e1() {
        let ds1 = DepSpec::from_string("package~=1.0").unwrap();
        let p1 = Package::from_name_version_durl("package", "1.9", None).unwrap();
        assert!(ds1.validate_package(&p1));
        let p2 = Package::from_name_version_durl("package", "0.1", None).unwrap();
        assert!(!ds1.validate_package(&p2));
        let p3 = Package::from_name_version_durl("package", "2.1", None).unwrap();
        assert!(!ds1.validate_package(&p3));
    }
    #[test]
    fn test_dep_spec_validate_package_e2() {
        let ds1 = DepSpec::from_string("package~=5.2.0").unwrap();
        let p1 = Package::from_name_version_durl("package", "5.2.8", None).unwrap();
        assert!(ds1.validate_package(&p1));
        let p2 = Package::from_name_version_durl("package", "5.3", None).unwrap();
        assert!(!ds1.validate_package(&p2));
        let p3 = Package::from_name_version_durl("package", "4.9", None).unwrap();
        assert!(!ds1.validate_package(&p3));
    }
    #[test]
    fn test_dep_spec_validate_package_e3() {
        let ds1 = DepSpec::from_string("package~=5.2.4.6").unwrap();
        let p1 = Package::from_name_version_durl("package", "5.2.4.6", None).unwrap();
        assert!(ds1.validate_package(&p1));
        let p2 = Package::from_name_version_durl("package", "5.2.4.8", None).unwrap();
        assert!(ds1.validate_package(&p2));
        let p3 = Package::from_name_version_durl("package", "5.2.5", None).unwrap();
        assert!(!ds1.validate_package(&p3));
    }
    #[test]
    fn test_dep_spec_validate_package_f1() {
        let ds1 = DepSpec::from_string("package~2.3").unwrap();
        let p1 = Package::from_name_version_durl("package", "1.9", None).unwrap();
        assert!(!ds1.validate_package(&p1));
        let p2 = Package::from_name_version_durl("package", "2.3.1", None).unwrap();
        assert!(ds1.validate_package(&p2));
        let p3 = Package::from_name_version_durl("package", "2.4", None).unwrap();
        assert!(!ds1.validate_package(&p3));
    }
    #[test]
    fn test_dep_spec_validate_package_f2() {
        let ds1 = DepSpec::from_string("package~2").unwrap();
        let p1 = Package::from_name_version_durl("package", "1.9", None).unwrap();
        assert!(!ds1.validate_package(&p1));
        let p2 = Package::from_name_version_durl("package", "2.3.1", None).unwrap();
        assert!(ds1.validate_package(&p2));
        let p3 = Package::from_name_version_durl("package", "2.4", None).unwrap();
        assert!(ds1.validate_package(&p3));
    }
    #[test]
    fn test_dep_spec_validate_package_g1() {
        let ds1 = DepSpec::from_string("package^2.3").unwrap();
        let p1 = Package::from_name_version_durl("package", "1.9", None).unwrap();
        assert!(!ds1.validate_package(&p1));
        let p2 = Package::from_name_version_durl("package", "2.3.1", None).unwrap();
        assert!(ds1.validate_package(&p2));
        let p3 = Package::from_name_version_durl("package", "2.9", None).unwrap();
        assert!(ds1.validate_package(&p3));
        let p4 = Package::from_name_version_durl("package", "3.1", None).unwrap();
        assert!(!ds1.validate_package(&p4));
    }
    #[test]
    fn test_dep_spec_validate_package_g2() {
        let ds1 = DepSpec::from_string("package^0.2.3").unwrap();
        let p1 = Package::from_name_version_durl("package", "1.9", None).unwrap();
        assert!(!ds1.validate_package(&p1));
        let p2 = Package::from_name_version_durl("package", "0.3.1", None).unwrap();
        assert!(!ds1.validate_package(&p2));
        let p3 = Package::from_name_version_durl("package", "0.2.9", None).unwrap();
        assert!(ds1.validate_package(&p3));
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_dep_spec_to_string_a() {
        let ds1 = DepSpec::from_string("package  >=0.2,  <0.3   ").unwrap();
        assert_eq!(ds1.to_string(), "package>=0.2,<0.3");
    }
    #[test]
    fn test_dep_spec_to_string_b() {
        let ds1 = DepSpec::from_string("requests [security,tests] >= 2.8.1, == 2.8.* ")
            .unwrap();
        assert_eq!(ds1.to_string(), "requests>=2.8.1,==2.8.*");
    }
    //--------------------------------------------------------------------------
    #[test]
    fn test_dep_spec_from_package_a() {
        let p = Package::from_name_version_durl("foo", "1.2.3.4", None).unwrap();
        let ds = DepSpec::from_package(&p, DepOperator::Eq).unwrap();
        assert_eq!(ds.to_string(), "foo==1.2.3.4");
    }
    #[test]
    fn test_dep_spec_from_package_b() {
        let p = Package::from_name_version_durl("foo", "1.2.3.4", None).unwrap();
        let ds = DepSpec::from_package(&p, DepOperator::GreaterThan).unwrap();
        assert_eq!(ds.to_string(), "foo>1.2.3.4");
    }
    #[test]
    fn test_dep_spec_from_package_c() {
        let p = Package::from_name_version_durl("foo", "1.2.3.4", None).unwrap();
        let ds = DepSpec::from_package(&p, DepOperator::LessThanOrEq).unwrap();
        assert_eq!(ds.to_string(), "foo<=1.2.3.4");
    }
    //--------------------------------------------------------------------------
    #[test]
    fn test_dep_spec_url_a() {
        let ds =
            DepSpec::from_string("SomeProject@git+https://git.repo/some_pkg.git@1.3.1")
                .unwrap();
        assert_eq!(
            ds.to_string(),
            "SomeProject @ git+https://git.repo/some_pkg.git@1.3.1"
        );
        assert_eq!(ds.url.unwrap(), "git+https://git.repo/some_pkg.git@1.3.1")
    }
    #[test]
    fn test_dep_spec_url_b() {
        let ds = DepSpec::from_string("https://example.com/app-1.0.whl").unwrap();
        assert_eq!(ds.to_string(), "app==1.0");
        assert_eq!(ds.url.unwrap(), "https://example.com/app-1.0.whl");
    }
    #[test]
    fn test_dep_spec_url_c() {
        let ds = DepSpec::from_string("http://example.com/app-1.0.whl").unwrap();
        assert_eq!(ds.to_string(), "app==1.0");
        assert_eq!(ds.url.unwrap(), "http://example.com/app-1.0.whl");
    }
    #[test]
    fn test_dep_spec_url_d() {
        let ds = DepSpec::from_string(
            "foo @ http://foo/package/foo-3.1.4/foo-3.1.4-py3-none-any.whl",
        )
        .unwrap();
        assert_eq!(ds.to_string(), "foo==3.1.4");
        assert_eq!(
            ds.url.unwrap(),
            "http://foo/package/foo-3.1.4/foo-3.1.4-py3-none-any.whl"
        );
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_dep_spec_from_whl_a() {
        let ds = DepSpec::from_whl("https://example.com/app-1.0.whl").unwrap();
        assert_eq!(ds.to_string(), "app==1.0");
        assert_eq!(ds.url.unwrap(), "https://example.com/app-1.0.whl")
    }
    #[test]
    fn test_dep_spec_from_whl_b() {
        let ds = DepSpec::from_whl("http://example.com/app-1.0.whl").unwrap();
        assert_eq!(ds.to_string(), "app==1.0");
        assert_eq!(ds.url.unwrap(), "http://example.com/app-1.0.whl")
    }
    #[test]
    fn test_dep_spec_from_whl_c() {
        let ds = DepSpec::from_whl("file:///a/b/c/app-2.0.whl").unwrap();
        assert_eq!(ds.to_string(), "app==2.0");
        assert_eq!(ds.url.unwrap(), "file:///a/b/c/app-2.0.whl")
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_dep_spec_validate_url_a() {
        let ds1 = DepSpec::from_string("https://files.pythonhosted.org/packages/5d/01/a4e76fc45b9352d6b762c6452172584b0be0006bd745e4e2a561b2972b28/static_frame-2.13.0-py3-none-any.whl").unwrap();
        // note: the DepSpec discovers the package name with an underscore
        assert_eq!(ds1.to_string(), "static_frame==2.13.0");
        assert_eq!(ds1.url.clone().unwrap(), "https://files.pythonhosted.org/packages/5d/01/a4e76fc45b9352d6b762c6452172584b0be0006bd745e4e2a561b2972b28/static_frame-2.13.0-py3-none-any.whl");

        // while we can install/require from the hyphen, the .dist-info file will always have an underscore
        let durl = DirectURL::from_url_vcs_cid("https://files.pythonhosted.org/packages/5d/01/a4e76fc45b9352d6b762c6452172584b0be0006bd745e4e2a561b2972b28/static_frame-2.13.0-py3-none-any.whl".to_string(), None, None).unwrap();

        let p1 = Package::from_name_version_durl("static_frame", "2.13.0", Some(durl))
            .unwrap();
        assert!(ds1.validate_package(&p1));

        let ds2 = DepSpec::from_string("static-frame==2.13.0").unwrap();
        assert!(ds2.validate_package(&p1));
    }

    #[test]
    fn test_dep_spec_validate_url_b() {
        // this will use the currently defined version in setup.py in authoring the entry in site-packages
        let ds1 = DepSpec::from_string("static-frame @ git+https://github.com/static-frame/static-frame.git@454d8d5446b71eceb57935b5ea9ba4efb051210e").unwrap();

        assert_eq!(ds1.to_string(), "static-frame @ git+https://github.com/static-frame/static-frame.git@454d8d5446b71eceb57935b5ea9ba4efb051210e"); // we get no version
        assert_eq!(ds1.url.clone().unwrap(), "git+https://github.com/static-frame/static-frame.git@454d8d5446b71eceb57935b5ea9ba4efb051210e");

        // even without a version in the depspec, the observed package will have a version, which is why we need to check durl
        let p1 = Package::from_name_version_durl("static_frame", "2.13.0", None).unwrap();
        assert!(!ds1.validate_package(&p1)); // this fails without durl

        let durl = DirectURL::from_url_vcs_cid(
            "https://github.com/static-frame/static-frame.git".to_string(),
            Some("git".to_string()),
            Some("454d8d5446b71eceb57935b5ea9ba4efb051210e".to_string()),
        )
        .unwrap();
        let p2 = Package::from_name_version_durl("static_frame", "2.13.0", Some(durl))
            .unwrap();
        assert!(ds1.validate_package(&p2));
    }

    #[test]
    fn test_dep_spec_validate_url_c() {
        // from pip3 install "dill @ git+ssh://git@github.com/uqfoundation/dill.git@0.3.8"
        let ds1 = DepSpec::from_string(
            "dill @ git+ssh://git@github.com/uqfoundation/dill.git@0.3.8",
        )
        .unwrap();

        assert_eq!(
            ds1.to_string(),
            "dill @ git+ssh://github.com/uqfoundation/dill.git@0.3.8"
        ); // we get no version
        assert_eq!(
            ds1.url.clone().unwrap(),
            "git+ssh://git@github.com/uqfoundation/dill.git@0.3.8"
        );

        let json_str = r#"
            {"url": "ssh://git@github.com/uqfoundation/dill.git", "vcs_info": {"commit_id": "a0a8e86976708d0436eec5c8f7d25329da727cb5", "revision": "0.3.8", "vcs": "git"}}
            "#;

        let durl: DirectURL = serde_json::from_str(json_str).unwrap();
        let p = Package::from_name_version_durl("dill", "0.3.8", Some(durl)).unwrap();
        assert!(ds1.validate_package(&p));
    }

    // NOTE: parser cannot handle git+ssh without starting with "package @"
    // #[test]
    // fn test_dep_spec_validate_url_d() {
    //     // from pip3 install "git+ssh://git@github.com/uqfoundation/dill.git@0.3.8"
    //     let ds1 = DepSpec::from_string(
    //         "git+ssh://git@github.com/uqfoundation/dill.git@0.3.8",
    //     )
    //     .unwrap();
    // }

    //--------------------------------------------------------------------------
    #[test]
    fn test_dep_spec_json_a() {
        let ds = DepSpec::from_whl("https://example.com/app-1.0.whl").unwrap();
        let json = serde_json::to_string(&ds).unwrap();
        assert_eq!(json, "{\"name\":\"app\",\"key\":\"app\",\"url\":\"https://example.com/app-1.0.whl\",\"operators\":[\"Eq\"],\"versions\":[\"1.0\"],\"env_marker\":\"\",\"env_marker_expr\":null}")
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_dep_spec_env_marker_a() {
        let input =
            "requests [security,tests] >= 2.8.1, == 2.8.*, < 3; python_version < '2.7'";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(format!("{:?}", ds1.versions), "[VersionSpec([Number(2), Number(8), Number(1)]), VersionSpec([Number(2), Number(8), Text(\"*\")]), VersionSpec([Number(3)])]");
        assert_eq!(
            format!("{:?}", ds1.operators),
            "[GreaterThanOrEq, Eq, LessThan]"
        );
        assert_eq!(ds1.env_marker, "python_version < '2.7'");
        assert_eq!(format!("{:?}", ds1.env_marker_expr.unwrap()), "{\"python_version < '2.7'\": EnvMarkerExpr { left: \"python_version\", operator: \"<\", right: \"2.7\" }}");
    }

    #[test]
    fn test_dep_spec_env_marker_b() {
        let input = "foo >= 3.4 ; python_version < '2.7.9' or (python_version >= '3.0' and python_version < '3.4')";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.env_marker, "python_version < '2.7.9' or (python_version >= '3.0' and python_version < '3.4')");
        // assert_eq!(ds1.env_marker_expr.len(), 3);

        let mut keys: Vec<String> = ds1
            .env_marker_expr
            .as_ref()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "python_version < '2.7.9'",
                "python_version < '3.4'",
                "python_version >= '3.0'"
            ]
        );
    }

    #[test]
    fn test_dep_spec_env_marker_c() {
        let input = "foo >= 3.4 ;(python_version > '2.0' and python_version < '2.7.9') or (python_version >= '3.0' and python_version < '3.4')";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.env_marker, "(python_version > '2.0' and python_version < '2.7.9') or (python_version >= '3.0' and python_version < '3.4')");
        assert_eq!(ds1.env_marker_expr.as_ref().unwrap().len(), 4);

        let mut keys: Vec<String> =
            ds1.env_marker_expr.unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "python_version < '2.7.9'",
                "python_version < '3.4'",
                "python_version > '2.0'",
                "python_version >= '3.0'"
            ]
        );
    }

    #[test]
    fn test_dep_spec_env_marker_d() {
        let input = "foo >= 3.4 ;(python_version > '2.0' and python_version < '2.7.9') or python_version >= '3.0'";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.env_marker, "(python_version > '2.0' and python_version < '2.7.9') or python_version >= '3.0'");
        assert_eq!(ds1.env_marker_expr.as_ref().unwrap().len(), 3);

        let mut keys: Vec<String> =
            ds1.env_marker_expr.unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "python_version < '2.7.9'",
                "python_version > '2.0'",
                "python_version >= '3.0'"
            ]
        );
    }

    //--------------------------------------------------------------------------
    fn get_ems_darwin() -> EnvMarkerState {
        EnvMarkerState::from_str(
            "posix", "darwin", "arm64", "CPython", "23.1.0", "Darwin", "3.13", "3.13.1",
            "cpython",
        )
    }

    #[test]
    fn test_dep_spec_validate_env_marker_a1() {
        let input = "foo >= 3.4 ;(python_version > '2.0' and python_version < '2.7.9') or python_version >= '3.0'";
        let ds1 = DepSpec::from_string(input).unwrap();

        let em = get_ems_darwin();
        assert!(ds1.validate_env_marker(&em));
    }

    #[test]
    fn test_dep_spec_validate_env_marker_a2() {
        let input = "foo >= 3.4 ;(python_version > '2.0' and python_version < '2.7.9') or python_version < '3.12'";
        let ds1 = DepSpec::from_string(input).unwrap();

        let em = get_ems_darwin();
        assert!(!ds1.validate_env_marker(&em));
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_dep_spec_is_exact_a() {
        let ds1 = DepSpec::from_string("package>=0.2,<0.3").unwrap();
        assert!(ds1.get_exact().is_none());

        let ds2 = DepSpec::from_string("package>=0.2").unwrap();
        assert!(ds2.get_exact().is_none());

        let ds3 = DepSpec::from_string("package!=0.2").unwrap();
        assert!(ds3.get_exact().is_none());

        let ds4 = DepSpec::from_string("package~=0.2").unwrap();
        assert!(ds4.get_exact().is_none());

        let ds5 = DepSpec::from_string("package").unwrap();
        assert!(ds5.get_exact().is_none());
    }

    #[test]
    fn test_dep_spec_is_exact_b() {
        let ds1 = DepSpec::from_string("package==0.2").unwrap();
        assert!(ds1.get_exact().is_some());

        let ds2 = DepSpec::from_string("package==3").unwrap();
        assert!(ds2.get_exact().is_some());
    }
}
