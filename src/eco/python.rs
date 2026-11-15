use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::io::BufRead;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use crate::eco::head_lines;
use crate::eco::Artifacts;
use crate::eco::Detect;
use crate::eco::DirItem;
use crate::eco::EcosystemScanner;
use crate::eco::Hook;
use crate::eco::HOOK_HEAD_LINES;
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

/// File extensions in a site-packages directory that can run on interpreter startup: `.pth` files (lines starting with `import` are executed) and the customize modules.
const HOOK_EXTS: [&str; 2] = ["py", "pth"];
/// Python modules imported automatically on startup, if present in a site directory.
pub(crate) const PY_STARTUP_MODULES: [&str; 2] = ["sitecustomize.py", "usercustomize.py"];

/// All `.pth` files and startup modules directly in a site directory, sorted by name.
fn site_hooks(site: &Path) -> Vec<Hook> {
    let Ok(entries) = fs::read_dir(site) else {
        return Vec::new();
    };
    let mut hooks: Vec<Hook> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|fp| !fp.is_dir())
        .filter_map(|fp| {
            let ext = fp.extension().and_then(|e| e.to_str())?;
            let name = fp.file_name().and_then(|s| s.to_str())?.to_string();
            if !HOOK_EXTS.contains(&ext)
                || (ext == "py" && !PY_STARTUP_MODULES.contains(&name.as_str()))
            {
                return None;
            }
            match head_lines(&fp, HOOK_HEAD_LINES) {
                Ok(detail) => Some(Hook { name, detail }),
                Err(e) => {
                    eprintln!("Cannot load file {fp:?}: {e}");
                    None
                }
            }
        })
        .collect();
    hooks.sort();
    hooks
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
/// Installed artifacts of a Python package: the files listed in its dist-info `RECORD` (with whether each exists), plus the dist-info directory and the package's source directory, if found.
pub(crate) fn artifacts_from_record(
    package: &Package,
    site: &Site,
) -> ResultDynError<Artifacts> {
    let dir_dist_info = package
        .to_dist_info_dir(site)
        .ok_or("Cannot find dist-info dir")?;
    // parent of dist-info dir is site packages; all RECORD paths are relative to this
    let dir_site = dir_dist_info
        .parent()
        .ok_or("dist-info dir has no parent")?;
    let file = fs::File::open(dir_dist_info.join("RECORD"))?;
    // note: RECORD files might have redundancies
    let mut files = Vec::new();
    for line in io::BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(fp_rel) = line.split(',').next() {
            let fp = dir_site.join(fp_rel);
            let exists = fp.exists();
            files.push((fp, exists));
        }
    }
    let mut dirs = vec![dir_dist_info];
    if let Some(dir_src) = package.to_src_dir(site) {
        dirs.push(dir_src);
    };
    Ok(Artifacts { files, dirs })
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
                ..Default::default()
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
            ..Default::default()
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

    fn site_to_hooks(&self, site: &Site) -> Vec<Hook> {
        site_hooks(site.as_path())
    }

    fn site_to_artifacts(
        &self,
        site: &Site,
        wanted: &HashSet<&Package>,
    ) -> Vec<(Package, Artifacts)> {
        wanted
            .iter()
            .filter_map(|p| match artifacts_from_record(p, site) {
                Ok(a) => Some(((*p).clone(), a)),
                Err(e) => {
                    logger!(
                        true,
                        module_path!(),
                        "Failed to read artifacts of {} at {}: {}",
                        p,
                        site,
                        e
                    );
                    None
                }
            })
            .collect()
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
    fn test_site_hooks_a() {
        let dir = tempdir().unwrap();
        let site = dir.path();
        fs::write(site.join("zz.pth"), "import os; os.environ['X'] = '1'\n").unwrap();
        fs::write(site.join("aa.pth"), "/extra/path\n").unwrap();
        fs::write(
            site.join("sitecustomize.py"),
            "# customization\nimport sys\n",
        )
        .unwrap();
        fs::write(site.join("other.py"), "print(1)\n").unwrap();
        fs::write(site.join("README.txt"), "x").unwrap();
        fs::create_dir(site.join("dir.pth")).unwrap();
        let mut lines = String::new();
        for i in 0..20 {
            lines.push_str(&format!("line {i}\n"));
        }
        fs::write(site.join("long.pth"), lines).unwrap();

        let hooks = PythonScanner
            .site_to_hooks(&Site::python(PathShared::from(site.to_path_buf())));
        let names: Vec<&str> = hooks.iter().map(|h| h.name.as_str()).collect();
        // sorted; only .pth and startup modules; directories excluded
        assert_eq!(
            names,
            vec!["aa.pth", "long.pth", "sitecustomize.py", "zz.pth"]
        );
        assert_eq!(hooks[2].detail, "# customization import sys");
        // first 10 lines only
        assert!(
            hooks[1].detail.contains("line 9") && !hooks[1].detail.contains("line 10")
        );
        // a missing site yields none
        assert!(PythonScanner
            .site_to_hooks(&Site::python(PathShared::from(site.join("missing"))))
            .is_empty());
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

    #[test]
    fn test_record_a() {
        use std::io::Write;
        let pkg = Package::from_dist_info("xarray-0.21.1.dist-info", None, None).unwrap();
        let dir_temp = tempdir().unwrap(); // this is our site
        let site = Site::python(PathShared::from_path_buf(dir_temp.path().to_path_buf()));
        let dir_dist_info = dir_temp.path().join("xarray-0.21.1.dist-info");
        fs::create_dir(&dir_dist_info).unwrap();
        let fp_record = dir_dist_info.as_path().join("RECORD");

        let content = r#"
xarray-0.21.1.dist-info/INSTALLER,sha256=zuuue4knoyJ-UwPPXg8fezS7VCrXJQrAP7zeNuwvFQg,4
xarray-0.21.1.dist-info/LICENSE,sha256=c7p036pSC0mkAbXSFFmoUjoUbzt1GKgz7qXvqFEwv2g,10273
xarray-0.21.1.dist-info/METADATA,sha256=T6ewGJSP7S1OFMxt7eEcm-pKKjzyq0rx5pEGlFbe0ms,6008
xarray-0.21.1.dist-info/RECORD,,
xarray-0.21.1.dist-info/REQUESTED,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0
xarray-0.21.1.dist-info/WHEEL,sha256=G16H4A3IeoQmnOrYV4ueZGKSjhipXx8zc8nu9FGlvMA,92
xarray-0.21.1.dist-info/top_level.txt,sha256=OGV8AqTgYtuaw6YV6tevWXEdDI5vHJiARQCJgRyT7co,7
xarray/__init__.py,sha256=Kn7MQ1eaUQZVe5dyc8aYoVpr4iMaao5oEKWyA8TK_oQ,2826
xarray/__pycache__/__init__.cpython-311.pyc,,
xarray/__pycache__/tutorial.cpython-311.pyc,,
xarray/__pycache__/ufuncs.cpython-311.pyc,,
xarray/backends/__init__.py,sha256=SOkeUBf7KCR7ji-QYJranGhhto0sGfV3QHHeU_hSVoA,1100
xarray/backends/__pycache__/__init__.cpython-311.pyc,,
xarray/backends/__pycache__/store.cpython-311.pyc,,
xarray/backends/__pycache__/zarr.cpython-311.pyc,,
xarray/backends/api.py,sha256=NZsX3TXz_pUHKu9S50twpKlIri8AksMlpki2OaD_0jQ,54268
xarray/backends/rasterio_.py,sha256=S8cm7Zvz95rC9Ee3u7384x_E6GeBk6nVhnXKgmlcTTA,15482
xarray/backends/zarr.py,sha256=Ect1jrS0X4qlZlKCQ5CyrLxOWFzqh7i0vuUhzQ3-y_U,30946
xarray/coding/__init__.py,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0
xarray/coding/__pycache__/__init__.cpython-311.pyc,,
xarray/coding/__pycache__/variables.cpython-311.pyc,,
xarray/coding/calendar_ops.py,sha256=6Bt47kyLTjnxnV0wYnG7_glLtBWnyNU54ksuPWgICc8,13703
xarray/coding/cftime_offsets.py,sha256=WZQtWiCG34s7sR6OBo9ffqO6aZchpY-QofvmJwFNZ_c,42730
xarray/convert.py,sha256=E2Rocp9OeVll4le8WtqQWnlGVAZ7hhmXqtnDnL1G1Vk,9643
xarray/core/__init__.py,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0
xarray/core/__pycache__/__init__.cpython-311.pyc,,
xarray/core/__pycache__/_reductions.cpython-311.pyc,,
xarray/core/__pycache__/weighted.cpython-311.pyc,,
xarray/core/_reductions.py,sha256=ivDih2-exiXoLb5aXtvWbz5JDiiQxfIRKmjNHdAJ9w0,133749
xarray/core/_typed_ops.py,sha256=9Sw7vc3cLotwINQXSMZC5Or9R1dPExeslAQxxTODZKU,26243
xarray/core/_typed_ops.pyi,sha256=mPs9rWZaBwNfRG6dKbamkw_ryRPW9w_iIxB1no-TZds,31184
xarray/core/accessor_dt.py,sha256=0EoMBBMoGmje_efIez3BTq5WEvgYz5MtPt5GCgaqPus,18578
xarray/core/weighted.py,sha256=CQtBTXlLMCsiW_c6hsKSyduM1FwBS0LRe4gwTeL2zOw,11686
xarray/plot/__init__.py,sha256=mMMC5ySGsutPmnWQ5vbx9AKX0anr6z6Y2avjmNvo2Ro,329
xarray/plot/__pycache__/__init__.cpython-311.pyc,,
xarray/plot/__pycache__/dataset_plot.cpython-311.pyc,,
xarray/plot/__pycache__/utils.cpython-311.pyc,,
xarray/plot/dataset_plot.py,sha256=3RGm3LnFK62DDKAjt6RiNbhrxHAId-qpNBIYx_tJb_Y,20829
xarray/py.typed,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0
xarray/static/__init__.py,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0
xarray/static/__pycache__/__init__.cpython-311.pyc,,
xarray/static/css/__init__.py,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0
xarray/static/css/__pycache__/__init__.cpython-311.pyc,,43
xarray/testing.py,sha256=-WotNkBU9ch6AR9YK64ieaKtMkTBJNzzvUt_9xyhs5o,12412
xarray/tests/__init__.py,sha256=oUn-mBul4siTKundcFupxY6YuEHObLN8t8g11Rcq1TE,7077
xarray/tests/__pycache__/__init__.cpython-311.pyc,,
xarray/tests/__pycache__/conftest.cpython-311.pyc,,
xarray/tests/__pycache__/test_accessor_dt.cpython-311.pyc,,
xarray/tests/__pycache__/test_weighted.cpython-311.pyc,,
xarray/tests/conftest.py,sha256=E_8llREz2LY96ts6N2Pwoq08nQ96pC8sM3qfYJR5n24,169
xarray/tests/data/bears.nc,sha256=912tQ5fHIS-VDTBe3UplQi2rdRcreMSQ0tIdCOg9FRI,1184
xarray/tests/test_weighted.py,sha256=ryJPu4CD3sodDGN238HKUYzkcqCsgSl9jsYURTXBWZA,16265
xarray/tutorial.py,sha256=Aoau1-Tm1tCfxCWORaepseRwWZRL1CUooBLCbqUsxDc,7040
xarray/ufuncs.py,sha256=Rlykkrkk5ZAN-H6B9ZKYMP2uvbiwGmjEPfoY51dln0E,4562
xarray/util/__init__.py,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0
xarray/util/__pycache__/__init__.cpython-311.pyc,,
xarray/util/__pycache__/generate_ops.cpython-311.pyc,,
xarray/util/generate_ops.py,sha256=amEIE7w5momaWtwbl1wZY9jfVe2liYkweOHCRm8dxMs,9227
xarray/util/print_versions.py,sha256=kSqlh0crnpEzanhYmV3F7RuGEys8nrOhM_Yf_i7D7bM,5145
        "#;
        let mut file = File::create(&fp_record).unwrap();
        write!(file, "{}", content).unwrap();
        let rc = artifacts_from_record(&pkg, &site).unwrap();
        // println!("{:?}", rc);
        assert_eq!(rc.files.len(), 59);
        assert_eq!(rc.dirs.len(), 1);
    }
}
