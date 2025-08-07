use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::path::PathBuf;
use std::path::MAIN_SEPARATOR;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::util::path_home;

/// As a normal Arc-wrapped PathBuf cannot be a key in a mapping or set, we create this wrapped Arc PathBuf that implements hashability. Cloning this type will increment the reference count.
#[derive(Debug, Clone)]
pub struct PathShared(Arc<PathBuf>);

impl PathShared {
    pub fn from_path_buf(path: PathBuf) -> Self {
        PathShared(Arc::new(path))
    }

    // pub fn from(path: &str) -> Self {
    //     PathShared::from_path_buf(PathBuf::from(path))
    // }

    #[cfg(test)]
    pub(crate) fn strong_count(&self) -> usize {
        Arc::strong_count(&self.0)
    }

    pub(crate) fn as_path(&self) -> &Path {
        self.0.as_path()
    }

    pub(crate) fn join(&self, part: &str) -> PathBuf {
        self.0.join(part)
    }
}

impl From<&str> for PathShared {
    fn from(s: &str) -> Self {
        PathShared::from_path_buf(PathBuf::from(s))
    }
}

impl From<String> for PathShared {
    fn from(s: String) -> Self {
        PathShared::from_path_buf(PathBuf::from(s))
    }
}

impl From<PathBuf> for PathShared {
    fn from(p: PathBuf) -> Self {
        PathShared::from_path_buf(p)
    }
}

impl Serialize for PathShared {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0
            .to_str()
            .ok_or_else(|| serde::ser::Error::custom("Invalid UTF-8 in path"))?
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PathShared {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let path_str = String::deserialize(deserializer)?;
        Ok(PathShared(Arc::new(PathBuf::from(path_str))))
    }
}

impl PartialEq for PathShared {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_path() == other.0.as_path()
    }
}

/// Specialized Path display that replaces home directories with `~`
impl fmt::Display for PathShared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(home) = path_home() {
            let pre = Path::new(&home);
            if let Ok(post) = self.0.strip_prefix(pre) {
                return write!(f, "~{}{}", MAIN_SEPARATOR, post.display());
            }
        }
        write!(f, "{}", self.0.display())
    }
}

impl Eq for PathShared {}

impl Hash for PathShared {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.as_path().hash(state);
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_a() {
        let path1 = PathShared(Arc::new(PathBuf::from("/home/user1")));
        let path2 = PathShared(Arc::new(PathBuf::from("/home/user2")));

        let mut map = HashMap::new();
        map.insert(path1.clone(), "a");
        map.insert(path2.clone(), "b");
        assert_eq!(path1.strong_count(), 2);
        assert_eq!(path2.strong_count(), 2);

        let v = [path1.clone(), path1.clone(), path1.clone(), path2.clone()];

        assert_eq!(map.len(), 2);
        assert_eq!(v.len(), 4);
        assert_eq!(path1.strong_count(), 5);
        assert_eq!(path2.strong_count(), 3);
    }

    #[test]
    fn test_b() {
        let path1 = PathShared::from("/home/user1");
        assert_eq!(format!("{}", path1), "/home/user1");
    }

    #[test]
    fn test_c() {
        let path1 = PathShared::from("/home/user1");
        assert_eq!(path1.as_path(), Path::new("/home/user1"));
    }

    #[test]
    fn test_serialization_a() {
        let path = PathBuf::from("/some/example/path");
        let path_shared = PathShared(Arc::new(path.clone()));

        // Serialize PathShared to a JSON string
        let json = serde_json::to_string(&path_shared).unwrap();
        assert_eq!(json, "\"/some/example/path\"");

        // Deserialize the JSON string back to PathShared
        let deserialized: PathShared = serde_json::from_str(&json).unwrap();
        assert_eq!(*deserialized.0, path);
    }

    #[test]
    fn test_serialization_b() {
        let v: Vec<PathShared> = vec![
            PathShared(Arc::new(PathBuf::from("/some/ex/a"))),
            PathShared(Arc::new(PathBuf::from("/some/ex/b"))),
        ];
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, "[\"/some/ex/a\",\"/some/ex/b\"]");
    }

    #[test]
    fn test_serialization_c() {
        let v: Vec<PathShared> = vec![
            PathShared::from("/some/ex/a"),
            PathShared::from("/some/ex/b"),
        ];
        let mut hm: HashMap<PathBuf, Vec<PathShared>> = HashMap::new();
        hm.insert(PathBuf::from("/usr/bin/py"), v.clone());

        let json = serde_json::to_string(&hm).unwrap();
        assert_eq!(json, "{\"/usr/bin/py\":[\"/some/ex/a\",\"/some/ex/b\"]}");
    }
}
