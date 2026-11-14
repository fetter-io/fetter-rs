//! VS Code (and forks) extensions scanner.
//!
//! An origin is an editor data directory (e.g. `~/.vscode`, `~/.cursor`; see `EDITOR_DIRS`) that contains an `extensions` directory. A project's `.vscode` settings folder has no `extensions` directory (at most an `extensions.json` file), so it is not mistaken for one. Its `extensions` directory is a site (`SiteKind::Installed`) whose packages are the installed extensions, named `publisher.name` with the case from each extension's `package.json` (OSV names are case-sensitive).
//!
//! Installed extensions are those listed in `extensions/extensions.json`. Editors retain old versions until cleanup, marked in `extensions/.obsolete`; without `extensions.json`, the installed set is every extension directory not marked obsolete.
//!
//! An installed extension that bundles `node_modules` contributes that directory as an additional site of the npm ecosystem (read by the npm scanner), so vulnerable dependencies running inside the editor are reported and traceable to their extension by path.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde_json::Value as JsonValue;

use crate::eco::Detect;
use crate::eco::DirItem;
use crate::eco::EcosystemScanner;
use crate::eco::EDITOR_DIRS;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::origin_site::SiteKind;
use crate::package::Package;
use crate::path_shared::PathShared;
use crate::util::path_home;
use crate::util::path_normalize;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::ScanConfig;
use crate::version_spec::VersionSpec;

const EXTENSIONS: &str = "extensions";
const EXTENSIONS_JSON: &str = "extensions.json";
const OBSOLETE: &str = ".obsolete";
const PACKAGE_JSON: &str = "package.json";
const NODE_MODULES: &str = "node_modules";

//------------------------------------------------------------------------------
/// An editor directory: named in `EDITOR_DIRS` and containing an `extensions` directory.
fn is_editor_dir(dir: &Path) -> bool {
    dir.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| EDITOR_DIRS.contains(&n))
        && dir.join(EXTENSIONS).is_dir()
}

fn origin(dir: PathBuf) -> Origin {
    Origin::new(Ecosystem::VsCode, PathShared::from(dir))
}

/// Directories of installed extensions in an `extensions` directory. `extensions.json` entries give a `relativeLocation` (or an absolute `location.path`); entries whose directory is missing are skipped. Without a readable `extensions.json`, all extension directories not marked in `.obsolete` are used.
fn installed_extensions(ext_dir: &Path) -> Vec<PathBuf> {
    if let Some(entries) = fs::read_to_string(ext_dir.join(EXTENSIONS_JSON))
        .ok()
        .and_then(|c| serde_json::from_str::<JsonValue>(&c).ok())
        .and_then(|v| v.as_array().cloned())
    {
        return entries
            .iter()
            .filter_map(|e| {
                e.get("relativeLocation")
                    .and_then(JsonValue::as_str)
                    .map(|r| ext_dir.join(r))
                    .or_else(|| {
                        e.get("location")
                            .and_then(|l| l.get("path"))
                            .and_then(JsonValue::as_str)
                            .map(PathBuf::from)
                    })
            })
            .filter(|p| p.is_dir())
            .collect();
    }
    let obsolete: HashSet<String> = fs::read_to_string(ext_dir.join(OBSOLETE))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Map<String, JsonValue>>(&c).ok())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    fs::read_dir(ext_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| !n.starts_with('.') && !obsolete.contains(n))
        })
        .map(|e| e.path())
        .collect()
}

/// An extension's identity from its `package.json`: `publisher.name` (case preserved) and version.
fn extension_package(ext: &Path) -> Option<Package> {
    let content = fs::read_to_string(ext.join(PACKAGE_JSON)).ok()?;
    let pj: JsonValue = serde_json::from_str(&content).ok()?;
    let publisher = pj.get("publisher")?.as_str()?;
    let name = pj.get("name")?.as_str()?;
    let version = pj.get("version")?.as_str()?;
    Some(Package::new(
        Ecosystem::VsCode,
        &format!("{publisher}.{name}"),
        VersionSpec::new(version),
        None,
    ))
}

/// Editor directories under the home directory.
fn home_editor_dirs() -> Vec<PathBuf> {
    let Some(home) = path_home() else {
        return Vec::new();
    };
    EDITOR_DIRS
        .iter()
        .map(|d| home.join(d))
        .filter(|d| is_editor_dir(d))
        .collect()
}

/// An editor directory given by `VSCODE_EXTENSIONS` (a custom extensions directory), as its parent.
fn env_editor_dir() -> Option<PathBuf> {
    let ext = PathBuf::from(env::var_os("VSCODE_EXTENSIONS").filter(|v| !v.is_empty())?);
    let parent = ext.parent()?;
    (ext.file_name().is_some_and(|n| n == EXTENSIONS) && ext.is_dir())
        .then(|| parent.to_path_buf())
}

//------------------------------------------------------------------------------
pub(crate) struct VsCodeScanner;

impl EcosystemScanner for VsCodeScanner {
    fn default_origins(&self) -> Vec<Origin> {
        let mut dirs = home_editor_dirs();
        dirs.extend(env_editor_dir());
        dirs.into_iter().map(origin).collect()
    }

