use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::fs;
use std::process::Command;
use std::io::Result;
use std::env;


fn path_exclude_mac() -> HashSet<PathBuf> {
    let home = env::var("HOME").unwrap_or_else(|_| String::from("/"));
    let mut paths: HashSet<PathBuf> = HashSet::new();
    paths.insert(PathBuf::from(home.clone()).join("Library"));
    paths.insert(PathBuf::from(home.clone()).join("Photos"));
    paths.insert(PathBuf::from(home.clone()).join("Downloads"));
    paths.insert(PathBuf::from(home.clone()).join(".Trash"));
    paths.insert(PathBuf::from(home.clone()).join(".cache"));
    paths
}

fn path_exclude_linux() -> HashSet<PathBuf> {
    let home = env::var("HOME").unwrap_or_else(|_| String::from("/"));
    let mut paths: HashSet<PathBuf> = HashSet::new();
    paths.insert(PathBuf::from(home.clone()).join(".cache"));
    paths
}


/// Try to find all Python executables given a starting directory. This will recursively search all directories.
fn get_executables_inner(
        path: &Path,
        exclude: &HashSet<PathBuf>,
        ) -> Result<Vec<PathBuf>> {
    if exclude.contains(path) {
        return Ok(Vec::with_capacity(0));
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
                        let entry = entry?;
                        let path = entry.path();
                        let file_name = path.file_name().unwrap().to_str().unwrap();

                        if path.is_dir() { // recurse
                            paths.extend(get_executables_inner(&path, exclude)?);
                        } else if file_name == "python" {
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
    Ok(paths)
}

// Main entry point with platform dependent branching
fn get_executables(
    path: &Path,
    ) -> Result<Vec<PathBuf>> {

    let exclude;
    if cfg!(target_os = "macos") {
        exclude = path_exclude_mac();
    } else if cfg!(target_os = "linux") {
        exclude = path_exclude_linux();
    } else {
        exclude = HashSet::with_capacity(0);
    }
    get_executables_inner(path, &exclude)
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
        let p1 = Path::new("/Users/ariza");
        let _paths = get_executables(p1);
        println!("{:?}", _paths);

        let p1 = Path::new("/usr/bin");
        let _paths = get_executables(p1);
        println!("{:?}", _paths);

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
