// use std::path::Path;


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




#[cfg(test)]
mod tests {

    use super::*;
    use tempfile::tempdir;
    use std::fs::create_dir;
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
        create_dir(fpd2.clone()).unwrap();

        let fpf2 = fpd2.join("file2.txt");
        let mut file2 = File::create(fpf2).unwrap();
        writeln!(file2, "test content").unwrap();

    }


}
