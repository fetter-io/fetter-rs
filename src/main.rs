mod dep_manifest;
mod dep_spec;
mod exe_search;
mod package;
mod scan_fs;
mod version_spec;
use crate::scan_fs::ScanFS;

// NEXT:
// DepSpec has a from_package
// ScanFS, given an fp, writes out a requirements-bound file (not a lock file)
// Implement command line entry point that takes a requirements bound file and validates
// Implement a colorful display
// Implement a monitoring mode


fn main() {
    let sfs = ScanFS::from_defaults().unwrap();
    sfs.report();
}
