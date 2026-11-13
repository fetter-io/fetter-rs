use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use crate::eco::Detect;
use crate::eco::DirItem;
use crate::eco::EcosystemScanner;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::package::Package;
use crate::path_shared::PathShared;
use crate::util::default_python_exe_names;
use crate::util::exe_path_normalize;
use crate::util::get_absolute_path_from_exe;
use crate::util::is_python_exe;
use crate::util::is_python_exe_file_name;
use crate::util::logger;
use crate::util::path_normalize;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::ScanConfig;

//------------------------------------------------------------------------------
/// Given a path to a Python binary, call out to Python to get all known site packages; some site packages may not exist; we do not filter them here. This will include "dist-packages" on Linux. If `config.force_usite` is false, we use site.ENABLE_USER_SITE to determine if we should include the user site packages; if `config.force_usite` is true, we always include usite.
/// Calling Python using `-S` disables loading site so that we can mock sitecustomize.py (which fetter might customize). We then call `site.main()` to force proper initialization.
const PY_SITE_PACKAGES: &str = "import sys;import site;import types;sys.modules['fetter_validate'] = types.ModuleType('fetter_validate');site.main();print(site.ENABLE_USER_SITE);print(\"\\n\".join(site.getsitepackages()));print(site.getusersitepackages())";

fn get_site_package_dirs(
    executable: &Path,
    config: &ScanConfig,
    log: FlagLog,
) -> Vec<PathShared> {
    match Command::new(executable)
        .arg("-S") // disable site on startup
        .arg("-c")
        .arg(PY_SITE_PACKAGES)
        .output()
    {
        Ok(output) => {
            let mut paths = Vec::new();
            let mut usite_enabled = false;

            let lines = std::str::from_utf8(&output.stdout)
                .expect("Failed to convert to UTF-8")
                .trim()
                .lines();
            for (i, line) in lines.enumerate() {
                if i == 0 {
                    usite_enabled = line.trim() == "True";
                } else {
                    paths.push(PathShared::from(line.trim()));
                }
            }
            // if necessary, remove the usite
            if !config.force_usite && !usite_enabled {
                let _p = paths.pop();
            }
            paths
        }
        Err(e) => {
            logger!(
                log,
                module_path!(),
                "Failed to execute command with {:?}: {}",
                executable,
                e
            );

            Vec::with_capacity(0)
        }
    }
}

/// Given a site-packages directory, collect all packages from dist-info directories.
fn get_packages(site_packages: &Path) -> Vec<Package> {
    let mut packages = Vec::new();
    if let Ok(entries) = fs::read_dir(site_packages) {
        for entry in entries.flatten() {
            let file_path = entry.path();
            if let Some(package) = Package::from_file_path(&file_path) {
                packages.push(package);
            }
        }
    }
    packages
}

/// Provide directories that should be searched for executables in addition to home directories. Returns a set of tuples of PathBuf, bool, where the bool indicates if the directory should be recursively searched.
fn get_origins_bin() -> HashSet<(PathBuf, bool)> {
    let mut paths: HashSet<(PathBuf, bool)> = HashSet::new();
    // get all paths on PATH; split_paths handles the platform separator (":" on
    // Unix, ";" on Windows) as well as quoted entries.
    if let Some(path_var) = env::var_os("PATH") {
        for path in env::split_paths(&path_var) {
            paths.insert((path, false));
        }
    }
    if env::consts::OS == "windows" {
        // Windows has no system "bin" convention; seed known Python install
        // roots (searched recursively) so interpreters not on PATH are found.
        for var in ["LOCALAPPDATA", "PROGRAMFILES", "PROGRAMFILES(X86)"] {
            if let Some(base) = env::var_os(var) {
                let root = if var == "LOCALAPPDATA" {
                    PathBuf::from(base).join("Programs").join("Python")
                } else {
                    PathBuf::from(base).join("Python")
                };
                if root.is_dir() {
                    paths.insert((root, true));
                }
            }
        }
    } else {
        paths.insert((PathBuf::from("/bin"), false));
        paths.insert((PathBuf::from("/sbin"), false));
        paths.insert((PathBuf::from("/usr/bin"), false));
        paths.insert((PathBuf::from("/usr/sbin"), false));
        paths.insert((PathBuf::from("/usr/local/bin"), false));
        paths.insert((PathBuf::from("/usr/local/sbin"), false));
        if env::consts::OS == "macos" {
            paths.insert((PathBuf::from("/opt/homebrew/bin"), false));
        }
    }
    paths
}

