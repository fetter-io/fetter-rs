mod dep_manifest;
mod dep_spec;
mod exe_search;
mod package;
mod scan_fs;
mod version_spec;
use crate::scan_fs::ScanFS;

// NEXT:
// Need to implment to_file() on DepManifest
// Implement command line entry points
// write a requurements bound file based on ScanFSs
// takes a requirements bound file and validates
// Implement a colorful display
// Implement a monitoring mode

fn main() {
    let sfs = ScanFS::from_defaults().unwrap();
    sfs.report();
}
