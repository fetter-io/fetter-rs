//! Per-ecosystem scanners. Each ecosystem implements `EcosystemScanner`; the walker (`crate::walk`) dispatches to all enabled scanners during a single filesystem walk.

pub(crate) mod python;

use std::fs::FileType;
use std::path::Path;
use std::path::PathBuf;

use crate::ecosystem::Ecosystem;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::package::Package;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::ScanConfig;

//------------------------------------------------------------------------------
/// One entry of a directory being walked. The `file_type` is from the directory read and does not follow symlinks, so a symlink to a directory has `is_dir() == false`.
pub(crate) struct DirItem {
    pub(crate) path: PathBuf,
    pub(crate) file_type: FileType,
}

impl DirItem {
    pub(crate) fn file_name(&self) -> Option<&str> {
        self.path.file_name().and_then(|n| n.to_str())
    }
}

/// The result of a scanner inspecting one directory.
#[derive(Debug, Default)]
pub(crate) struct Detect {
    /// Origins found in this directory.
    pub(crate) origins: Vec<Origin>,
    /// If true, the walker will not descend into this directory's children, for any scanner.
    pub(crate) prune: bool,
}

//------------------------------------------------------------------------------
/// The interface each ecosystem implements.
pub(crate) trait EcosystemScanner: Sync {
    /// Additional `(directory, recursive)` roots to walk beyond the home directories (e.g. PATH bin dirs for Python).
    fn roots(&self) -> Vec<(PathBuf, bool)> {
        Vec::new()
    }

    /// Origins found without walking (e.g. the default Python on PATH).
    fn default_origins(&self) -> Vec<Origin> {
        Vec::new()
    }

    /// Called once per visited directory with its entries; may claim origins and may prune descent.
    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect;

    /// Convert a user-provided path into an Origin of this ecosystem, validating that it exists.
    fn normalize_origin(&self, path: &Path) -> ResultDynError<Origin>;

    /// Find all Sites for an Origin.
    fn origin_to_sites(
        &self,
        origin: &Origin,
        config: &ScanConfig,
        log: FlagLog,
    ) -> Vec<Site>;

    /// Read all Packages from a Site.
    fn site_to_packages(&self, site: &Site) -> Vec<Package>;
}

//------------------------------------------------------------------------------
static PYTHON: python::PythonScanner = python::PythonScanner;

/// Return the scanner for an ecosystem, if one is implemented.
pub(crate) fn scanner_for(ecosystem: Ecosystem) -> Option<&'static dyn EcosystemScanner> {
    match ecosystem {
        Ecosystem::Python => Some(&PYTHON),
        Ecosystem::Npm | Ecosystem::Cargo | Ecosystem::Gha | Ecosystem::VsCode => None,
    }
}

/// Return all implemented scanners, in canonical ecosystem order.
pub(crate) fn scanners() -> Vec<&'static dyn EcosystemScanner> {
    Ecosystem::ALL
        .iter()
        .filter_map(|e| scanner_for(*e))
        .collect()
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scanner_for_a() {
        assert!(scanner_for(Ecosystem::Python).is_some());
        assert!(scanner_for(Ecosystem::Npm).is_none());
    }

    #[test]
    fn test_scanners_a() {
        assert_eq!(scanners().len(), 1);
    }
}
