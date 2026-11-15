//! Per-ecosystem scanners. Each ecosystem implements `EcosystemScanner`; the walker (`crate::walk`) dispatches to all enabled scanners during a single filesystem walk.

pub(crate) mod cargo;
pub(crate) mod gha;
pub(crate) mod go;
pub(crate) mod npm;
pub(crate) mod php;
pub(crate) mod precommit;
pub(crate) mod python;
pub(crate) mod ruby;
pub(crate) mod swift;
pub(crate) mod vscode;

use std::collections::HashSet;
use std::fs;
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
/// Editor data directories (VS Code and forks). An editor directory is one of these that contains an `extensions` directory; its extensions belong to the VS Code scanner, and the npm scanner never treats them as projects.
pub(crate) const EDITOR_DIRS: [&str; 8] = [
    ".vscode",
    ".vscode-insiders",
    ".vscode-oss",
    ".vscode-server",
    ".cursor",
    ".cursor-server",
    ".windsurf",
    ".positron",
];

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

/// Code that runs without being explicitly invoked: at interpreter startup, at install time, or at editor startup (reported by `inspect`). Only hooks that are cheap to detect and rare are reported, so that their presence is a signal (see SPEC §4).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Hook {
    /// What has the hook: a file name (Python), or a package and hook kind (npm, VS Code).
    pub(crate) name: String,
    /// The code or setting that runs, e.g. the first lines of a `.pth` file or an install script.
    pub(crate) detail: String,
}

/// The installed files of one package at one site (for `unpack-*` and `purge-*`). `files` are explicit files, each with whether it exists; `dirs` are directories that belong wholly to the package (removed recursively by `purge-*`).
#[derive(Debug, Clone, Default)]
pub(crate) struct Artifacts {
    pub(crate) files: Vec<(PathBuf, bool)>,
    pub(crate) dirs: Vec<PathBuf>,
}

impl Artifacts {
    /// Artifacts of packages that are installed as whole directories (npm packages, VS Code extensions): the directories, and all files below them. Below each directory, child directories named in `exclude` (e.g. `node_modules`, which holds other packages) are not listed. Symlinks are listed as files and not followed.
    pub(crate) fn from_dirs(dirs: Vec<PathBuf>, exclude: &[&str]) -> Self {
        let mut files = Vec::new();
        for dir in &dirs {
            collect_files(dir, exclude, &mut files);
        }
        files.sort();
        Artifacts { files, dirs }
    }
}

/// Recursively collect files below `dir`; child directories of `dir` itself named in `exclude` are skipped.
fn collect_files(dir: &Path, exclude: &[&str], out: &mut Vec<(PathBuf, bool)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if is_dir {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|n| exclude.contains(&n))
            {
                continue;
            }
            collect_files(&path, &[], out);
        } else {
            out.push((path, true));
        }
    }
}

/// Lines of a hook file to show.
pub(crate) const HOOK_HEAD_LINES: usize = 10;

/// The first lines of a file, joined with spaces (for hooks whose code is in a file).
pub(crate) fn head_lines(fp: &Path, n: usize) -> std::io::Result<String> {
    use std::io::BufRead;
    let reader = std::io::BufReader::new(fs::File::open(fp)?);
    Ok(reader
        .lines()
        .take(n)
        .map_while(Result::ok)
        .collect::<Vec<_>>()
        .join(" "))
}

/// The result of a scanner inspecting one directory.
#[derive(Debug, Default)]
pub(crate) struct Detect {
    /// Origins found in this directory.
    pub(crate) origins: Vec<Origin>,
    /// If true, the walker will not descend into this directory's children, for any scanner.
    pub(crate) prune: bool,
    /// Names of child directories the walker will not descend into, for any scanner (e.g. `target` in a Cargo project). Unlike `prune`, other children are still visited.
    pub(crate) skip_children: Vec<&'static str>,
}

//------------------------------------------------------------------------------
/// The interface each ecosystem implements.
pub(crate) trait EcosystemScanner: Sync {
    /// Additional `(directory, recursive)` roots to walk beyond the home directories (e.g. PATH bin dirs for Python).
    fn roots(&self) -> Vec<(PathBuf, bool)> {
        Vec::new()
    }

