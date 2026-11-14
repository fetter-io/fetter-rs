use std::cmp::Ordering;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ecosystem::Ecosystem;
use crate::path_shared::PathShared;

//------------------------------------------------------------------------------
/// An Origin is the thing that consumes and resolves a set of dependencies: for Python, an interpreter; for npm, a project directory; for Cargo, a workspace root; etc.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    pub ecosystem: Ecosystem,
    pub path: PathShared,
}

impl Origin {
    pub fn new(ecosystem: Ecosystem, path: PathShared) -> Self {
        Origin { ecosystem, path }
    }

    pub(crate) fn as_path(&self) -> &Path {
        self.path.as_path()
    }
}

/// Order by path first, so that output sorted by Origin reads as sorted by path.
impl Ord for Origin {
    fn cmp(&self, other: &Self) -> Ordering {
        self.path
            .cmp(&other.path)
            .then_with(|| self.ecosystem.cmp(&other.ecosystem))
    }
}

impl PartialOrd for Origin {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Display as the path alone.
impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.path.fmt(f)
    }
}

//------------------------------------------------------------------------------
/// The nature of the source from which a Site's packages are read.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum SiteKind {
    /// Artifacts installed on disk (site-packages, node_modules, an extensions dir).
    Installed,
    /// A resolved lockfile (Cargo.lock, package-lock.json).
    Lockfile,
    /// A declaration file that is itself the resolution (a GitHub Actions workflow).
    Manifest,
}

//------------------------------------------------------------------------------
/// A Site is a concrete location holding resolved packages for one or more Origins.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Site {
    pub ecosystem: Ecosystem,
    pub kind: SiteKind,
    pub path: PathShared,
}

impl Site {
    pub fn new(ecosystem: Ecosystem, kind: SiteKind, path: PathShared) -> Self {
        Site {
            ecosystem,
            kind,
            path,
        }
    }

    /// Convenience constructor for a Python site-packages directory.
    pub(crate) fn python(path: PathShared) -> Self {
        Site::new(Ecosystem::Python, SiteKind::Installed, path)
    }

    pub(crate) fn as_path(&self) -> &Path {
        self.path.as_path()
    }

    pub(crate) fn join(&self, part: &str) -> PathBuf {
        self.path.join(part)
    }
}

/// Order by path first, so that output sorted by Site reads as sorted by path.
impl Ord for Site {
    fn cmp(&self, other: &Self) -> Ordering {
        self.path
            .cmp(&other.path)
            .then_with(|| self.ecosystem.cmp(&other.ecosystem))
            .then_with(|| self.kind.cmp(&other.kind))
    }
}

impl PartialOrd for Site {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Display as the path alone.
impl fmt::Display for Site {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.path.fmt(f)
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_site_ord_a() {
        let s1 = Site::python(PathShared::from("/b"));
        let s2 = Site::python(PathShared::from("/a"));
        let s3 = Site::new(Ecosystem::Npm, SiteKind::Lockfile, PathShared::from("/a"));
        let mut v = vec![s1.clone(), s3.clone(), s2.clone()];
        v.sort();
        assert_eq!(v, vec![s2, s3, s1]);
    }

    #[test]
    fn test_site_display_a() {
        let s = Site::python(PathShared::from("/x/site-packages"));
        assert_eq!(
            s.to_string(),
            PathShared::from("/x/site-packages").to_string()
        );
    }

    #[test]
    fn test_origin_ord_a() {
        let o1 = Origin::new(Ecosystem::Python, PathShared::from("/usr/bin/python3"));
        let o2 = Origin::new(Ecosystem::Cargo, PathShared::from("/home/a/proj"));
        let mut v = vec![o1.clone(), o2.clone()];
        v.sort();
        assert_eq!(v, vec![o2, o1]);
    }
}
