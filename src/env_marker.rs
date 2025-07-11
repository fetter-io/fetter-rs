use crate::util::ResultDynError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use crate::version_spec::VersionSpec;

//------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub(crate) struct EnvMarkerExpr {
    pub(crate) left: String,
    pub(crate) operator: String,
    pub(crate) right: String,
}

impl EnvMarkerExpr {
    /// Used for testing.
    #[cfg(test)]
    pub fn new(left: &str, operator: &str, right: &str) -> Self {
        Self {
            left: left.to_string(),
            operator: operator.to_string(),
            right: right.to_string(),
        }
    }
}

//------------------------------------------------------------------------------

//1 os_name 	                    os.name
//2 sys_platform 	                sys.platform
//3 platform_machine 	            platform.machine()
//4 platform_python_implementation 	platform.python_implementation()
//5 platform_release 	            platform.release()
//6 platform_system 	            platform.system()
//7 python_version 	                '.'.join(platform.python_version_tuple()[:2])
//8 python_full_version 	        platform.python_version()
//9 implementation_name 	        sys.implementation.name

const PY_ENV_MARKERS: &str = "import os;import sys;import platform;print(os.name);print(sys.platform);print(platform.machine());print(platform.python_implementation());print(platform.release());print(platform.system());print('.'.join(platform.python_version_tuple()[:2]));print(platform.python_version());print(sys.implementation.name)";

// NOTE: not implementing "implementation_version", "platform.version", or "extra"
#[derive(Clone, Debug, PartialEq)]
pub struct EnvMarkerState {
    pub os_name: String,
    pub sys_platform: String,
    pub platform_machine: String,
    pub platform_python_implementation: String,
    pub platform_release: String,
    pub platform_system: String,
    pub python_version: String,
    pub python_full_version: String,
    pub implementation_name: String,
}

enum EvalType {
    StringEval,
    VersionEval,
}

impl EnvMarkerState {
    pub(crate) fn from_exe(executable: &Path) -> ResultDynError<Self> {
        match Command::new(executable)
            .arg("-S") // disable site on startup
            .arg("-c")
            .arg(PY_ENV_MARKERS)
            .output()
        {
            Ok(output) => {
                let mut lines = std::str::from_utf8(&output.stdout)
                    .expect("Failed to convert to UTF-8")
                    .trim()
                    .lines()
                    .map(String::from);
                Ok(EnvMarkerState {
                    os_name: lines.next().ok_or("Missing os_name")?,
                    sys_platform: lines.next().ok_or("Missing sys_platform")?,
                    platform_machine: lines.next().ok_or("Missing platform_machine")?,
                    platform_python_implementation: lines
                        .next()
                        .ok_or("Missing platform_python_implementation")?,
                    platform_release: lines.next().ok_or("Missing platform_release")?,
                    platform_system: lines.next().ok_or("Missing platform_system")?,
                    python_version: lines.next().ok_or("Missing python_version")?,
                    python_full_version: lines
                        .next()
                        .ok_or("Missing python_full_version")?,
                    implementation_name: lines
                        .next()
                        .ok_or("Missing implementation_name")?,
                })
            }
            Err(_) => Ok(EnvMarkerState {
                os_name: "Missing os_name".to_string(),
                sys_platform: "Missing sys_platform".to_string(),
                platform_machine: "Missing platform_machine".to_string(),
                platform_python_implementation: "Missing platform_python_implementation"
                    .to_string(),
                platform_release: "Missing platform_release".to_string(),
                platform_system: "Missing platform_system".to_string(),
                python_version: "Missing python_version".to_string(),
                python_full_version: "Missing python_full_version".to_string(),
                implementation_name: "Missing implementation_name".to_string(),
            }),
        }
    }

