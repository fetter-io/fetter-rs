use std::error::Error;
use std::fmt;
use std::str::FromStr;

use pest::Parser;
use pest_derive::Parser;

use crate::package::Package;
use crate::version_spec::VersionSpec;

// This is a grammar for https://packaging.python.org/en/latest/specifications/dependency-specifiers/
#[derive(Parser)]
#[grammar = "dep_spec.pest"]
struct DepSpecParser;

#[derive(Debug, Clone, PartialEq)]
enum DepOperator {
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

// Dependency Specfication: A model of a specification of one or more versions, such as "numpy>1.18,<2.0". At this time the parsing does is not complete and thus parsing errors are mostly ignored.
#[derive(Debug, Clone)]
pub(crate) struct DepSpec {
    pub(crate) name: String,
    operators: Vec<DepOperator>,
    versions: Vec<VersionSpec>,
}
impl DepSpec {
    pub fn from_string(input: &str) -> Result<Self, String> {
        let mut parsed = DepSpecParser::parse(Rule::name_req, input)
            .map_err(|e| format!("Parsing error: {}", e))?;

        // check for unconsumed input
        let parse_result = parsed.next().ok_or("Parsing error: No results")?;

        if parse_result.as_str() != input {
            return Err(format!(
                "Unrecognized input: {:?}",
                input[parse_result.as_str().len()..].to_string()
            ));
        }

        let mut package_name = None;
        let mut operators = Vec::new();
        let mut versions = Vec::new();

        let inner_pairs: Vec<_> = parse_result.into_inner().collect();
        for pair in inner_pairs {
            match pair.as_rule() {
                Rule::identifier => {
                    // grammar permits only one
                    package_name = Some(pair.as_str().to_string());
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
                        let version_pair = inner_pairs.next().ok_or("Expected version")?;
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
        Ok(DepSpec {
            name: package_name,
            operators,
            versions,
        })
    }
    pub fn from_package(package: &Package) -> Result<Self, String> {
        let mut operators = Vec::new();
        let mut versions = Vec::new();
        operators.push(DepOperator::Eq);
        versions.push(package.version.clone());
        Ok(DepSpec {
            name: package.name.clone(),
            operators,
            versions,
        })
    }

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
                DepOperator::Compatible => version.is_compatible(spec_version),
                DepOperator::ArbitraryEq => version.is_arbitrary_equal(spec_version),
            };
            if !valid {
                return false;
            }
        }
        true
    }
    pub fn validate_package(&self, package: &Package) -> bool {
        self.name == package.name && self.validate_version(&package.version)
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
        let ds1 = DepSpec::from_string("foo @ git+https://xxxxxxxxxx:x-xx-xx@xx.com/xxxx/xxxx.git@xxxxxx")
            .unwrap();
        assert_eq!(ds1.to_string(), "foo");
    }
    #[test]
    fn test_dep_spec_h2() {
        let ds1 = DepSpec::from_string("package-two@git+https://github.com/owner/repo@41b95ec").unwrap();
        assert_eq!(ds1.to_string(), "package-two");
    }
    #[test]
    fn test_dep_spec_h3() {
        let ds1 = DepSpec::from_string("package-four @ git+ssh://example.com/owner/repo@main").unwrap();
        assert_eq!(ds1.to_string(), "package-four");
    }
    #[test]
    fn test_dep_spec_h4() {
        let ds1 = DepSpec::from_string("pip @ file:///localbuilds/pip-1.3.1-py33-none-any.whl").unwrap();
        assert_eq!(ds1.to_string(), "pip");
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
        let input = "requests [security,tests] >= 2.8.1, == 2.8.*, < 3; python_version < '2.7'";
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
        let p1 = Package::from_name_and_version("package", "1.0").unwrap();
        let ds1 = DepSpec::from_string("package>0.5,<1.5").unwrap();
        assert_eq!(ds1.validate_package(&p1), true);
    }
    #[test]
    fn test_dep_spec_validate_package_b() {
        let p1 = Package::from_name_and_version("package", "1.5").unwrap();
        let ds1 = DepSpec::from_string("package>0.5,<1.5").unwrap();
        assert_eq!(ds1.validate_package(&p1), false);
    }
    #[test]
    fn test_dep_spec_validate_package_c() {
        let p1 = Package::from_name_and_version("package", "1.0").unwrap();
        let ds1 = DepSpec::from_string("package>0.5,<1.5,!=1.0").unwrap();
        assert_eq!(ds1.validate_package(&p1), false);
    }
    #[test]
    fn test_dep_spec_validate_package_d() {
        let p1 = Package::from_name_and_version("package", "1.0.0.0.1").unwrap();
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
        let ds1 = DepSpec::from_string("requests [security,tests] >= 2.8.1, == 2.8.* ").unwrap();
        assert_eq!(ds1.to_string(), "requests>=2.8.1,==2.8.*");
    }
    //--------------------------------------------------------------------------
    #[test]
    fn test_dep_spec_from_package_a() {
        let p = Package::from_name_and_version("foo", "1.2.3.4").unwrap();
        let ds = DepSpec::from_package(&p).unwrap();
        assert_eq!(ds.to_string(), "foo==1.2.3.4");
    }

}
