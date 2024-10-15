use std::hash::{Hash, Hasher};
use std::path::Display;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

/// As a normal Arc-wrapped PathBuf cannot be a key in a mapping or set, we create this wrapped Arc PathBuf that implements hashability. Cloning this type will increment the reference count.
#[derive(Debug, Clone)]
pub(crate) struct PathShared(Arc<PathBuf>);

impl PathShared {
    pub(crate) fn from_path_buf(path: PathBuf) -> Self {
        PathShared(Arc::new(path))
    }

    pub(crate) fn from_str(path: &str) -> Self {
        PathShared::from_path_buf(PathBuf::from(path))
    }

    #[allow(dead_code)]
    pub(crate) fn strong_count(&self) -> usize {
        Arc::strong_count(&self.0)
    }

    pub(crate) fn as_path(&self) -> &Path {
        self.0.as_path()
    }

    pub(crate) fn join(&self, part: &str) -> PathBuf {
        self.0.join(part)
    }

    pub(crate) fn display(&self) -> Display {
        self.0.display()
    }
}

impl PartialEq for PathShared {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_path() == other.0.as_path()
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

        let v = vec![path1.clone(), path1.clone(), path1.clone(), path2.clone()];

        assert_eq!(map.len(), 2);
        assert_eq!(v.len(), 4);
        assert_eq!(path1.strong_count(), 5);
        assert_eq!(path2.strong_count(), 3);
    }

    #[test]
    fn test_b() {
        let path1 = PathShared::from_str("/home/user1");
        assert_eq!(format!("{}", path1.display()), "/home/user1");
    }

    #[test]
    fn test_c() {
        let path1 = PathShared::from_str("/home/user1");
        assert_eq!(path1.as_path(), Path::new("/home/user1"));
    }
}
