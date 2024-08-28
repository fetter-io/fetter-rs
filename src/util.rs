use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::fs;
use std::process::Command;
use std::io::Result;
use std::env;

use rayon::prelude::*;

fn path_exclude -> HashSet<PathBuf> {
    let home = env::var("HOME").unwrap_or_else(|_| String::from("/"));
    let mut paths: HashSet<PathBuf> = HashSet::new();
    // common to all
    paths.insert(PathBuf::from(home.clone()).join(".cache"));
    paths.insert(PathBuf::from(home.clone()).join(".npm"));

    if env::consts::OS == "macos" {
        paths.insert(PathBuf::from(home.clone()).join("Library"));
        paths.insert(PathBuf::from(home.clone()).join("Photos"));
        paths.insert(PathBuf::from(home.clone()).join("Downloads"));
        paths.insert(PathBuf::from(home.clone()).join(".Trash"));
    }
    // } else if env::consts::OS == "linux" {
    //     exclude = path_exclude_linux();
    // }
    paths
}


fn exe_origins() -> Vec<PathBuf> {
    let home = env::var("HOME").unwrap_or_else(|_| String::from("/"));
    let mut paths: Vec<PathBuf> = Vec::new();
    paths.push(PathBuf::from(home.clone()));
    paths.push(PathBuf::from("/bin"));
    paths.push(PathBuf::from("/sbin"));
    paths.push(PathBuf::from("/usr/bin"));
    paths.push(PathBuf::from("/usr/sbin"));
    paths.push(PathBuf::from("/usr/local/bin"));
    paths.push(PathBuf::from("/usr/local/sbin"));

    // if env::consts::OS == "macos" {
    //     exclude = path_exclude_mac();
    // } else if env::consts::OS == "linux" {
    //     exclude = path_exclude_linux();
    // }

    paths

}


fn is_exe_file_name(file_name: &str) -> bool {
    if file_name.starts_with("python") {
        let suffix = &file_name[6..];
        return suffix.is_empty() || suffix.chars().all(|c| c.is_digit(10) || c == '.');
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
        exclude: &HashSet<PathBuf>,
        ) -> Vec<PathBuf> {
    if exclude.contains(path) {
        return Vec::with_capacity(0);
    }
    let mut paths = Vec::new();
    if path.is_dir() {
        // need to skip paths that are always bad, like
        // if we find "fpdir/pyvenv.cfg", we can always get fpdir/bin/python3
        let path_cfg = path.to_path_buf().join("pyvenv.cfg");
        if path_cfg.exists() {
            let path_exe = path.to_path_buf().join("bin").join("python3");
            // println!("path_exe: {:?}", path_exe);
            if path_exe.exists() {
                paths.push(path_exe)
            }
        }
        else {
            // println!("trying read_dir: {:?}", path);
            match fs::read_dir(path) {
                Ok(entries) => {
                    for entry in entries {
                        let entry = entry.unwrap();
                        let path = entry.path();
                        let file_name = path.file_name().unwrap().to_str().unwrap();

                        if path.is_dir() && !is_symlink(&path) { // recurse
                            paths.extend(get_executables_inner(&path, exclude));
                        } else if is_exe_file_name(&file_name) {
                            // TODO: can we check if it is executable?
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
    let exclude = path_exclude();
    let origins = exe_origins();

    // let mut paths = Vec::new();
    // for path in origins {
    //     println!("searchin dir {:?}", path);
    //     paths.extend(get_executables_inner(&path, &exclude));
    // }

    // let paths: Result<Vec<PathBuf>, io::Error> = origins
    //         .par_iter()
    //         .flat_map(|path| {
    //             // Convert the inner Vec<PathBuf> into an iterator of Result<PathBuf, io::Error>
    //             match get_executables_inner(path, &exclude) {
    //                 Ok(inner_paths) => inner_paths.into_iter().map(Ok).collect::<Vec<_>>(),
    //                 Err(e) => vec![Err(e)],  // Convert the error into an iterator
    //             }
    //         })
    //         .collect();  // Collect into Result<Vec<PathBuf>, io::Error>

    let paths: Vec<PathBuf> = origins
            .par_iter()
            .flat_map(|path| get_executables_inner(path, &exclude))  // No need to handle Result
            .collect();  // Collect directly into a Vec<PathBuf>

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




