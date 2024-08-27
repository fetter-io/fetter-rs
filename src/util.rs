use std::path::Path;
use std::fs;

pub(crate) fn gcd(mut n: i64, mut m: i64) -> Result<i64, &'static str>
{
    if n <= 0 || m <= 0 {
        return Err("zero or negative values not supported");
    }
    while m != i64::from(0) {
        if m < n {
            std::mem::swap(&mut m, &mut n);
        }
        m = m % n;
    }
    Ok(n)
}


fn files_eager(path: &Path) -> std::io::Result<Vec<String>> {
    let mut files = Vec::new();
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() { // recurse
                files.extend(files_eager(&path)?);
            } else { // Collect file names
                files.push(path.to_str().unwrap().to_string());
                // if let Some(file_name) = path.file_name() {
                //     files.push(file_name.to_string_lossy().into_owned());
                // }
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

    #[test]
    fn test_gcd_a() {
        assert_eq!(gcd(14, 15).unwrap(), 1);
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
        assert_eq!(result[0].ends_with("file1.txt"), true);
        assert_eq!(result[1].ends_with("file2.txt"), true);
    }


}
