

mod scan_fs;
mod package;
mod dep_spec;
mod dep_manifest;
mod version_spec;
mod exe_search;
use crate::scan_fs::ScanFS;

fn main() {
    let sfs = ScanFS::from_defaults().unwrap();
    sfs.report();
}
