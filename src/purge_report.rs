use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

fn record_to_files(record_file: &PathBuf) -> io::Result<()> {
    let record_dir = record_file.parent().unwrap_or_else(|| Path::new(""));

    let mut dirs = HashSet::new();

    let file = fs::File::open(record_file)?;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let line = line?;
        if let Some(fp_rel) = line.split(',').next() {
            let fp = record_dir.join(fp_rel);
            if fp.exists() {
                // fs::remove_file(&fp)?;
                println!("Exists: {:?}", fp);
                if let Some(dir) = fp.parent() {
                    dirs.insert(dir.to_path_buf());
                }
            } else {
                println!("File not found: {:?}", fp);
            }
        }
    }

    // Attempt to remove any empty directories
    // for dir in dirs {
    //     match fs::remove_dir(&dir) {
    //         Ok(_) => println!("Removed empty directory: {:?}", dir),
    //         Err(e) => {
    //             // Directory is not empty or some other error occurred
    //             if e.kind() == io::ErrorKind::NotEmpty {
    //                 println!("Directory not empty, skipping: {:?}", dir);
    //             } else {
    //                 println!("Error removing directory {:?}: {}", dir, e);
    //             }
    //         }
    //     }
    // }

    Ok(())
}
