use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

struct RecordTargets {
    files: Vec<(PathBuf, bool)>,
    dirs: HashSet<PathBuf>,
}

fn record_to_files(record_fp: &PathBuf) -> io::Result<RecordTargets> {
    let record_dir = record_fp.parent().unwrap_or_else(|| Path::new(""));

    let mut dirs = HashSet::new();
    let mut files: Vec<(PathBuf, bool)> = Vec::new();

    let file = fs::File::open(record_fp)?;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let line = line?;
        if let Some(fp_rel) = line.split(',').next() {
            let fp = record_dir.join(fp_rel);
            let exists = fp.exists();
            files.push((fp.to_path_buf(), exists));
            if exists {
                if let Some(dir) = fp.parent() {
                    dirs.insert(dir.to_path_buf());
                }
            }
        }
    }
    Ok(RecordTargets { files, dirs })
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
