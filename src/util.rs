use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::fs;
use std::process::Command;
use std::io::Result;
use std::env;
use std::os::unix::fs::PermissionsExt;
use rayon::prelude::*;

// Provide absolute paths for directories that should be excluded from search.
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

// Provide directories that should be used as origins for searching for executables.
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
    paths

}


fn is_exe(path: &Path) -> bool {
    let file_name = path.file_name().unwrap().to_str().unwrap();
    if file_name.starts_with("python") {
        let suffix = &file_name[6..];
        if suffix.is_empty() || suffix.chars().all(|c| c.is_digit(10) || c == '.') {
            match fs::metadata(path) {
                Ok(md) => {
                    return md.permissions().mode() & 0o111 != 0;
                }
                Err(_e) => {
                    return false;
                }
            }

        }
    }
    false
}

fn is_symlink(path: &Path) -> bool {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        metadata.file_type().is_symlink()
    } else {
        false
    }
}


/// Try to find all Python executables given a starting directory. This will recursively search all directories that are not symlinks.
fn get_executables_inner(
        path: &Path,
        exclude_paths: &HashSet<PathBuf>,
        recurse: bool,
        ) -> Vec<PathBuf> {
    if exclude_paths.contains(path) {
        return Vec::with_capacity(0);
    }
    let mut paths = Vec::new();
    if path.is_dir() {
        // need to skip paths that are always bad, like
        // if we find "fpdir/pyvenv.cfg", we can always get fpdir/bin/python3
        let path_cfg = path.to_path_buf().join("pyvenv.cfg");
        if path_cfg.exists() {
            let path_exe = path.to_path_buf().join("bin").join("python3");
            if path_exe.exists() {
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
                            paths.extend(get_executables_inner(&path, exclude_paths, recurse));
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

// Main entry point with platform dependent branching
fn get_executables() -> Result<Vec<PathBuf>> {
    let exclude = get_exclude_path();
    let origins = get_exe_origins();

    println!("origins: {:?}", origins);

    // let mut paths = Vec::new();
    // for path in origins {
    //     println!("searchin dir {:?}", path);
    //     paths.extend(get_executables_inner(&path, &exclude));
    // }

    let paths: Vec<PathBuf> = origins
            .par_iter()
            .flat_map(|(path, recurse)| get_executables_inner(path, &exclude, *recurse))
            .collect();

    Ok(paths)
}


/// Given a path to a Python binary, call out to Python to get all known site packages; some site packages may not exist; we do not filter them here. This will include "dist-packages" on Linux.
fn get_site_packages(executable: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new(executable.to_str().unwrap())
            .arg("-c")
            .arg("import site;print(\"\\n\".join(site.getsitepackages()));print(site.getusersitepackages())") // since Python 3.2
            .output();

    if let Err(e) = output {
        eprintln!("Failed to execute command: {}", e); // log this
        return Ok(Vec::new());
    }

    let out_raw = output.unwrap().stdout;
    let paths_lines = std::str::from_utf8(&out_raw)
            .expect("Failed to convert to UTF-8") // will panic
            .trim();

    let paths: Vec<PathBuf> = paths_lines
            .lines()
            .map(|line| PathBuf::from(line.trim()))
            .collect();

    println!("{:?}", paths);
    return Ok(paths);
}


fn files_eager(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() { // recurse
                files.extend(files_eager(&path)?);
            } else { // Collect file names
                files.push(path);
            }
        }
    }
    Ok(files)
}



#[cfg(test)]
mod tests {

    use super::*;
    use tempfile::tempdir;
    use std::fs::File;
    use std::io::Write;
    // use std::str::FromStr;

    #[test]
    fn test_get_site_packages_a() {
        let p1 = Path::new("python3");
        let _paths = get_site_packages(p1);

        let p2 = Path::new("/usr/bin/python3");
        let _paths = get_site_packages(p2);

    }

    #[test]
    fn test_get_executables_a() {
        // let p1 = Path::new("/usr/local");
        let _paths = get_executables();
        println!("{:?}", _paths);

        // let p1 = Path::new("/usr/bin");
        // let _paths = get_executables(p1);
        // println!("{:?}", _paths);

        // let p2 = Path::new("/usr/bin/python3");
        // let _paths = get_site_packages(p2);

    }

    #[test]
    fn test_search_dir_a() {
        let temp_dir = tempdir().unwrap();
        let fpd1 = temp_dir.path();
        let fpf1 = fpd1.join("file1.txt");
        let mut file1 = File::create(fpf1).unwrap();
        writeln!(file1, "test content 1").unwrap();

        let fpd2 = fpd1.join("dir_sub");
        fs::create_dir(fpd2.clone()).unwrap();

        let fpf2 = fpd2.join("file2.txt");
        let mut file2 = File::create(fpf2).unwrap();
        writeln!(file2, "test content").unwrap();

        let result = files_eager(fpd1).unwrap();
        assert_eq!(result.len(), 2);
        // assert_eq!(result[0].ends_with("file1.txt"), true);
        // assert_eq!(result[1].ends_with("file2.txt"), true);
    }


}




