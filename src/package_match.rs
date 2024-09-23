// Simple glob-like matching, supporting * and ? wildcards.
fn match_iter<I>(mut pattern_chars: I, mut input_chars: I, case_insensitive: bool) -> bool
where
    I: Iterator<Item = char> + Clone,
{
    while let Some(pat_char) = pattern_chars.next() {
        let pat_char = if case_insensitive {
            pat_char.to_ascii_lowercase()
        } else {
            pat_char
        };
        match pat_char {
            '*' => {
                // advance pattern to last of any consecutive *
                while let Some(&next_pat_char) = pattern_chars.clone().peekable().peek() {
                    if next_pat_char != '*' {
                        break;
                    }
                    pattern_chars.next();
                }
                // handle zero * matches in input
                if match_iter(pattern_chars.clone(), input_chars.clone(), case_insensitive) {
                    return true;
                }
                // drop one input character and compare the rest
                while input_chars.next().is_some() {
                    if match_iter(
                        pattern_chars.clone(),
                        input_chars.clone(),
                        case_insensitive,
                    ) {
                        return true;
                    }
                }
                return false;
            }
            '?' => {
                if input_chars.next().is_none() {
                    return false;
                }
            }
            '-' | '_' => match input_chars.next() {
                Some(in_char) if in_char == '-' || in_char == '_' => continue,
                _ => return false,
            },
            _ => {
                let in_char = match input_chars.next() {
                    Some(c) if case_insensitive => c.to_ascii_lowercase(),
                    Some(c) => c,
                    None => return false,
                };
                if pat_char != in_char {
                    return false;
                }
            }
        }
    }
    input_chars.next().is_none() // input exhausted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_match_a() {
        assert!(match_iter("he?lo".chars(), "hello".chars(), false));
        assert!(match_iter("*world".chars(), "hello world".chars(), false));
        assert!(match_iter("he*o".chars(), "hello".chars(), false));
        assert!(match_iter("he*o".chars(), "heo".chars(), false));
        assert!(match_iter("he*o".chars(), "heasdfasdfero".chars(), false));

        assert!(!match_iter("h?llo".chars(), "hella".chars(), false));
        assert!(!match_iter("hell*o".chars(), "hellaaaaa".chars(), false));
    }

    #[test]
    fn test_match_b() {
        assert!(match_iter("He?lo".chars(), "hello".chars(), true));
        assert!(match_iter("he*O".chars(), "heLLo".chars(), true));

        assert!(!match_iter("He*O".chars(), "hello".chars(), false));
    }

    #[test]
    fn test_match_c() {
        assert!(match_iter("*".chars(), "anything".chars(), false));
        assert!(match_iter("h*o".chars(), "hello".chars(), false));
        assert!(match_iter("h?llo*".chars(), "hello world".chars(), false));
        assert!(match_iter("he*l?d*".chars(), "he_______l?dworlt".chars(), false));

        assert!(!match_iter("he*l?d".chars(), "hello worlt".chars(), false));
    }

    #[test]
    fn test_match_d() {
        assert!(match_iter("he_lo".chars(), "he-lo".chars(), false));
        assert!(match_iter("he-lo".chars(), "he_lo".chars(), false));
        assert!(match_iter("he*-world".chars(), "heLLo_world".chars(), false));
        assert!(match_iter("he*-wor?d".chars(), "heLLo_worxd".chars(), false));
        assert!(match_iter("he*-wor*".chars(), "heLLo_worxd".chars(), false));

        assert!(!match_iter("he*-wor*q".chars(), "heLLo_worxd".chars(), false));
    }
}
