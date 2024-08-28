use std::path::{Path, PathBuf};
use std::fs;
use std::process::Command;
use std::io::Result;


/// Try to find all Python executables given a starting directory. This will recursively search all directories.
fn get_executables(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if path.is_dir() {
        // NOTE: might be able to pre-form expected Path and see if it exists to avoid iterating over all names
        // NOTE: might be able to pre-detect a vitual env and avoid recursion
        // if we find "fpdir/pyvenv.cfg", we can always get fpdir/bin/python3
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            let file_name = path.file_name().unwrap().to_str().unwrap();

            if path.is_dir() { // recurse
                files.extend(get_executables(&path)?);
            } else if file_name.starts_with("python") {
                files.push(path);
            }
        }
    }
    Ok(files)
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
