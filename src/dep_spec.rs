use std::str::FromStr;
use std::fmt;
use std::error::Error;

use pest::Parser;
use pest_derive::Parser;

use crate::version_spec::VersionSpec;
// use crate::version_spec::VersionPart;

// This is a redimentary grammar for https://packaging.python.org/en/latest/specifications/dependency-specifiers/
#[derive(Parser)]
#[grammar = "dep_spec.pest"]
struct DepSpecParser;

#[derive(Debug, PartialEq)]
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

// Dependency Specfication
#[derive(Debug)]
pub(crate) struct DepSpec {
    pub(crate) name: String,
    operators: Vec<DepOperator>,
    versions: Vec<VersionSpec>,
}

impl DepSpec {

    pub fn new(input: &str) -> Result<Self, String> {
        let mut parsed = DepSpecParser::parse(Rule::name_req, input)
            .map_err(|e| format!("Parsing error: {}", e))?
            .next()
            .ok_or("Parsing error: No results")?
            .into_inner();

        let mut package_name = None;
        let mut operators = Vec::new();
        let mut versions = Vec::new();

        while let Some(pair) = parsed.next() {
            match pair.as_rule() {
                Rule::identifier => { // grammar permits only one
                    package_name = Some(pair.as_str().to_string());
                }
                Rule::version_many => {
                    for version_pair in pair.into_inner() {
                        let mut inner_pairs = version_pair.into_inner();
                        // get operator
                        let op_pair = inner_pairs.next().ok_or("Expected version_cmp")?;
                        if op_pair.as_rule() != Rule::version_cmp {
                            return Err("Expected version_cmp".to_string());
                        }
                        let op = op_pair.as_str().parse::<DepOperator>().map_err(|e| e.to_string())?;
                        // get version
                        let version_pair = inner_pairs.next().ok_or("Expected version")?;
                        if version_pair.as_rule() != Rule::version {
                            return Err("Expected version".to_string());
                        }
                        let version = version_pair.as_str().to_string();

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
    pub fn validate_version(&self, version: &VersionSpec) -> bool {
        for (op, spec_version) in self.operators.iter().zip(&self.versions) {
            // println!("{:?} spec_version {:?} version {:?}", op, spec_version, version);
            let is_compatible = match op {
                DepOperator::LessThan => version < spec_version,
                DepOperator::LessThanOrEq => version <= spec_version,
                DepOperator::Eq => version == spec_version,
                DepOperator::NotEq => version != spec_version,
                DepOperator::GreaterThan => version > spec_version,
                DepOperator::GreaterThanOrEq => version >= spec_version,
                DepOperator::Compatible => version.is_compatible(spec_version),
                DepOperator::ArbitraryEq => version.is_arbitrary_equal(spec_version),
            };
            if !is_compatible {
                return false;
            }
        }
        true
    }
    pub(crate) fn to_string(&self) -> String {
        let mut parts = Vec::new();
        for (op, ver) in self.operators.iter().zip(self.versions.iter()) {
            parts.push(format!("{}{}", op.to_string(), ver.to_string()));
        }
        format!("{} {}", self.name, parts.join(", "))
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dep_spec_a() {
        let input = "package>=0.2,<0.3";
        let ds1 = DepSpec::new(input).unwrap();
        assert_eq!(ds1.name, "package");
        assert_eq!(ds1.operators[0], DepOperator::GreaterThanOrEq);
        assert_eq!(ds1.operators[1], DepOperator::LessThan);
    }
    #[test]
    fn test_dep_spec_b() {
        let input = "package[foo]>=0.2; python_version < '2.7'";
        let ds1 = DepSpec::new(input).unwrap();
        // println!("{:?}", ds1);
        assert_eq!(ds1.name, "package");
        assert_eq!(ds1.operators[0], DepOperator::GreaterThanOrEq);
        assert_eq!(ds1.versions[0], VersionSpec::new("0.2"));

    }
    #[test]
    fn test_dep_spec_c() {
        let input = "package==0.2<=";
        let ds1 = DepSpec::new(input).unwrap();
        assert_eq!(ds1.name, "package");
    }
    #[test]
    fn test_dep_spec_d() {
        let input = "==0.2<=";
        assert!(DepSpec::new(input).is_err());
    }
    #[test]
    fn test_dep_spec_validate_version_a() {
        let input = "package>0.2<2.0";
        let ds1 = DepSpec::new(input).unwrap();
        assert_eq!(ds1.name, "package");
        assert_eq!(ds1.validate_version(&VersionSpec::new("0.3")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("0.2")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("0.2.1")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_b() {
        let input = "package>0.2,<2.0";
        let ds1 = DepSpec::new(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.0.1")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.0.0")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.9.99.99999")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_c() {
        let input = "package>=2.0,<=3.0";
        let ds1 = DepSpec::new(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.0")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.9.99.99999")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("3.0")), true);

    }
    #[test]
    fn test_dep_spec_validate_version_d() {
        let input = "package==2.*";
        let ds1 = DepSpec::new(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.4")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.9.99.99999")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("3.0")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.3")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_e() {
        let input = "requests [security,tests] >= 2.8.1, == 2.8.* ; python_version < '2.7'*";
        let ds1 = DepSpec::new(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.8.1")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.2.1")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.8.0")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("2.8.99")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_f() {
        let input = "name>=3,<2";
        let ds1 = DepSpec::new(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("2")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("3")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("4")), false);

    }
    #[test]
    fn test_dep_spec_validate_version_g() {
        let input = "name==1.1.post1";
        let ds1 = DepSpec::new(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.post1")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.*")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_h() {
        let input = "name==1.1a1";
        let ds1 = DepSpec::new(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1a1")), true);
        // this is supposed to match...
        // assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.*")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_i() {
        let input = "name==1.1";
        let ds1 = DepSpec::new(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.0")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.0.0")), true);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.dev1")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1a1")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.post1")), false);
        assert_eq!(ds1.validate_version(&VersionSpec::new("1.1.*")), true);
    }
    #[test]
    fn test_dep_spec_validate_version_j() {
        let input = "name===foo++";
        let ds1 = DepSpec::new(input).unwrap();
        assert_eq!(ds1.validate_version(&VersionSpec::new("foo++")), true);
    }
    #[test]
    fn test_dep_spec_to_string_a() {
        let ds1 =DepSpec::new("package  >=0.2,  <0.3   ").unwrap();
        assert_eq!(ds1.to_string(), "");

}