    /// Claim an editor directory and do not descend: its contents are extensions (read from the origin) and editor state.
    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect {
        let named = dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| EDITOR_DIRS.contains(&n));
        let has_ext_dir = items
            .iter()
            .any(|i| i.file_name() == Some(EXTENSIONS) && i.file_type.is_dir());
        if named && has_ext_dir {
            return Detect {
                origins: vec![origin(dir.to_path_buf())],
                prune: true,
                ..Default::default()
            };
        }
        Detect::default()
    }

    /// Accept an editor directory or its `extensions` directory.
    fn accepts(&self, path: &Path) -> bool {
        let Ok(p) = path_normalize(path, false) else {
            return false;
        };
        is_editor_dir(&p)
            || (p.file_name().is_some_and(|n| n == EXTENSIONS)
                && p.parent().is_some_and(is_editor_dir))
    }

    fn normalize_origin(&self, path: &Path) -> ResultDynError<Origin> {
        let p = path_normalize(path, true)?;
        if is_editor_dir(&p) {
            return Ok(origin(p));
        }
        match p.parent() {
            Some(parent) if is_editor_dir(parent) => Ok(origin(parent.to_path_buf())),
            _ => Err(format!("Not an editor extensions directory: {p:?}").into()),
        }
    }

    /// The `extensions` directory, plus an npm site for each installed extension's bundled `node_modules`.
    fn origin_to_sites(
        &self,
        origin: &Origin,
        _config: &ScanConfig,
        _log: FlagLog,
    ) -> Vec<Site> {
        let ext_dir = origin.as_path().join(EXTENSIONS);
        if !ext_dir.is_dir() {
            return Vec::new();
        }
        let mut sites = vec![Site::new(
            Ecosystem::VsCode,
            SiteKind::Installed,
            PathShared::from(ext_dir.clone()),
        )];
        for ext in installed_extensions(&ext_dir) {
            let nm = ext.join(NODE_MODULES);
            if nm.is_dir() {
                sites.push(Site::new(
                    Ecosystem::Npm,
                    SiteKind::Installed,
                    PathShared::from(nm),
                ));
            }
        }
        sites
    }

    fn site_to_packages(&self, site: &Site) -> Vec<Package> {
        let unique: HashSet<Package> = installed_extensions(site.as_path())
            .iter()
            .filter_map(|e| extension_package(e))
            .collect();
        unique.into_iter().collect()
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::eco::scanner_for;
    use tempfile::tempdir;

    fn write_ext(
        ext_dir: &Path,
        dir_name: &str,
        publisher: &str,
        name: &str,
        version: &str,
    ) -> PathBuf {
        let d = ext_dir.join(dir_name);
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join(PACKAGE_JSON),
            format!(r#"{{"publisher": "{publisher}", "name": "{name}", "version": "{version}", "engines": {{"vscode": "^1.80.0"}}}}"#),
        )
        .unwrap();
        d
    }

    /// An editor dir with two installed extensions (one bundling node_modules), one obsolete older version, and an extensions.json.
    fn make_editor(root: &Path) -> PathBuf {
        let editor = root.join(".vscode");
        let ext_dir = editor.join(EXTENSIONS);
        write_ext(
            &ext_dir,
            "codeinklingon.git-worktree-menu-1.0.9",
            "CodeInKlingon",
            "git-worktree-menu",
            "1.0.9",
        );
        let cpp = write_ext(
            &ext_dir,
            "ms-vscode.cpptools-1.35.6-darwin-arm64",
            "ms-vscode",
            "cpptools",
            "1.35.6",
        );
        write_ext(
            &ext_dir,
            "ms-vscode.cpptools-1.34.0-darwin-arm64",
            "ms-vscode",
            "cpptools",
            "1.34.0",
        );
        let lodash = cpp.join("node_modules/lodash");
        fs::create_dir_all(&lodash).unwrap();
        fs::write(
            lodash.join(PACKAGE_JSON),
            r#"{"name": "lodash", "version": "4.17.15"}"#,
        )
        .unwrap();
        fs::write(
            ext_dir.join(EXTENSIONS_JSON),
            r#"[{"identifier": {"id": "codeinklingon.git-worktree-menu"}, "version": "1.0.9", "relativeLocation": "codeinklingon.git-worktree-menu-1.0.9"},
                {"identifier": {"id": "ms-vscode.cpptools"}, "version": "1.35.6", "relativeLocation": "ms-vscode.cpptools-1.35.6-darwin-arm64"},
                {"identifier": {"id": "gone.ext"}, "version": "1.0.0", "relativeLocation": "gone.ext-1.0.0"}]"#,
        )
        .unwrap();
        fs::write(
            ext_dir.join(OBSOLETE),
            r#"{"ms-vscode.cpptools-1.34.0-darwin-arm64": true}"#,
        )
        .unwrap();
        editor
    }

    fn names(packages: &[Package]) -> Vec<String> {
        let mut v: Vec<String> = packages.iter().map(|p| p.to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn test_installed_from_extensions_json_a() {
        let dir = tempdir().unwrap();
        let editor = make_editor(dir.path());
        let site = Site::new(
            Ecosystem::VsCode,
            SiteKind::Installed,
            PathShared::from(editor.join(EXTENSIONS)),
        );
        let packages = VsCodeScanner.site_to_packages(&site);
        // only installed versions; a listed but missing dir is skipped; publisher case is preserved
        assert_eq!(
            names(&packages),
            vec![
                "CodeInKlingon.git-worktree-menu-1.0.9",
                "ms-vscode.cpptools-1.35.6"
            ]
        );
        let p = packages
            .iter()
            .find(|p| p.name.starts_with("CodeInKlingon"))
            .unwrap();
        assert_eq!(p.key, "codeinklingon.git-worktree-menu");
        assert_eq!(p.ecosystem, Ecosystem::VsCode);
    }

    #[test]
    fn test_installed_from_obsolete_a() {
        // without extensions.json: every extension dir not marked obsolete
        let dir = tempdir().unwrap();
        let editor = make_editor(dir.path());
        fs::remove_file(editor.join(EXTENSIONS).join(EXTENSIONS_JSON)).unwrap();
        let mut installed: Vec<String> = installed_extensions(&editor.join(EXTENSIONS))
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        installed.sort();
        assert_eq!(
            installed,
            vec![
                "codeinklingon.git-worktree-menu-1.0.9",
                "ms-vscode.cpptools-1.35.6-darwin-arm64"
            ]
        );
    }

    #[test]
    fn test_origin_to_sites_a() {
        let dir = tempdir().unwrap();
        let editor = make_editor(dir.path());
        // the obsolete version also bundles node_modules: it must not become a site
        fs::create_dir_all(
            editor.join("extensions/ms-vscode.cpptools-1.34.0-darwin-arm64/node_modules"),
        )
        .unwrap();
        let config = ScanConfig::new(false, false);
        let mut sites = VsCodeScanner.origin_to_sites(
            &origin(editor.clone()),
            &config,
            FlagLog(false),
        );
        sites.sort();
        assert_eq!(sites.len(), 2);
        let npm_site = sites
            .iter()
            .find(|s| s.ecosystem == Ecosystem::Npm)
            .unwrap();
        assert!(npm_site
            .as_path()
            .ends_with("ms-vscode.cpptools-1.35.6-darwin-arm64/node_modules"));
        assert_eq!(npm_site.kind, SiteKind::Installed);
        // the npm site is read by the npm scanner
        let packages = scanner_for(npm_site.ecosystem)
            .unwrap()
            .site_to_packages(npm_site);
        assert_eq!(names(&packages), vec!["lodash-4.17.15"]);
        assert_eq!(packages[0].ecosystem, Ecosystem::Npm);
    }

    fn items_of(dir: &Path) -> Vec<DirItem> {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| DirItem {
                path: e.path(),
                file_type: e.file_type().unwrap(),
            })
            .collect()
    }

    #[test]
    fn test_detect_a() {
        let dir = tempdir().unwrap();
        let editor = make_editor(dir.path());
        let d = VsCodeScanner.detect(&editor, &items_of(&editor));
        assert_eq!(d.origins.len(), 1);
        assert!(d.prune);
        // a project's .vscode settings folder (with an extensions.json file, not dir) is not an editor dir
        let proj = dir.path().join("proj/.vscode");
        fs::create_dir_all(&proj).unwrap();
        fs::write(proj.join("settings.json"), "{}").unwrap();
        fs::write(proj.join(EXTENSIONS_JSON), r#"{"recommendations": []}"#).unwrap();
        let d = VsCodeScanner.detect(&proj, &items_of(&proj));
        assert!(d.origins.is_empty());
        assert!(!d.prune);
        // an extensions dir under a non-editor name is not one either
        let other = dir.path().join("src");
        fs::create_dir_all(other.join(EXTENSIONS)).unwrap();
        assert!(VsCodeScanner
            .detect(&other, &items_of(&other))
            .origins
            .is_empty());
    }

    #[test]
    fn test_accepts_normalize_a() {
        let dir = tempdir().unwrap();
        let editor = make_editor(dir.path());
        for p in [editor.clone(), editor.join(EXTENSIONS)] {
            assert!(VsCodeScanner.accepts(&p), "{p:?}");
            assert_eq!(
                VsCodeScanner.normalize_origin(&p).unwrap().as_path(),
                editor
            );
        }
        assert!(!VsCodeScanner
            .accepts(&editor.join("extensions/ms-vscode.cpptools-1.35.6-darwin-arm64")));
        assert!(!VsCodeScanner.accepts(dir.path()));
    }

    #[test]
    fn test_extension_package_missing_fields_a() {
        let dir = tempdir().unwrap();
        let d = dir.path().join("x");
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join(PACKAGE_JSON),
            r#"{"name": "nopublisher", "version": "1.0.0"}"#,
        )
        .unwrap();
        assert!(extension_package(&d).is_none());
    }
}
