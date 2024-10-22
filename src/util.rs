use std::env;
use std::path::PathBuf;

// Normalize all names
pub(crate) fn name_to_key(name: &String) -> String {
    name.to_lowercase().replace("-", "_")
}

/// Remove whitespace and a leading "@" if found. Note: this owns the passed String as this is appropriate for the context in which it is used.
pub(crate) fn url_trim(mut input: String) -> String {
    input = input.trim().to_string();
    if input.starts_with('@') {
        input.remove(0);
        input = input.trim().to_string();
    }
    input
}

pub(crate) fn url_strip_user(url: &String) -> String {
    if let Some(pos_protocol) = url.find("://") {
        let pos_start = pos_protocol + 3;
        // get span to first @ if it exists
        if let Some(pos_span) = url[pos_start..].find('@') {
            let pos_end = pos_start + pos_span + 1;
            if url[pos_start..pos_end].find('/').is_none() {
                return format!("{}{}", &url[..pos_start], &url[pos_end..]);
            }
        }
    }
    url.to_string()
}

//------------------------------------------------------------------------------

pub(crate) fn path_home() -> Option<PathBuf> {
    if env::consts::OS == "windows" {
        env::var_os("USERPROFILE").map(PathBuf::from)
    } else {
        env::var_os("HOME").map(PathBuf::from)
    }
}

pub(crate) fn path_normalize(mut path: PathBuf) -> Result<PathBuf, String> {
    if let Some(path_str) = path.to_str() {
        if path_str.starts_with("~") {
            if let Some(home) = path_home() {
                path = home.join(path_str.trim_start_matches("~"));
            } else {
                return Err("Usage of `~` unresolved.".into());
            }
        }
    }
    // only expand relative paths if there is more than one component
    if path.is_relative() && path.components().count() > 1 {
        let cwd = env::current_dir().map_err(|e| e.to_string())?;
        path = cwd.join(path);
    }
    Ok(path)
}
//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_strip_user_a() {
        let s1 = "file:///localbuilds/pip-1.3.1-py33-none-any.whl".to_string();
        let s2 = url_strip_user(&s1);
        assert_eq!(s1, s2)
    }

    #[test]
    fn test_url_strip_user_b() {
        let s1 = "file://foo@/localbuilds/pip-1.3.1-py33-none-any.whl".to_string();
        let s2 = url_strip_user(&s1);
        assert_eq!(s2, "file:///localbuilds/pip-1.3.1-py33-none-any.whl")
    }

    #[test]
    fn test_url_strip_user_c() {
        let s1 = "https://github.com/pypa/pip/archive/1.3.1.zip#sha1=da9234ee9982d4bbb3c72346a6de940a148ea686".to_string();
        let s2 = url_strip_user(&s1);
        assert_eq!(s2, s1)
    }

    #[test]
    fn test_url_strip_user_d() {
        let s1 = "git+https://git.repo/some_pkg.git@1.3.1".to_string();
        let s2 = url_strip_user(&s1);
        assert_eq!(s2, s1)
    }

    #[test]
    fn test_url_strip_user_e() {
        let s1 = "git+ssh://git@github.com/uqfoundation/dill.git@0.3.8".to_string();
        let s2 = url_strip_user(&s1);
        assert_eq!(s2, "git+ssh://github.com/uqfoundation/dill.git@0.3.8")
    }

    #[test]
    fn test_url_strip_user_f() {
        let s1 = "git+https://foo@github.com/pypa/packaging.git@cf2cbe2aec28f87c6228a6fb136c27931c9af407".to_string();
        let s2 = url_strip_user(&s1);
        assert_eq!(s2, "git+https://github.com/pypa/packaging.git@cf2cbe2aec28f87c6228a6fb136c27931c9af407")
    }
}