/// Within a virtual environment directory (one that contains `pyvenv.cfg`), the interpreter is at a known relative location.
fn venv_exe(dir: &Path) -> PathBuf {
    if env::consts::OS == "windows" {
        dir.join("Scripts").join("python.exe")
    } else {
        dir.join("bin").join("python3")
    }
}

fn origin(exe: PathBuf) -> Origin {
    Origin::new(Ecosystem::Python, PathShared::from(exe))
}

//------------------------------------------------------------------------------
/// Origins are Python executables; Sites are site-packages directories; Packages are read from `*.dist-info` directories.
pub(crate) struct PythonScanner;

impl EcosystemScanner for PythonScanner {
    fn roots(&self) -> Vec<(PathBuf, bool)> {
        get_origins_bin().into_iter().collect()
    }

    /// The interpreter name varies by platform ("python" first on Windows), so try each candidate and take the first that resolves.
    fn default_origins(&self) -> Vec<Origin> {
        default_python_exe_names()
            .iter()
            .find_map(|name| get_absolute_path_from_exe(name))
            .map(origin)
            .into_iter()
            .collect()
    }

    fn detect(&self, dir: &Path, items: &[DirItem]) -> Detect {
        // A virtual environment: take only its interpreter and do not descend.
        if items.iter().any(|i| i.file_name() == Some("pyvenv.cfg")) {
            let exe = venv_exe(dir);
            let origins = if exe.exists() && is_python_exe(&exe) {
                vec![origin(exe)]
            } else {
                Vec::new()
            };
            return Detect {
                origins,
                prune: true,
            };
        }
        // Test all non-directory entries (including symlinks) as interpreters.
        let origins = items
            .iter()
            .filter(|i| !i.file_type.is_dir() && is_python_exe(&i.path))
            .map(|i| origin(i.path.clone()))
            .collect();
        Detect {
            origins,
            prune: false,
        }
    }

    /// Accept paths named like an interpreter (`python3`, `/usr/bin/python3.12`, `python.exe`) or a virtual environment directory.
    fn accepts(&self, path: &Path) -> bool {
        is_python_exe_file_name(path)
            || path_normalize(path, false).is_ok_and(|p| p.join("pyvenv.cfg").is_file())
    }

    fn normalize_origin(&self, path: &Path) -> ResultDynError<Origin> {
        let fp = path_normalize(path, false)?;
        if fp.join("pyvenv.cfg").is_file() {
            let exe = venv_exe(&fp);
            if !is_python_exe(&exe) {
                return Err(format!(
                    "No interpreter found in virtual environment: {fp:?}"
                )
                .into());
            }
            return Ok(origin(exe));
        }
        Ok(origin(exe_path_normalize(path)?))
    }

    fn origin_to_sites(
        &self,
        origin: &Origin,
        config: &ScanConfig,
        log: FlagLog,
    ) -> Vec<Site> {
        get_site_package_dirs(origin.as_path(), config, log)
            .into_iter()
            .map(Site::python)
            .collect()
    }

