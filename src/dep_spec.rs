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
    LessThanOrEqual,
    Equal,
    NotEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Compatible,
    ExactMatch,
}

impl FromStr for DepOperator {
    type Err = Box<dyn Error>;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "<" => Ok(DepOperator::LessThan),
            "<=" => Ok(DepOperator::LessThanOrEqual),
            "==" => Ok(DepOperator::Equal),
            "!=" => Ok(DepOperator::NotEqual),
            ">" => Ok(DepOperator::GreaterThan),
            ">=" => Ok(DepOperator::GreaterThanOrEqual),
            "~=" => Ok(DepOperator::Compatible),
            "===" => Ok(DepOperator::ExactMatch),
            _ => Err(format!("Unknown operator: {}", s).into()),
        }
    }
}

impl fmt::Display for DepOperator {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let op_str = match self {
            DepOperator::LessThan => "<",
            DepOperator::LessThanOrEqual => "<=",
            DepOperator::Equal => "==",
            DepOperator::NotEqual => "!=",
            DepOperator::GreaterThan => ">",
            DepOperator::GreaterThanOrEqual => ">=",
            DepOperator::Compatible => "~=",
            DepOperator::ExactMatch => "===",
        };
        write!(f, "{}", op_str)
    }
}


#[derive(Debug)]
struct DepSpec {
    name: String,
    operators: Vec<DepOperator>,
    versions: Vec<VersionSpec>,
}

impl DepSpec {
    pub fn new(input: &str) -> Result<Self, String> {
        let parse_result = DepSpecParser::parse(Rule::name_req, input)
            .map_err(|e| format!("Parsing error: {}", e))?;

        let results = parse_result.into_iter().next().unwrap();
        let mut package_name = String::new();
        let mut versions = Vec::new();
        let mut operators = Vec::new();

        for pairs in results.into_inner() {
            match pairs.as_rule() {
                Rule::identifier => {
                    package_name = pairs.as_str().to_string();
                }
                Rule::version_many => {
                    for capture in pairs.into_inner() {
                        // print!("{:?}", capture);
                        if capture.as_rule() == Rule::version_one {
                            let mut op = None;
                            let mut version = String::new();

                            for v_pair in capture.into_inner() {
                                match v_pair.as_rule() {
                                    Rule::version_cmp => {
                                        op = Some(v_pair.as_str().parse::<DepOperator>()
                                            .map_err(|e| e.to_string())?);
                                    }
                                    Rule::version => version = v_pair.as_str().to_string(),
                                    _ => {}
                                }
                            }
                            if let Some(op) = op {
                                operators.push(op);
                                versions.push(VersionSpec::new(&version));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if operators.len() != versions.len() {
            return Err("Unmatched versions and operators".to_string());
        }
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
                DepOperator::LessThanOrEqual => version <= spec_version,
                DepOperator::Equal => version == spec_version,
                DepOperator::NotEqual => version != spec_version,
                DepOperator::GreaterThan => version > spec_version,
                DepOperator::GreaterThanOrEqual => version >= spec_version,
                DepOperator::Compatible => version.is_major_compatible(spec_version),
                DepOperator::ExactMatch => version == spec_version,
            };
            if !is_compatible {
                return false;
            }
        }
        true
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
        assert_eq!(ds1.operators[0], DepOperator::GreaterThanOrEqual);
        assert_eq!(ds1.operators[1], DepOperator::LessThan);
    }
    #[test]
    fn test_dep_spec_b() {
        let input = "package[foo]>=0.2; python_version < '2.7'";
        let ds1 = DepSpec::new(input).unwrap();
        // println!("{:?}", ds1);
        assert_eq!(ds1.name, "package");
        assert_eq!(ds1.operators[0], DepOperator::GreaterThanOrEqual);
        assert_eq!(ds1.versions[0], VersionSpec::new("0.2"));

    }
    #[test]
    fn test_dep_spec_c() {
        let input = "package==0.2<=";
        let ds1 = DepSpec::new(input).unwrap();
        assert_eq!(ds1.name, "package");
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
}
