

mod scan_fs;
mod package;
mod dep_spec;
mod dep_manifest;
mod version_spec;


fn main() {
    scan_fs::scan();
    // println!("{:?}", post);
}
