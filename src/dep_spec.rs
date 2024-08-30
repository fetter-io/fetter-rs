use std::str::FromStr;
use std::fmt;
use std::error::Error;

use pest::Parser;
use pest_derive::Parser;

use crate::version_spec::VersionSpec;


#[derive(Parser)]
#[grammar = "dep_spec.pest"]
struct DepSpecParser;


#[derive(Debug)]
enum VersionOperator {
    LessThan,
    LessThanOrEqual,
    Equal,
    NotEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Compatible,
    ExactMatch,
}

impl FromStr for VersionOperator {
    type Err = Box<dyn Error>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "<" => Ok(VersionOperator::LessThan),
            "<=" => Ok(VersionOperator::LessThanOrEqual),
            "==" => Ok(VersionOperator::Equal),
            "!=" => Ok(VersionOperator::NotEqual),
            ">" => Ok(VersionOperator::GreaterThan),
            ">=" => Ok(VersionOperator::GreaterThanOrEqual),
            "~=" => Ok(VersionOperator::Compatible),
            "===" => Ok(VersionOperator::ExactMatch),
            _ => Err(format!("Unknown operator: {}", s).into()),
        }
    }
}

impl fmt::Display for VersionOperator {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let op_str = match self {
            VersionOperator::LessThan => "<",
            VersionOperator::LessThanOrEqual => "<=",
            VersionOperator::Equal => "==",
            VersionOperator::NotEqual => "!=",
            VersionOperator::GreaterThan => ">",
            VersionOperator::GreaterThanOrEqual => ">=",
            VersionOperator::Compatible => "~=",
            VersionOperator::ExactMatch => "===",
        };
        write!(f, "{}", op_str)
    }
}


#[derive(Debug)]
struct DepSpec {
    name: String,
    operators: Vec<VersionOperator>,
    versions: Vec<VersionSpec>,
}

impl DepSpec {
    pub fn new(input: &str) -> Result<Self, String> {
        let parse_result = DepSpecParser::parse(Rule::name_req, input)
            .map_err(|e| format!("Parsing error: {}", e))?;

        let reuslts = parse_result.into_iter().next().unwrap();
        let mut package_name = String::new();
        let mut versions = Vec::new();
        let mut operators = Vec::new();

        for pairs in reuslts.into_inner() {
            match pairs.as_rule() {
                Rule::identifier => {
                    package_name = pairs.as_str().to_string();
                }
                Rule::version_many => {
                    for version_pair in pairs.into_inner() {
                        if version_pair.as_rule() == Rule::version_one {
                            let mut op = None;
                            let mut version = String::new();

                            for v_pair in version_pair.into_inner() {
                                match v_pair.as_rule() {
                                    Rule::version_cmp => {
                                        op = Some(v_pair.as_str().parse::<VersionOperator>()
                                            .map_err(|e| e.to_string())?);
                                    }
                                    Rule::version => version = v_pair.as_str().to_string(),
                                    _ => {}
                                }
                            }

                            if let Some(op) = op {
                                operators.push(op);
                                versions.push(VersionSpec::new(&version));                            }
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(DepSpec {
            name: package_name,
            operators,
            versions,
        })
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
        println!("{:?}", ds1);
    }
}