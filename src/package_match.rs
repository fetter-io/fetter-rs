// Simple glob-like matching, supporting * and ? wildcards. Inputs are char iterators.
pub(crate) fn match_str(pattern: &str, input: &str, case_insensitive: bool) -> bool {
    // println!("match_str: pattern = {:?}, input = {:?}", pattern, input);

    let mut p_chars = pattern.chars();
    let mut i_chars = input.chars();

    while let Some(p_char) = p_chars.next() {
        let p_char = if case_insensitive {
            p_char.to_ascii_lowercase()
        } else {
            p_char
        };
        match p_char {
            '*' => {
                // consume all contiguous '*', return if pattern ends with '*'
                loop {
                    match p_chars.clone().peekable().peek() {
                        Some(&p_char_next) => {
                            if p_char_next != '*' {
                                break;
                            }
                            p_chars.next();
                        }
                        None => {
                            return true;
                        }
                    }
                }
                let p_str = p_chars.as_str();
                let i_str = i_chars.as_str();

                // match zero characters or more characters in the i_str
                for i in 0..i_str.len() {
                    if match_str(p_str, &i_str[i..], case_insensitive) {
                        return true;
                    }
                }
                return false;
            }
            '?' => {
                if i_chars.next().is_none() {
                    return false;
                }
            }
            '-' | '_' => match i_chars.next() {
                Some(i_char) if i_char == '-' || i_char == '_' => continue,
                _ => return false,
            },
            _ => {
                let i_char = match i_chars.next() {
                    Some(c) if case_insensitive => c.to_ascii_lowercase(),
                    Some(c) => c,
                    None => return false,
                };
                if p_char != i_char {
                    return false;
                }
            }
        }
    }
    i_chars.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_a() {
        assert!(match_str("he?lo", "hello", false));
        assert!(match_str("*world", "hello world", false));
        assert!(match_str("he*o", "hello", false));
        assert!(match_str("he*o", "heo", false));
        assert!(match_str("he*o", "heasdfasdfero", false));

        assert!(!match_str("h?llo", "hella", false));
        assert!(!match_str("hell*o", "hellaaaaa", false));
    }

    #[test]
    fn test_match_b() {
        assert!(match_str("He?lo", "hello", true));
        assert!(match_str("he*O", "heLLo", true));

        assert!(!match_str("He*O", "hello", false));
    }

    #[test]
    fn test_match_c() {
        assert!(match_str("*", "anything", false));
        assert!(match_str("h*o", "hello", false));
        assert!(match_str("h?llo*", "hello world", false));
        assert!(match_str("he*l?d*", "he_______l?dworlt", false));

        assert!(!match_str("he*l?d", "hello worlt", false));
    }

    #[test]
    fn test_match_d() {
        assert!(match_str("he_lo", "he-lo", false));
        assert!(match_str("he-lo", "he_lo", false));
        assert!(match_str("he*-world", "heLLo_world", false));
        assert!(match_str("he*-wor?d", "heLLo_worxd", false));
        assert!(match_str("he*-wor*", "heLLo_worxd", false));

        assert!(!match_str("he*-wor*q", "heLLo_worxd", false));
    }

    #[test]
    fn test_match_e() {
        assert!(match_str("H*o w*d", "hello world", true));
        assert!(match_str("H*o  w*d", "hello  world", true));

        assert!(!match_str("H*o w*d", "hello  world", true));
    }

    #[test]
    fn test_match_f() {
        assert!(match_str("???", "hld", true));
        assert!(match_str("???*", "hld", true));
        assert!(match_str("???*", "hldfoooo", true));
        assert!(match_str("???*.png", "hldfoooo.png", true));

        assert!(!match_str("???", "ld", true));
        assert!(!match_str("???*.png", "hldfoooo.pn", true));
        assert!(!match_str("???.png", "o.png", true));
    }

    #[test]
    fn test_match_g() {
        assert!(!match_str("???*.png", "x.png", true));
    }

    #[test]
    fn test_match_h() {
        assert!(match_str("-_-_??*.png", "----oo.png", true));
        assert!(match_str("-_-_??*.png", "____oo.png", true));
        assert!(match_str("-_-_??*.png", "____ooXXX.png", true));

        assert!(!match_str("-_-_??*.png", "____o.png", true));
        assert!(!match_str("-_-_??.png", "____ooo.png", true));
    }
}
