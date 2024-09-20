use pest::Parser;
use pest_derive::Parser;
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::package::Package;
use crate::util::name_to_key;
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
            _ => Err(format!("Unknown operator: {}", s).into()),
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
        };
        write!(f, "{}", op_str)
    }
}

fn url_trim(mut input: String) -> String {
    input = input.trim().to_string();
    if input.starts_with('@') {
        input.remove(0);
        input = input.trim().to_string();
    }
    input
}

// Dependency Specfication: A model of a specification of one or more versions, such as "numpy>1.18,<2.0". At this time the parsing does is not complete and thus parsing errors are mostly ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DepSpec {
    pub(crate) name: String,
    pub(crate) key: String,
    pub(crate) url: Option<String>,
    operators: Vec<DepOperator>,
    versions: Vec<VersionSpec>,
}
impl DepSpec {
    // Given a URL to a whl file, parse the name and version and return a DepSpec
    fn from_whl(input: &str) -> Result<Self, String> {
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
                    operators: operators,
                    versions: versions,
                });
            }
        }
        return Err("Invalid .whl".to_string());
    }

    pub(crate) fn from_string(input: &str) -> Result<Self, String> {
        if let Ok(ds) = DepSpec::from_whl(input) {
            return Ok(ds);
        }

        let mut parsed = DepSpecParser::parse(Rule::name_req, input)
            .map_err(|e| format!("Parsing error: {}", e))?;

        let parse_result = parsed.next().ok_or("Parsing error: No results")?;
        // check for unconsumed input
        if parse_result.as_str() != input {
            return Err(format!(
                "Unrecognized input: {:?}",
                input[parse_result.as_str().len()..].to_string()
            ));
        }

        let mut package_name = None;
        let mut url = None;
        let mut operators = Vec::new();
        let mut versions = Vec::new();

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
                            return Err("Expected version_cmp".to_string());
                        }
                        let op = op_pair
                            .as_str()
                            .trim()
                            .parse::<DepOperator>()
                            .map_err(|e| format!("Invalid operator: {}", e))?;
                        // version
                        let version_pair =
                            inner_pairs.next().ok_or("Expected version")?;
                        if version_pair.as_rule() != Rule::version {
                            return Err("Expected version".to_string());
                        }
                        let version = version_pair.as_str().trim().to_string();

                        operators.push(op);
                        versions.push(VersionSpec::new(&version));
                    }
                }
                _ => {}
            }
        }
        let package_name = package_name.ok_or("Missing package name")?;
        let key = name_to_key(&package_name);
        // if url is defined and it is wheel, take definition from the wheel
        if let Some(ref url) = url {
            if let Ok(ds) = DepSpec::from_whl(&url) {
                if ds.key != key {
                    return Err(format!(
                        "Provided name {} does not match whl name {}",
                        ds.name, package_name
                    ));
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
        })
    }
    pub(crate) fn from_package(
        package: &Package,
        operator: DepOperator,
    ) -> Result<Self, String> {
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
        })
    }
    // TODO: from_dep_specs: if all have the same name, combine operators and versions?

    //--------------------------------------------------------------------------
    pub(crate) fn validate_version(&self, version: &VersionSpec) -> bool {
        // operators and versions are always the same length
        for (op, spec_version) in self.operators.iter().zip(&self.versions) {
            let valid = match op {
                DepOperator::LessThan => version < spec_version,
                DepOperator::LessThanOrEq => version <= spec_version,
                DepOperator::Eq => version == spec_version,
                DepOperator::NotEq => version != spec_version,
                DepOperator::GreaterThan => version > spec_version,
                DepOperator::GreaterThanOrEq => version >= spec_version,
                DepOperator::Compatible => version.is_compatible(spec_version),
                DepOperator::ArbitraryEq => version.is_arbitrary_equal(spec_version),
            };
            if !valid {
                return false;
            }
        }
        true
    }

    pub(crate) fn validate_url(&self, package: &Package) -> bool {
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

    pub(crate) fn validate_package(&self, package: &Package) -> bool {
        self.key == package.key
            && self.validate_version(&package.version)
            && self.validate_url(&package)
    }
}

