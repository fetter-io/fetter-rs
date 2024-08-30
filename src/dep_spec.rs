use pest::Parser;
use pest_derive::Parser;


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

#[derive(Debug)]
struct DepSpec {
    name: String,
    operators: Vec<VersionOperator>,
    versions: Vec<String>,
}

impl DepSpec {
    pub fn new(input: &str) -> Result<Self, String> {
        let parse_result = DepSpecParser::parse(Rule::name_req, input)
            .map_err(|e| format!("Parsing error: {}", e))?;

        let pairs = parse_result.into_iter().next().unwrap();
        let mut package_name = String::new();
        let mut versions = Vec::new();
        let mut operators = Vec::new();

        for pair in pairs.into_inner() {
            match pair.as_rule() {
                Rule::identifier => {
                    package_name = pair.as_str().to_string();
                }
                Rule::version_many => {
                    for version_pair in pair.into_inner() {
                        if version_pair.as_rule() == Rule::version_one {
                            let mut op = None;
                            let mut version = String::new();
                            for v_pair in version_pair.into_inner() {
                                match v_pair.as_rule() {
                                    Rule::version_cmp => {
                                        op = Some(match v_pair.as_str() {
                                            "<" => VersionOperator::LessThan,
                                            "<=" => VersionOperator::LessThanOrEqual,
                                            "==" => VersionOperator::Equal,
                                            "!=" => VersionOperator::NotEqual,
                                            ">" => VersionOperator::GreaterThan,
                                            ">=" => VersionOperator::GreaterThanOrEqual,
                                            "~=" => VersionOperator::Compatible,
                                            "===" => VersionOperator::ExactMatch,
                                            _ => return Err("Unknown operator".to_string()),
                                        });
                                    }
                                    Rule::version => version = v_pair.as_str().to_string(),
                                    _ => {}
                                }
                            }

                            if let Some(op) = op {
                                operators.push(op);
                                versions.push(version);
                            }
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



// fn test_grammar() {
//     let input = "package>=0.2,<0.3";
//     // let input = "package_name[extra1,extra2] == 1.0.0";

//     match DepSpecParser::parse(Rule::name_req, input) {
//         Ok(parsed) => {
//             println!("Parsed successfully: {:?}", parsed);
//             // Process the parsed structure as needed
//         },
//         Err(e) => {
//             eprintln!("Parsing error: {}", e);
//         },
//     }
// }


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