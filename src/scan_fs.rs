use std::collections::HashSet;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::fs;
use std::process::Command;
use std::env;
use std::os::unix::fs::PermissionsExt;

use rayon::prelude::*;

use crate::package::Package;
use crate::dep_manifest::DepManifest;

//------------------------------------------------------------------------------
// Provide absolute paths for directories that should be excluded from executable search.
fn get_exclude_path() -> HashSet<PathBuf> {
    let mut paths: HashSet<PathBuf> = HashSet::new();
    match env::var("HOME") {
        Ok(home) => {
            paths.insert(PathBuf::from(home.clone()).join(".cache"));
            paths.insert(PathBuf::from(home.clone()).join(".npm"));

            if env::consts::OS == "macos" {
                paths.insert(PathBuf::from(home.clone()).join("Library"));
                paths.insert(PathBuf::from(home.clone()).join("Photos"));
                paths.insert(PathBuf::from(home.clone()).join("Downloads"));
                paths.insert(PathBuf::from(home.clone()).join(".Trash"));
            } else if env::consts::OS == "linux" {
                paths.insert(PathBuf::from(home.clone()).join(".local/share/Trash"));
            }
        }
        Err(e) => { // log this
            eprintln!("Error getting HOME {}", e);
        }
    }
    paths
}