    // Constructor for testing.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_str(
        os_name: &str,
        sys_platform: &str,
        platform_machine: &str,
        platform_python_implementation: &str,
        platform_release: &str,
        platform_system: &str,
        python_version: &str,
        python_full_version: &str,
        implementation_name: &str,
    ) -> Self {
        Self {
            os_name: os_name.to_string(),
            sys_platform: sys_platform.to_string(),
            platform_machine: platform_machine.to_string(),
            platform_python_implementation: platform_python_implementation.to_string(),
            platform_release: platform_release.to_string(),
            platform_system: platform_system.to_string(),
            python_version: python_version.to_string(),
            python_full_version: python_full_version.to_string(),
            implementation_name: implementation_name.to_string(),
        }
    }

    //--------------------------------------------------------------------------

    fn eval_version(
        &self,
        left_value: &str,
        operator: &str,
        right_value: &str,
    ) -> ResultDynError<bool> {
        let lv = VersionSpec::new(left_value);
        let rv = VersionSpec::new(right_value);
        let result = match operator {
            "<" => lv < rv,
            "<=" => lv <= rv,
            "==" => lv == rv,
            "!=" => lv != rv,
            ">" => lv > rv,
            ">=" => lv >= rv,
            "~=" => lv.is_compatible(&rv),
            "===" => lv.is_arbitrary_equal(&rv),
            "^" => lv.is_caret(&rv),
            "~" => lv.is_tilde(&rv),
            "in" => left_value.contains(right_value),
            "not in" => !left_value.contains(right_value),
            _ => return Err(format!("Unsupported operator: {operator}").into()),
        };
        Ok(result)
    }

    fn eval_string(
        &self,
        left_value: &str,
        operator: &str,
        right_value: &str,
    ) -> ResultDynError<bool> {
        let result = match operator {
            "<" => left_value < right_value,
            "<=" => left_value <= right_value,
            "==" => left_value == right_value,
            "!=" => left_value != right_value,
            ">" => left_value > right_value,
            ">=" => left_value >= right_value,
            "in" => right_value.contains(left_value),
            "not in" => !right_value.contains(left_value),
            _ => return Err(format!("Unsupported operator: {operator}").into()),
        };
        Ok(result)
    }

    pub(crate) fn eval(&self, eme: &EnvMarkerExpr) -> ResultDynError<bool> {
        use EvalType::*;

        let (left_value, eval_type) = match eme.left.as_ref() {
            "os_name" => (&self.os_name, StringEval),
            "sys_platform" => (&self.sys_platform, StringEval),
            "platform_machine" => (&self.platform_machine, StringEval),
            "platform_python_implementation" => {
                (&self.platform_python_implementation, StringEval)
            }
            "platform_system" => (&self.platform_system, StringEval),
            "implementation_name" => (&self.implementation_name, StringEval),
            "platform_release" => (&self.platform_release, VersionEval),
            "python_version" => (&self.python_version, VersionEval),
            "python_full_version" => (&self.python_full_version, VersionEval),
            _ => return Err("invalid key".into()),
        };

        match eval_type {
            VersionEval => self.eval_version(left_value, &eme.operator, &eme.right),
            StringEval => self.eval_string(left_value, &eme.operator, &eme.right),
        }
    }
}

//------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum BExpToken {
    And,
    Or,
    ParenOpen,
    ParenClose,
    Phrase(String), // Arbitrary strings
}

fn bexp_tokenize(expr: &str) -> Vec<BExpToken> {
    let mut tokens = Vec::new();
    let mut chars = expr.chars().peekable();
    let mut phrase = String::new();

    while let Some(&ch) = chars.peek() {
        match ch {
            '(' => {
                if !phrase.is_empty() {
                    tokens.push(BExpToken::Phrase(phrase.clone()));
                    phrase.clear();
                }
                tokens.push(BExpToken::ParenOpen);
                chars.next();
            }
            ')' => {
                if !phrase.is_empty() {
                    tokens.push(BExpToken::Phrase(phrase.clone()));
                    phrase.clear();
                }
                tokens.push(BExpToken::ParenClose);
                chars.next();
            }
            _ => {
                while let Some(&c) = chars.peek() {
                    if c == ' ' {
                        // when adding a space, can check if we have a leading or / and
                        if !phrase.is_empty() {
                            if phrase.eq("or") {
                                tokens.push(BExpToken::Or);
                                phrase.clear();
                            } else if phrase.eq("and") {
                                tokens.push(BExpToken::And);
                                phrase.clear();
                            } else {
                                // only accumulate if not leading
                                phrase.push(c);
                            }
                        }
                        chars.next();
                    } else if c != '(' && c != ')' {
                        phrase.push(c);
                        chars.next();

                        if c == 'r' && phrase.ends_with(" or") {
                            let pre_op = phrase[..phrase.len() - 3].trim();
                            if !pre_op.is_empty() {
                                tokens.push(BExpToken::Phrase(pre_op.to_string()));
                            }
                            tokens.push(BExpToken::Or);
                            phrase.clear();
                        } else if c == 'd' && phrase.ends_with(" and") {
                            let pre_op = phrase[..phrase.len() - 4].trim();
                            if !pre_op.is_empty() {
                                tokens.push(BExpToken::Phrase(pre_op.to_string()));
                            }
                            tokens.push(BExpToken::And);
                            phrase.clear();
                        }
                    } else {
                        break; // c is ( or )
                    }
                }
            }
        }
    }
    if !phrase.is_empty() {
        tokens.push(BExpToken::Phrase(phrase.clone()));
    }
    tokens
}