    fn site_to_packages(&self, site: &Site) -> Vec<Package> {
        get_packages(site.as_path())
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

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

    #[cfg(unix)]
    fn make_exe(fp: &Path) {
        let _ = File::create(fp).unwrap();
        let mut perms = fs::metadata(fp).unwrap().permissions();
        perms.set_mode(0o755); // rwxr-xr-x (755) for an executable script
        fs::set_permissions(fp, perms).unwrap();
    }

    #[test]
    fn test_get_site_package_dirs_a() {
        let p1 = Path::new("python3");
        let config_force = ScanConfig::new(true, false);
        let paths1 = get_site_package_dirs(p1, &config_force, FlagLog(false));
        assert!(!paths1.is_empty());
        let config_no_force = ScanConfig::new(false, false);
        let paths2 = get_site_package_dirs(p1, &config_no_force, FlagLog(false));
        assert!(paths1.len() >= paths2.len());
    }

    #[test]
    fn test_get_origins_bin_a() {
        let post = get_origins_bin();
        #[cfg(unix)]
        assert!(post.len() > 6);
        // On Windows origins come from PATH (non-empty in any CI/shell env).
        #[cfg(windows)]
        assert!(!post.is_empty());
    }

    #[test]
    fn test_default_origins_a() {
        let origins = PythonScanner.default_origins();
        assert_eq!(origins.len(), 1);
        assert_eq!(origins[0].ecosystem, Ecosystem::Python);
    }

    #[test]
    fn test_get_packages_a() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("numpy-2.1.2.dist-info")).unwrap();
        fs::create_dir(dir.path().join("numpy")).unwrap();
        let site = Site::python(PathShared::from(dir.path().to_path_buf()));
        let packages = PythonScanner.site_to_packages(&site);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].to_string(), "numpy-2.1.2");
    }

    #[cfg(windows)]
    #[test]
    fn test_is_exe_win_a() {
        let temp_dir = tempdir().unwrap();
        let fp = temp_dir.path().join("python.exe");
        let _ = File::create(fp.clone()).unwrap();
        assert!(is_python_exe(&fp));
    }

    #[cfg(windows)]
    #[test]
    fn test_is_exe_win_b() {
        // A file named like an interpreter but without ".exe" is not executable.
        let temp_dir = tempdir().unwrap();
        let fp = temp_dir.path().join("python3");
        let _ = File::create(fp.clone()).unwrap();
        assert!(!is_python_exe(&fp));
    }

    #[cfg(windows)]
    #[test]
    fn test_detect_venv_win_a() {
        let temp_dir = tempdir().unwrap();
        let fpd1 = temp_dir.path();
        let _ = File::create(fpd1.join("pyvenv.cfg")).unwrap();
        let fpd2 = fpd1.join("Scripts");
        fs::create_dir(fpd2.clone()).unwrap();
        let _ = File::create(fpd2.join("python.exe")).unwrap();

        let d = PythonScanner.detect(fpd1, &items_of(fpd1));
        assert!(d.prune);
        assert_eq!(d.origins.len(), 1);
        assert!(d.origins[0]
            .as_path()
            .ends_with(PathBuf::from("Scripts").join("python.exe")));
    }

    #[cfg(unix)]
    #[test]
    fn test_is_exe_a() {
        let temp_dir = tempdir().unwrap();
        let fp = temp_dir.path().join("test.sh");
        make_exe(&fp);
        assert!(!is_python_exe(&fp));
    }

    #[cfg(unix)]
    #[test]
    fn test_is_exe_b() {
        let temp_dir = tempdir().unwrap();
        let fp = temp_dir.path().join("python");
        make_exe(&fp);
        assert!(is_python_exe(&fp));
    }

    #[cfg(unix)]
    #[test]
    fn test_is_exe_c() {
        let temp_dir = tempdir().unwrap();
        let fp = temp_dir.path().join("python10.100");
        make_exe(&fp);
        assert!(is_python_exe(&fp));
    }

    #[cfg(unix)]
    #[test]
    fn test_detect_venv_a() {
        let temp_dir = tempdir().unwrap();
        let fpd1 = temp_dir.path();
        let _ = File::create(fpd1.join("pyvenv.cfg")).unwrap();
        let fpd2 = fpd1.join("bin");
        fs::create_dir(fpd2.clone()).unwrap();
        make_exe(&fpd2.join("python3"));

        let d = PythonScanner.detect(fpd1, &items_of(fpd1));
        assert!(d.prune);
        assert_eq!(d.origins.len(), 1);
        assert!(d.origins[0].as_path().ends_with("bin/python3"));
    }

    #[cfg(unix)]
    #[test]
    fn test_venv_origin_a() {
        let temp_dir = tempdir().unwrap();
        let fpd1 = temp_dir.path();
        let _ = File::create(fpd1.join("pyvenv.cfg")).unwrap();
        fs::create_dir(fpd1.join("bin")).unwrap();
        make_exe(&fpd1.join("bin/python3"));

        assert!(PythonScanner.accepts(fpd1));
        assert!(!PythonScanner.accepts(&fpd1.join("bin")));
        let o = PythonScanner.normalize_origin(fpd1).unwrap();
        assert_eq!(o.as_path(), fpd1.join("bin/python3"));
    }

    #[test]
    fn test_accepts_a() {
        assert!(PythonScanner.accepts(Path::new("python3")));
        assert!(PythonScanner.accepts(Path::new("/usr/bin/python3.12")));
        assert!(!PythonScanner.accepts(Path::new("/usr/bin/node")));
    }

    #[cfg(unix)]
    #[test]
    fn test_detect_bin_a() {
        let temp_dir = tempdir().unwrap();
        let fpd1 = temp_dir.path();
        make_exe(&fpd1.join("python3.12"));
        make_exe(&fpd1.join("pip"));
        fs::create_dir(fpd1.join("python3")).unwrap(); // a dir, not an exe

        let d = PythonScanner.detect(fpd1, &items_of(fpd1));
        assert!(!d.prune);
        assert_eq!(d.origins.len(), 1);
        assert!(d.origins[0].as_path().ends_with("python3.12"));
    }
}