// Provide directories that should be used as origins for searching for executables. Returns a vector of PathBuf, bool, where the bool indicates if the directory should be recursively searched.
fn get_exe_origins() -> Vec<(PathBuf, bool)> {
    let mut paths: Vec<(PathBuf, bool)> = Vec::new();
    match env::var("HOME") {
        Ok(home) => {
            paths.push((PathBuf::from(home.clone()), false));
            // collect all directories in the user's home directory
            match fs::read_dir(PathBuf::from(home)) {
                Ok(entries) => {
                    for entry in entries {
                        let path = entry.unwrap().path();
                        if path.is_dir() {
                            paths.push((path, true));
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error reading home: {}", e);
                }
            }
        }
        Err(e) => { // log this
            eprintln!("Error getting HOME {}", e);
        }
    }
    paths.push((PathBuf::from("/bin"), false));
    paths.push((PathBuf::from("/sbin"), false));
    paths.push((PathBuf::from("/usr/bin"), false));
    paths.push((PathBuf::from("/usr/sbin"), false));
    paths.push((PathBuf::from("/usr/local/bin"), false));
    paths.push((PathBuf::from("/usr/local/sbin"), false));
    if env::consts::OS == "macos" {
        paths.push((PathBuf::from("/opt/homebrew/bin"), false));
    }
    paths
}

// Return True if the path points to a python executable. We assume this has already been proven to exist.
fn is_exe(path: &Path) -> bool {
    return match path.file_name().and_then(|f| f.to_str()) {
        Some(file_name) if file_name.starts_with("python") => {
            let suffix = &file_name[6..];
            if suffix.is_empty() || suffix.chars().all(|c| c.is_digit(10) || c == '.') {
                match fs::metadata(path) {
                    Ok(md) => md.permissions().mode() & 0o111 != 0,
                    Err(_) => false,
                }
            } else {
                false
            }
        }
        _ => false,
    };
}

fn is_symlink(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(metadata) => metadata.file_type().is_symlink(),
        Err(_) => false,
    }
}

// Use the default Python to and get its executable path.
fn get_exe_default() -> Option<PathBuf> {
    return match Command::new("python3")
            .arg("-c")
            .arg("import sys;print(sys.executable)")
            .output() {
        Ok(output) => {
            match std::str::from_utf8(&output.stdout) {
                Ok(s) => Some(PathBuf::from(s.trim())),
                Err(_) => None,
            }
        }
        Err(_) => None,
    }
}
/// Try to find all Python executables given a starting directory. This will recursively search all directories that are not symlinks.
fn scan_executables_inner(
        path: &Path,
        exclude_paths: &HashSet<PathBuf>,
        recurse: bool,
        ) -> Vec<PathBuf> {
    if exclude_paths.contains(path) {
        return Vec::with_capacity(0);
    }
    let mut paths = Vec::new();
    if path.is_dir() {
        // if we find "fpdir/pyvenv.cfg", we can always get fpdir/bin/python3
        let path_cfg = path.to_path_buf().join("pyvenv.cfg");
        if path_cfg.exists() {
            let path_exe = path.to_path_buf().join("bin/python3");
            if path_exe.exists() && is_exe(&path_exe) {
                paths.push(path_exe)
            }
        }
        else {
            match fs::read_dir(path) {
                Ok(entries) => {
                    for entry in entries {
                        let path = entry.unwrap().path();
                        if recurse && path.is_dir() && !is_symlink(&path) { // recurse
                            // println!("recursing: {:?}", path);
                            paths.extend(scan_executables_inner(&path, exclude_paths, recurse));
                        } else if is_exe(&path) {
                            paths.push(path);
                        }
                    }
                }
                Err(e) => { // log this?
                    eprintln!("Error reading {:?}: {}", path, e);
                }
            }
        }
    }
    paths
}

// After collecting origins, find all executables
fn scan_executables() -> HashSet<PathBuf> {
    let exclude = get_exclude_path();
    let origins = get_exe_origins();

    let mut paths: HashSet<PathBuf> = origins
            .par_iter()
            .flat_map(|(path, recurse)| scan_executables_inner(path, &exclude, *recurse))
            .collect();
    if let Some(exe_def) = get_exe_default() {
        paths.insert(exe_def);
    }
    paths
}

/// Given a path to a Python binary, call out to Python to get all known site packages; some site packages may not exist; we do not filter them here. This will include "dist-packages" on Linux.
fn get_site_package_dirs(executable: &Path) -> Vec<PathBuf> {
    return match Command::new(executable)
            .arg("-c")
            .arg("import site;print(\"\\n\".join(site.getsitepackages()));print(site.getusersitepackages())") // since Python 3.2
            .output() {
        Ok(output) => {
            let paths_lines = std::str::from_utf8(&output.stdout)
                    .expect("Failed to convert to UTF-8")
                    .trim();
            paths_lines
                    .lines()
                    .map(|line| PathBuf::from(line.trim()))
                    .collect()
        }
        Err(e) => {
            eprintln!("Failed to execute command: {}", e); // log this
            Vec::with_capacity(0)
        }
    }
}

//------------------------------------------------------------------------------
// Given a package directory, collect the name of all packages.
fn get_packages(site_packages: &Path) -> Vec<Package> {
    let mut packages = Vec::new();
    if let Ok(entries) = fs::read_dir(site_packages) {
        for entry in entries {
            if let Ok(entry) = entry {
                if let Some(file_name) = entry.path().file_name().and_then(
                            |name| name.to_str()) {
                    if let Some(package) = Package::from_dist_info(file_name) {
                        packages.push(package);
                    }
                }
            }
        }
    }
    packages
}

//------------------------------------------------------------------------------
// #[derive(Debug)]
pub(crate) struct ScanFS {
    exe_to_sites: HashMap<PathBuf, Vec<PathBuf>>,
    package_to_sites: HashMap<Package, Vec<PathBuf>>,
}

// The results of a file-system scan.
impl ScanFS {
    pub(crate) fn from_exe_to_sites(
            exe_to_sites: HashMap<PathBuf, Vec<PathBuf>>,
            ) -> Result<Self, String> {
        // Some site packages will be repeated; let them be processed more than once here, as it seems easier than filtering them out
        let site_to_packages = exe_to_sites
                .par_iter()
                .flat_map(|(_, site_packages)| {
                    site_packages.par_iter().map(|site_package_path| {
                        let packages = get_packages(&site_package_path);
                        (site_package_path.clone(), packages)
                    })
                })
                .collect::<HashMap<PathBuf, Vec<Package>>>();

        let mut package_to_sites: HashMap<Package, Vec<PathBuf>> = HashMap::new();
        for (site_package_path, packages) in site_to_packages.iter() {
            for package in packages {
                package_to_sites
                    .entry(package.clone())
                    .or_insert_with(Vec::new)
                    .push(site_package_path.clone());
            }
        }
        Ok(ScanFS {
            exe_to_sites,
            package_to_sites,
        })
    }
    pub(crate) fn from_defaults() -> Result<Self, String> {
        // For every unique exe, we hae a list of site packages; some site packages might be associated with more than one exe, meaning that a reverse lookup would have to be site-package to Vec of exe
        let exe_to_sites: HashMap<PathBuf, Vec<PathBuf>> = scan_executables()
                .into_par_iter()
                .map(|exe| {
                    let dirs = get_site_package_dirs(&exe);
                    (exe, dirs)
                })
                .collect();
        Self::from_exe_to_sites(exe_to_sites)
    }
    //--------------------------------------------------------------------------
    /// The length of the scan is then number of unique packages.
    pub fn len(&self) -> usize {
        self.package_to_sites.len()
    }
    pub(crate) fn validate(&self, dm: DepManifest) {
        for p in self.package_to_sites.keys() {
            dm.validate(p);
        }
    }
    //--------------------------------------------------------------------------
    // draft implementations
    pub(crate) fn report(&self) {
        let mut packages: Vec<Package> = self.package_to_sites.keys().cloned().collect();
        packages.sort();

        let mut site_packages: HashSet<&PathBuf> = HashSet::new();

        for package in packages {
            println!("{:?}", package);
            if let Some(site_paths) = self.package_to_sites.get(&package) {
                for path in site_paths {
                    site_packages.insert(path);
                    println!("    {:?}", path);
                }
            }
        }
        println!("exes: {:?}", self.exe_to_sites.len());
        println!("site packages: {:?}", site_packages.len());
        println!("packages: {:?}", self.package_to_sites.len());
    }
}



//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {

    use super::*;
    use tempfile::tempdir;
    use std::fs::File;
    use std::os::unix::fs::symlink;

    #[test]
    fn test_get_exclude_path_a() {
        let post = get_exclude_path();
        assert_eq!(post.len() > 2, true);
    }

    #[test]
    fn test_get_exe_origins_a() {
        let post = get_exe_origins();
        assert_eq!(post.len() > 6, true);
    }

    #[test]
    fn test_is_exe_a() {
        let temp_dir = tempdir().unwrap();
        let fp = temp_dir.path().join("test.sh");
        let _ = File::create(fp.clone()).unwrap();
        let mut perms = fs::metadata(fp.clone()).unwrap().permissions();
        perms.set_mode(0o755); // rwxr-xr-x (755) for an executable script
        fs::set_permissions(fp.clone(), perms).unwrap();
        assert_eq!(is_exe(&fp), false);
    }

    #[test]
    fn test_is_exe_b() {
        let temp_dir = tempdir().unwrap();
        let fp = temp_dir.path().join("python");
        let _ = File::create(fp.clone()).unwrap();
        let mut perms = fs::metadata(fp.clone()).unwrap().permissions();
        perms.set_mode(0o755); // rwxr-xr-x (755) for an executable script
        fs::set_permissions(fp.clone(), perms).unwrap();
        assert_eq!(is_exe(&fp), true);
    }

    #[test]
    fn test_is_exe_c() {
        let temp_dir = tempdir().unwrap();
        let fp = temp_dir.path().join("python10.100");
        let _ = File::create(fp.clone()).unwrap();
        let mut perms = fs::metadata(fp.clone()).unwrap().permissions();
        perms.set_mode(0o755); // rwxr-xr-x (755) for an executable script
        fs::set_permissions(fp.clone(), perms).unwrap();
        assert_eq!(is_exe(&fp), true);
    }

    #[test]
    fn test_is_symlink_a() {
        let temp_dir = tempdir().unwrap();
        let fp1 = temp_dir.path().join("test.txt");
        let _ = File::create(fp1.clone()).unwrap();
        let fp2 = temp_dir.path().join("link.txt");
        let _ = symlink(fp1.clone(), fp2.clone());
        assert_eq!(is_symlink(&fp1), false);
        assert_eq!(is_symlink(&fp2), true);
    }

    #[test]
    fn test_get_site_package_dirs_a() {
        let p1 = Path::new("python3");
        let paths = get_site_package_dirs(p1);
        assert_eq!(paths.len() > 0, true)
    }

    #[test]
    fn test_scan_executable_inner_a() {
        let temp_dir = tempdir().unwrap();
        let fpd1 = temp_dir.path();
        let fpf1 = fpd1.join("pyvenv.cfg");
        let _ = File::create(fpf1).unwrap();

        let fpd2 = fpd1.join("bin");
        fs::create_dir(fpd2.clone()).unwrap();

        let fpf2 = fpd2.join("python3");
        let _ = File::create(fpf2.clone()).unwrap();
        let mut perms = fs::metadata(fpf2.clone()).unwrap().permissions();
        perms.set_mode(0o755); // rwxr-xr-x (755) for an executable script
        fs::set_permissions(fpf2.clone(), perms).unwrap();


        let exclude_paths = HashSet::with_capacity(0);
        let mut result = scan_executables_inner(fpd1, &exclude_paths, true);
        assert_eq!(result.len(), 1);

        let fp_found: PathBuf = result.pop().unwrap();
        let pcv = fp_found.into_iter().rev().take(2).collect::<Vec<_>>();
        let pcp = pcv.iter().rev().collect::<PathBuf>();
        assert_eq!(pcp, PathBuf::from("bin/python3"));
    }

    #[test]
    fn test_from_exe_to_sites_a() {
        let temp_dir = tempdir().unwrap();
        let fp_dir = temp_dir.path();
        let fp_exe = fp_dir.join("python");
        let _ = File::create(fp_exe.clone()).unwrap();

        let fp_sp = fp_dir.join("site-packages");
        fs::create_dir(fp_sp.clone()).unwrap();

        let fp_p1 = fp_sp.join("numpy-1.19.1.dist-info");
        let _ = File::create(fp_p1.clone()).unwrap();
        let fp_p2 = fp_sp.join("foo-3.0.dist-info");
        let _ = File::create(fp_p2.clone()).unwrap();

        let mut exe_to_sites = HashMap::<PathBuf, Vec<PathBuf>>::new();
        exe_to_sites.insert(fp_exe.clone(), vec![fp_sp]);
        let sfs = ScanFS::from_exe_to_sites(exe_to_sites).unwrap();
        assert_eq!(sfs.len(), 2)
        // sfs.report();
    }

}