fn bexp_eval(tokens: &[BExpToken], lookup: &HashMap<String, bool>) -> bool {
    let mut index = 0;

    fn eval(
        tokens: &[BExpToken],
        index: &mut usize,
        lookup: &HashMap<String, bool>,
    ) -> bool {
        let mut result = false;
        let mut op = None;

        while *index < tokens.len() {
            match &tokens[*index] {
                BExpToken::Phrase(phrase) => {
                    // println!(
                    //     "lookup phrase: {:?} lookup keys: {:?}",
                    //     phrase,
                    //     lookup.keys()
                    // );
                    result = *lookup.get(phrase).unwrap(); // should never happen
                    *index += 1;
                }
                BExpToken::And => {
                    op = Some(BExpToken::And);
                    *index += 1;
                }
                BExpToken::Or => {
                    op = Some(BExpToken::Or);
                    *index += 1;
                }
                BExpToken::ParenOpen => {
                    *index += 1;
                    let sub_result = eval(tokens, index, lookup);
                    if let Some(BExpToken::ParenClose) = tokens.get(*index) {
                        *index += 1;
                    }
                    result = sub_result;
                }
                _ => break,
            }

            if let Some(BExpToken::And) = op {
                result = result && eval(tokens, index, lookup);
            } else if let Some(BExpToken::Or) = op {
                result = result || eval(tokens, index, lookup);
            }
        }
        result
    }
    eval(tokens, &mut index, lookup)
}

// Given an EMS (which will need to be stored in HashMap<ExePath, EnvMarkerState>), validate the marker string.
pub(crate) fn marker_eval(
    marker: &str,
    marker_expr: &HashMap<String, EnvMarkerExpr>,
    ems: &EnvMarkerState,
) -> ResultDynError<bool> {
    // replace marker_expr with evaluated bools
    let mut marker_values: HashMap<String, bool> = HashMap::new();
    for (exp, eme) in marker_expr {
        marker_values.insert(exp.clone(), ems.eval(eme)?);
    }
    let tokens = bexp_tokenize(marker);
    Ok(bexp_eval(&tokens, &marker_values))
}