    /// Directories of downloaded package sources (e.g. Cargo's registry, Go's module cache). They contain lockfiles and manifests of other packages, not projects of any ecosystem, so the walk never enters them, whichever ecosystems are enabled. Default: none.
    fn cache_dirs(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Origins found without walking (e.g. the default Python on PATH).
    fn default_origins(&self) -> Vec<Origin> {
        Vec::new()
    }

    /// Called once per visited directory with its entries; may claim origins and may prune descent.
    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect;

    /// Return true if a user-provided path (e.g. from `-o`) looks like an Origin of this ecosystem. This should be cheap and should not require the path to be valid; `normalize_origin` validates.
    fn accepts(&self, path: &Path) -> bool;

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

    /// Read all Hooks from a Site (for `inspect`). Default: none.
    fn site_to_hooks(&self, _site: &Site) -> Vec<Hook> {
        Vec::new()
    }

    /// Installed artifacts of the `wanted` packages at a Site (for `unpack-*` and `purge-*`), one entry per package; a package installed several times within the site has its artifacts merged. Wanted packages without artifacts at this site are omitted. Default: none, as lockfile and manifest sites have no installed files.
    fn site_to_artifacts(
        &self,
        _site: &Site,
        _wanted: &HashSet<&Package>,
    ) -> Vec<(Package, Artifacts)> {
        Vec::new()
    }
}

//------------------------------------------------------------------------------
static PYTHON: python::PythonScanner = python::PythonScanner;
static CARGO: cargo::CargoScanner = cargo::CargoScanner;
static NPM: npm::NpmScanner = npm::NpmScanner;
static GHA: gha::GhaScanner = gha::GhaScanner;
static GO: go::GoScanner = go::GoScanner;
static RUBY: ruby::RubyScanner = ruby::RubyScanner;
static PHP: php::PhpScanner = php::PhpScanner;
static PRECOMMIT: precommit::PreCommitScanner = precommit::PreCommitScanner;
static SWIFT: swift::SwiftScanner = swift::SwiftScanner;
static VSCODE: vscode::VsCodeScanner = vscode::VsCodeScanner;

/// Return the scanner for an ecosystem, if one is implemented.
pub(crate) fn scanner_for(ecosystem: Ecosystem) -> Option<&'static dyn EcosystemScanner> {
    match ecosystem {
        Ecosystem::Python => Some(&PYTHON),
        Ecosystem::Cargo => Some(&CARGO),
        Ecosystem::Npm => Some(&NPM),
        Ecosystem::Gha => Some(&GHA),
        Ecosystem::VsCode => Some(&VSCODE),
        Ecosystem::Go => Some(&GO),
        Ecosystem::Ruby => Some(&RUBY),
        Ecosystem::Php => Some(&PHP),
        Ecosystem::PreCommit => Some(&PRECOMMIT),
        Ecosystem::Swift => Some(&SWIFT),
    }
}

/// Return the implemented scanners for the provided ecosystems, in the provided order.
pub(crate) fn scanners(ecosystems: &[Ecosystem]) -> Vec<&'static dyn EcosystemScanner> {
    ecosystems.iter().filter_map(|e| scanner_for(*e)).collect()
}

/// Determine which of the provided ecosystems a user-provided path belongs to, and normalize it to Origins. A path can be an Origin for more than one ecosystem (e.g. a directory with both `package.json` and `Cargo.lock`); all accepting scanners contribute. It is an error if no scanner accepts the path, or if an accepting scanner cannot normalize it.
pub(crate) fn infer_origins(
    path: &Path,
    ecosystems: &[Ecosystem],
) -> ResultDynError<Vec<Origin>> {
    let origins = scanners(ecosystems)
        .into_iter()
        .filter(|s| s.accepts(path))
        .map(|s| s.normalize_origin(path))
        .collect::<ResultDynError<Vec<Origin>>>()?;
    if !origins.is_empty() {
        return Ok(origins);
    }
    let names: Vec<&str> = ecosystems
        .iter()
        .filter(|e| scanner_for(**e).is_some())
        .map(|e| e.cli_name())
        .collect();
    let msg = if names.is_empty() {
        format!("Cannot determine an ecosystem for origin {path:?}: no enabled ecosystem is supported")
    } else {
        format!(
            "Cannot determine an ecosystem for origin {path:?} (supported enabled ecosystems: {})",
            names.join(", ")
        )
    };
    Err(msg.into())
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scanner_for_a() {
        // every ecosystem has a scanner
        for e in Ecosystem::ALL {
            assert!(scanner_for(e).is_some(), "{e}");
        }
    }

    #[test]
    fn test_scanners_a() {
        assert_eq!(scanners(&Ecosystem::ALL).len(), Ecosystem::ALL.len());
        assert_eq!(scanners(&[Ecosystem::VsCode]).len(), 1);
    }

    #[test]
    fn test_infer_origins_a() {
        let o = infer_origins(Path::new("python3"), &Ecosystem::ALL).unwrap();
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].ecosystem, Ecosystem::Python);
        assert!(o[0].as_path().is_absolute());
    }

    #[test]
    fn test_infer_origins_b() {
        // python is not enabled
        assert!(infer_origins(Path::new("python3"), &[Ecosystem::Npm]).is_err());
        // not recognized by any scanner
        assert!(infer_origins(Path::new("/foo/bar.txt"), &Ecosystem::ALL).is_err());
    }
}