impl fmt::Display for DepSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        for (op, ver) in self.operators.iter().zip(self.versions.iter()) {
            parts.push(format!("{}{}", op, ver));
        }
        write!(f, "{}{}", self.name, parts.join(","))
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
    }
    #[test]
    fn test_dep_spec_b() {
        let input = "package[foo]>=0.2; python_version < '2.7'";
        let ds1 = DepSpec::from_string(input).unwrap();
        // println!("{:?}", ds1);
        assert_eq!(ds1.name, "package");
        assert_eq!(ds1.operators[0], DepOperator::GreaterThanOrEq);
        assert_eq!(ds1.versions[0], VersionSpec::new("0.2"));
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
        assert_eq!(ds1.to_string(), "foo");
    }
    #[test]
    fn test_dep_spec_h2() {
        let ds1 =
            DepSpec::from_string("package-two@git+https://github.com/owner/repo@41b95ec")
                .unwrap();
        assert_eq!(ds1.to_string(), "package-two");
    }
    #[test]
    fn test_dep_spec_h3() {
        let ds1 =
            DepSpec::from_string("package-four @ git+ssh://example.com/owner/repo@main")
                .unwrap();
        assert_eq!(ds1.to_string(), "package-four");
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
        assert_eq!(ds1.to_string(), "pip");
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_dep_spec_validate_version_a() {
        let input = "package>0.2,<2.0";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.name, "package");
        assert_eq!(ds1.validate_version(&VersionSpec::new("0.3")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("0.2")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("0.2.1")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.2")), false);
    }
    #[test]
    fn test_dep_spec_validate_version_b() {
        let input = "package>0.2,<2.0";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.0.1")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.0.0")), false);
        assert_eq!(
            ds1.validate_version(&VersionSpec::new("1.9.99.99999")),
            true
        );
    }
    #[test]
    fn test_dep_spec_validate_version_c() {
        let input = "package>=2.0,<=3.0";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.0")), true);
        assert_eq!(
            ds1.validate_version(&VersionSpec::new("1.9.99.99999")),
            false
        );
        assert_eq!(ds1.validate_version(&VersionSpec::new("3.0")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_d() {
        let input = "package==2.*";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.4")), true);
        assert_eq!(
            ds1.validate_version(&VersionSpec::new("1.9.99.99999")),
            false
        );
        assert_eq!(ds1.validate_version(&VersionSpec::new("3.0")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.3")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_e() {
        let input =
            "requests [security,tests] >= 2.8.1, == 2.8.*, < 3; python_version < '2.7'";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.8.1")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.2.1")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.8.0")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.8.99")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_f() {
        let input = "name>=3,<2";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("2")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("3")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("4")), false);
    }
    #[test]
    fn test_dep_spec_validate_version_g() {
        let input = "name==1.1.post1";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.post1")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.*")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_h() {
        let input = "name==1.1a1";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1a1")), true);
        // this is supposed to match...
        // assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.*")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_i() {
        let input = "name==1.1";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.0")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.0.0")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.dev1")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1a1")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.post1")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.*")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_j1() {
        let input = "name===12";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("12")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_j2() {
        let input = "name===12++";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("12++")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_k() {
        let input = "name";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("foo++")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_l1() {
        let input = "name==1.*,<1.10";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.0")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.9")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.1")), false);
    }
    #[test]
    fn test_dep_spec_validate_version_l2() {
        let input = "name<1.10,==1.*";
        let ds1 = DepSpec::from_string(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.0")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.9")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.1")), false);
    }
    //--------------------------------------------------------------------------
    #[test]
    fn test_dep_spec_validate_package_a() {
        let p1 = Package::from_name_version_durl("package", "1.0", None).unwrap();
        let ds1 = DepSpec::from_string("package>0.5,<1.5").unwrap();
        assert_eq!(ds1.validate_package(&p1), true);
    }
    #[test]
    fn test_dep_spec_validate_package_b() {
        let p1 = Package::from_name_version_durl("package", "1.5", None).unwrap();
        let ds1 = DepSpec::from_string("package>0.5,<1.5").unwrap();
        assert_eq!(ds1.validate_package(&p1), false);
    }
    #[test]
    fn test_dep_spec_validate_package_c() {
        let p1 = Package::from_name_version_durl("package", "1.0", None).unwrap();
        let ds1 = DepSpec::from_string("package>0.5,<1.5,!=1.0").unwrap();
        assert_eq!(ds1.validate_package(&p1), false);
    }
    #[test]
    fn test_dep_spec_validate_package_d() {
        let p1 = Package::from_name_version_durl("package", "1.0.0.0.1", None).unwrap();
        let ds1 = DepSpec::from_string("package>0.5,<1.5,!=1.0").unwrap();
        assert_eq!(ds1.validate_package(&p1), true);
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
        assert_eq!(ds.to_string(), "SomeProject");
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

        assert_eq!(ds1.to_string(), "static-frame"); // we get no version
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
        // from pip3 install "git+ssh://git@github.com/uqfoundation/dill.git@0.3.8"
        let ds1 = DepSpec::from_string(
            "dill @ git+ssh://git@github.com/uqfoundation/dill.git@0.3.8",
        )
        .unwrap();

        assert_eq!(ds1.to_string(), "dill"); // we get no version
        assert_eq!(
            ds1.url.clone().unwrap(),
            "git+ssh://git@github.com/uqfoundation/dill.git@0.3.8"
        );

        let json_str = r#"
            {"url": "ssh://git@github.com/uqfoundation/dill.git", "vcs_info": {"commit_id": "a0a8e86976708d0436eec5c8f7d25329da727cb5", "requested_revision": "0.3.8", "vcs": "git"}}
            "#;

        let durl: DirectURL = serde_json::from_str(json_str).unwrap();
        let p = Package::from_name_version_durl("dill", "0.3.8", Some(durl)).unwrap();
        assert!(ds1.validate_package(&p));
    }
    //--------------------------------------------------------------------------
    #[test]
    fn test_dep_spec_json_a() {
        let ds = DepSpec::from_whl("https://example.com/app-1.0.whl").unwrap();
        let json = serde_json::to_string(&ds).unwrap();
        assert_eq!(json, "{\"name\":\"app\",\"key\":\"app\",\"url\":\"https://example.com/app-1.0.whl\",\"operators\":[\"Eq\"],\"versions\":[[{\"Number\":1},{\"Number\":0}]]}")
    }

}