//------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dep_spec::DepSpec;
    use std::path::PathBuf;

    #[test]
    fn test_bexp_a() {
        let expression = "foo bar or (baz qux and quux corge)";

        let lookup: HashMap<String, bool> = vec![
            ("foo bar".to_string(), true),
            ("baz qux".to_string(), false),
            ("quux corge".to_string(), true),
        ]
        .into_iter()
        .collect();

        let tokens = bexp_tokenize(expression);
        let result = bexp_eval(&tokens, &lookup);
        assert_eq!(result, false);
    }

    #[test]
    fn test_bexp_b() {
        let expression = "a or b or c";

        let lookup: HashMap<String, bool> = vec![
            ("a".to_string(), false),
            ("b".to_string(), false),
            ("c".to_string(), true),
        ]
        .into_iter()
        .collect();

        let tokens = bexp_tokenize(expression);
        let result = bexp_eval(&tokens, &lookup);
        assert_eq!(result, true);
    }

    #[test]
    fn test_bexp_c() {
        let expression = "a a or b b b b or c c c";

        let lookup: HashMap<String, bool> = vec![
            ("a a".to_string(), false),
            ("b b b b".to_string(), false),
            ("c c c".to_string(), false),
        ]
        .into_iter()
        .collect();

        let tokens = bexp_tokenize(expression);
        let result = bexp_eval(&tokens, &lookup);
        assert_eq!(result, false);
    }

    #[test]
    fn test_bexp_d() {
        let expression = "'a a' or ('b b b b' and 'c c c')";

        let lookup: HashMap<String, bool> = vec![
            ("'a a'".to_string(), false),
            ("'b b b b'".to_string(), true),
            ("'c c c'".to_string(), true),
        ]
        .into_iter()
        .collect();

        let tokens = bexp_tokenize(expression);
        let result = bexp_eval(&tokens, &lookup);
        assert_eq!(result, true);
    }

    #[test]
    fn test_bexp_e1() {
        let expression = "foo and bar";

        let lookup: HashMap<String, bool> =
            vec![("foo".to_string(), true), ("bar".to_string(), true)]
                .into_iter()
                .collect();

        let tokens = bexp_tokenize(expression);
        let result = bexp_eval(&tokens, &lookup);
        assert_eq!(result, true);
    }

    #[test]
    fn test_bexp_e2() {
        let expression = "foo and bar";

        let lookup: HashMap<String, bool> =
            vec![("foo".to_string(), true), ("bar".to_string(), false)]
                .into_iter()
                .collect();

        let tokens = bexp_tokenize(expression);
        let result = bexp_eval(&tokens, &lookup);
        assert_eq!(result, false);
    }

    #[test]
    fn test_bexp_f1() {
        let expression = "foo and (bar or (baz or (zab or pax)))";

        let lookup: HashMap<String, bool> = vec![
            ("foo".to_string(), true),
            ("bar".to_string(), false),
            ("baz".to_string(), false),
            ("zab".to_string(), false),
            ("pax".to_string(), true),
        ]
        .into_iter()
        .collect();

        let tokens = bexp_tokenize(expression);
        let result = bexp_eval(&tokens, &lookup);
        assert_eq!(result, true);
    }

    #[test]
    fn test_bexp_f2() {
        let expression = "foo and (bar or (baz or (zab or pax)))";

        let lookup: HashMap<String, bool> = vec![
            ("foo".to_string(), true),
            ("bar".to_string(), false),
            ("baz".to_string(), false),
            ("zab".to_string(), false),
            ("pax".to_string(), false),
        ]
        .into_iter()
        .collect();

        let tokens = bexp_tokenize(expression);
        let result = bexp_eval(&tokens, &lookup);
        assert_eq!(result, false);
    }

    #[test]
    fn test_bexp_g1() {
        let expression = "(python_version > '2.0' and python_version < '2.7.9') or python_version >= '3.0'";

        let lookup: HashMap<String, bool> = vec![
            ("python_version > '2.0'".to_string(), true),
            ("python_version < '2.7.9'".to_string(), false),
            ("python_version >= '3.0'".to_string(), false),
        ]
        .into_iter()
        .collect();

        let tokens = bexp_tokenize(expression);
        let result = bexp_eval(&tokens, &lookup);
        assert_eq!(result, false);
    }

    //--------------------------------------------------------------------------

    #[test]
    fn test_emv_a() {
        let emv = EnvMarkerState::from_exe(&PathBuf::from("python3"));
        assert_eq!(emv.is_ok(), true);
    }

    fn get_ems_darwin() -> EnvMarkerState {
        EnvMarkerState::from_str(
            "posix", "darwin", "arm64", "CPython", "23.1.0", "Darwin", "3.13", "3.13.1",
            "cpython",
        )
    }

    #[test]
    fn test_emv_eval_a1() {
        let emv = get_ems_darwin();
        let eme1 = EnvMarkerExpr::new("python_version", "<", "3.9");
        assert_eq!(emv.eval(&eme1).unwrap(), false);
        let eme2 = EnvMarkerExpr::new("python_version", ">=", "3.13");
        assert_eq!(emv.eval(&eme2).unwrap(), true);
        let eme3 = EnvMarkerExpr::new("python_version", ">", "3.12");
        assert_eq!(emv.eval(&eme3).unwrap(), true);
    }

    #[test]
    fn test_emv_eval_a2() {
        let emv = get_ems_darwin();
        let eme1 = EnvMarkerExpr::new("python_full_version", ">", "3.13.0");
        assert_eq!(emv.eval(&eme1).unwrap(), true);
        let eme2 = EnvMarkerExpr::new("python_full_version", ">=", "3.13.3");
        assert_eq!(emv.eval(&eme2).unwrap(), false);
        let eme3 = EnvMarkerExpr::new("python_full_version", "==", "3.13.*");
        assert_eq!(emv.eval(&eme3).unwrap(), true);
    }

    #[test]
    fn test_emv_eval_b() {
        let emv = get_ems_darwin();
        let eme1 = EnvMarkerExpr::new("platform_machine", "in", "arm64");
        assert_eq!(emv.eval(&eme1).unwrap(), true);
        let eme2 = EnvMarkerExpr::new("platform_machine", "==", "arm64");
        assert_eq!(emv.eval(&eme2).unwrap(), true);
        let eme3 = EnvMarkerExpr::new("platform_machine", "not in", "unarm64");
        assert_eq!(emv.eval(&eme3).unwrap(), false);
    }

    #[test]
    fn test_emv_eval_c() {
        let emv = get_ems_darwin();
        let eme1 = EnvMarkerExpr::new("os_name", "in", "posix");
        assert_eq!(emv.eval(&eme1).unwrap(), true);
        let eme2 = EnvMarkerExpr::new("os_name", "==", "posix");
        assert_eq!(emv.eval(&eme2).unwrap(), true);
        let eme3 = EnvMarkerExpr::new("os_name", "!=", "nt");
        assert_eq!(emv.eval(&eme3).unwrap(), true);
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_marker_eval_a1() {
        let ds = DepSpec::from_string("foo >= 3.4 ;(python_version > '2.0' and python_version < '2.7.9') or python_version >= '3.0'").unwrap();
        let ems = get_ems_darwin();
        assert_eq!(
            marker_eval(&ds.env_marker, &ds.env_marker_expr.unwrap(), &ems).unwrap(),
            true
        )
    }

    #[test]
    fn test_marker_eval_a2() {
        let ds = DepSpec::from_string("foo >= 3.4 ;(python_version > '2.0' and python_version < '2.7.9') or python_version >= '3.15'").unwrap();
        let ems = get_ems_darwin();
        assert_eq!(
            marker_eval(&ds.env_marker, &ds.env_marker_expr.unwrap(), &ems).unwrap(),
            false
        )
    }

    #[test]
    fn test_marker_eval_a3() {
        let ds = DepSpec::from_string("foo >= 3.4 ;(python_version > '2.0' and python_version < '2.7.9') or python_version < '3.5' or python_version >= '3.13'").unwrap();
        let ems = get_ems_darwin();
        assert_eq!(
            marker_eval(&ds.env_marker, &ds.env_marker_expr.unwrap(), &ems).unwrap(),
            true
        )
    }

    #[test]
    fn test_marker_eval_b1() {
        let ds = DepSpec::from_string(
            "foo >= 3.4 ;sys_platform == 'darwin' and platform_machine == 'arm64'",
        )
        .unwrap();
        let ems = get_ems_darwin();
        assert_eq!(
            marker_eval(&ds.env_marker, &ds.env_marker_expr.unwrap(), &ems).unwrap(),
            true
        )
    }

    #[test]
    fn test_marker_eval_b2() {
        let ds = DepSpec::from_string("foo >= 3.4;   sys_platform == 'darwin' and platform_machine == 'arm64' and   platform_system   == 'foo' ").unwrap();
        let ems = get_ems_darwin();
        assert_eq!(
            marker_eval(&ds.env_marker, &ds.env_marker_expr.unwrap(), &ems).unwrap(),
            false
        )
    }

    #[test]
    fn test_marker_eval_b3() {
        let ds = DepSpec::from_string("foo >= 3.4;   sys_platform == 'darwin' and platform_machine == 'arm64' and   platform_system   == 'Darwin' ").unwrap();
        let ems = get_ems_darwin();
        assert_eq!(
            marker_eval(&ds.env_marker, &ds.env_marker_expr.unwrap(), &ems).unwrap(),
            true
        )
    }

    #[test]
    fn test_marker_eval_c1() {
        let ds = DepSpec::from_string("foo >= 3.4;   os_name == 'posix' and platform_python_implementation == 'CPython' and   platform_release  == '23.*' ").unwrap();
        let ems = get_ems_darwin();
        assert_eq!(
            marker_eval(&ds.env_marker, &ds.env_marker_expr.unwrap(), &ems).unwrap(),
            true
        )
    }

    #[test]
    fn test_marker_eval_c2() {
        let ds = DepSpec::from_string("foo >= 3.4;   os_name == 'posix' and platform_python_implementation == 'foo' and   platform_release  == '23.*' ").unwrap();
        let ems = get_ems_darwin();
        assert_eq!(
            marker_eval(&ds.env_marker, &ds.env_marker_expr.unwrap(), &ems).unwrap(),
            false
        )
    }

    #[test]
    fn test_marker_eval_c3() {
        let ds = DepSpec::from_string("foo >= 3.4;   os_name == 'posix' and platform_python_implementation == 'CPython' and  implementation_name  == 'cpython' ").unwrap();
        let ems = get_ems_darwin();
        assert_eq!(
            marker_eval(&ds.env_marker, &ds.env_marker_expr.unwrap(), &ems).unwrap(),
            true
        )
    }
}
