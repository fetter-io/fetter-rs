use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::io::BufRead;
use std::marker::Send;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
// use std::time::Instant;

use rayon::prelude::*;

use crate::package::Package;
use crate::path_shared::PathShared;
use crate::table::HeaderFormat;
use crate::table::Rowable;
use crate::table::RowableContext;
use crate::table::Tableable;

//------------------------------------------------------------------------------
/// This contains the explicit files found in a RECORD file, as well as all discovered directories that contain one or more of those file.
#[derive(Debug, Clone)]
struct Artifacts {
    files: Vec<(PathBuf, bool)>,
    dirs: HashSet<PathBuf>,
}

impl Artifacts {

    fn from_package(package: &Package, site: &PathShared) -> io::Result<Self> {
        let dir_dist_info = package.to_dist_info_dir(site);
        let dir_src = package.to_src_dir(site);
        // parent of dist-info dir is site packages; all RECORD paths are relative to this
        let dir_site = dir_dist_info.parent().unwrap();
        let fp_record = dir_dist_info.join("RECORD");

        let mut dirs_observed = HashSet::new();
        let mut files = Vec::new();

        let file = fs::File::open(fp_record)?;
        let reader = io::BufReader::new(file);
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Some(fp_rel) = line.split(',').next() {
                let fp = dir_site.join(fp_rel);
                let exists = fp.exists();
                files.push((fp.to_path_buf(), exists));
                // Only store directories if the file exists; we will only delete them if they are empty after removals
                if exists {
                    if let Some(dir) = fp.parent() {
                        dirs_observed.insert(dir.to_path_buf());
                    }
                }
            }
        }
        // this can be a Vec
        let mut dirs = HashSet::new();
        if dirs_observed.contains(&dir_dist_info) {
            dirs.insert(dir_dist_info.clone());
        }
        if dirs_observed.contains(&dir_src) {
            dirs.insert(dir_src.clone());
        }
        Ok(Artifacts { files, dirs })
    }


    fn remove(&self, log: bool) -> io::Result<()> {
        for (fp, exists) in &self.files {
            if *exists {
                if log {
                    eprintln!("removing file: {:?}", fp);
                }
                fs::remove_file(&fp)?;
            }
        }
        // as file system might be delayed in recognizing deletions, we try to sleep, but this is not entirely effective
        thread::sleep(Duration::from_millis(4000));
        for dir in &self.dirs {
            if fs::read_dir(dir)?.next().is_none() {
                if log {
                    eprintln!("removing dir: {:?}", dir);
                }
                fs::remove_dir(&dir)?;
            }
        }
        Ok(())
    }
}

// for dir in &self.dirs {
//     let start = Instant::now();
//     let mut delay = Duration::from_millis(50);
//     let max_wait = Duration::from_secs(5);

//     while start.elapsed() < max_wait {
//         if fs::read_dir(dir)?.next().is_none() {
//             if log {
//                 eprintln!("removing dir: {:?}", dir);
//             }
//             fs::remove_dir(&dir)?;
//             break;
//         }
//         thread::sleep(delay);
//         delay = delay.saturating_mul(2);
//     }
// }

// we cannot evaluate this until after we remove the files
// let dirs = dir_candidates
//     .iter()
//     .filter_map(|dir| {
//         if fs::read_dir(&dir).ok()?.next().is_none() {
//             Some(dir.clone()) // keep if empty
//         } else {
//             None
//         }
//     })
//     .collect();
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

//------------------------------------------------------------------------------
trait UnpackRecordTrait {
    /// Return a new record; caller must clone as needed.
    fn new(package: Package, site: PathShared, artifacts: Artifacts) -> Self;
}

//------------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub(crate) struct UnpackFullRecord {
    package: Package,
    site: PathShared,
    artifacts: Artifacts,
}

impl UnpackRecordTrait for UnpackFullRecord {
    fn new(package: Package, site: PathShared, artifacts: Artifacts) -> Self {
        UnpackFullRecord {
            package,
            site,
            artifacts,
        }
    }
}

impl Rowable for UnpackFullRecord {
    fn to_rows(&self, context: &RowableContext) -> Vec<Vec<String>> {
        let is_tty = *context == RowableContext::TTY;

        let mut package_set = false;
        let mut package_display = || {
            if !is_tty || !package_set {
                package_set = true;
                self.package.to_string()
            } else {
                "".to_string()
            }
        };

        let mut site_set = false;
        let mut site_display = || {
            if !is_tty || !site_set {
                site_set = true;
                self.site.display().to_string()
            } else {
                "".to_string()
            }
        };

        let mut rows: Vec<Vec<String>> = Vec::new();
        for (fp, exists) in &self.artifacts.files {
            rows.push(vec![
                package_display(),
                site_display(),
                exists.to_string(),
                fp.display().to_string(),
            ]);
        }
        rows
    }
}
//------------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub(crate) struct UnpackCountRecord {
    package: Package,
    site: PathShared,
    artifacts: Artifacts,
}

impl UnpackRecordTrait for UnpackCountRecord {
    fn new(package: Package, site: PathShared, artifacts: Artifacts) -> Self {
        UnpackCountRecord {
            package,
            site,
            artifacts,
        }
    }
}

impl Rowable for UnpackCountRecord {
    fn to_rows(&self, _context: &RowableContext) -> Vec<Vec<String>> {
        vec![vec![
            self.package.to_string(),
            self.site.display().to_string(),
            self.artifacts.files.len().to_string(),
            self.artifacts.dirs.len().to_string(),
        ]]
    }
}

//------------------------------------------------------------------------------
/// Generic function to covert a `HashMap` to a `Vec` of of UnpackRecords.
fn package_to_sites_to_records<R>(
    package_to_sites: &HashMap<Package, Vec<PathShared>>,
) -> Vec<R>
where
    R: UnpackRecordTrait + Sync + Send,
{
    package_to_sites
        .par_iter()
        .flat_map(|(package, sites)| {
            sites.par_iter().filter_map(move |site| {
                if let Ok(artifacts) = Artifacts::from_package(&package, &site) {
                    Some(R::new(package.clone(), site.clone(), artifacts))
                } else {
                    eprintln!("Failed to read artifacts: {:?}", package);
                    None
                }
            })
        })
        .collect()
}

//------------------------------------------------------------------------------
pub(crate) struct UnpackFullReport {
    records: Vec<UnpackFullRecord>,
}

impl Tableable<UnpackFullRecord> for UnpackFullReport {
    fn get_header(&self) -> Vec<HeaderFormat> {
        vec![
            HeaderFormat::new("Package".to_string(), false, None),
            HeaderFormat::new("Site".to_string(), true, None),
            HeaderFormat::new("Exists".to_string(), false, None),
            HeaderFormat::new("Artifacts".to_string(), true, None),
        ]
    }
    fn get_records(&self) -> &Vec<UnpackFullRecord> {
        &self.records
    }
}

//------------------------------------------------------------------------------
pub(crate) struct UnpackCountReport {
    records: Vec<UnpackCountRecord>,
}

impl Tableable<UnpackCountRecord> for UnpackCountReport {
    fn get_header(&self) -> Vec<HeaderFormat> {
        vec![
            HeaderFormat::new("Package".to_string(), false, None),
            HeaderFormat::new("Site".to_string(), true, None),
            HeaderFormat::new("Files".to_string(), false, None),
            HeaderFormat::new("Dirs".to_string(), false, None),
        ]
    }
    fn get_records(&self) -> &Vec<UnpackCountRecord> {
        &self.records
    }
}

//------------------------------------------------------------------------------
pub(crate) enum UnpackReport {
    Full(UnpackFullReport),
    Count(UnpackCountReport),
}
impl UnpackReport {
    pub(crate) fn from_package_to_sites(
        count: bool,
        package_to_sites: &HashMap<Package, Vec<PathShared>>,
    ) -> Self {
        if count {
            let records = package_to_sites_to_records(package_to_sites);
            UnpackReport::Count(UnpackCountReport { records })
        } else {
            let records = package_to_sites_to_records(package_to_sites);
            UnpackReport::Full(UnpackFullReport { records })
        }
    }

    pub(crate) fn to_stdout(&self) -> io::Result<()> {
        match self {
            UnpackReport::Full(report) => report.to_stdout(),
            UnpackReport::Count(report) => report.to_stdout(),
        }
    }

    pub(crate) fn to_file(&self, file_path: &PathBuf, delimiter: char) -> io::Result<()> {
        match self {
            UnpackReport::Full(report) => report.to_file(file_path, delimiter),
            UnpackReport::Count(report) => report.to_file(file_path, delimiter),
        }
    }

    pub(crate) fn remove(&self, log: bool) -> io::Result<()> {
        match self {
            UnpackReport::Full(report) => {
                report.records.par_iter().for_each(|record| {
                    let _ = record.artifacts.remove(log);
                });
            }
            UnpackReport::Count(report) => {
                report.records.par_iter().for_each(|record| {
                    let _ = record.artifacts.remove(log);
                });
            }
        }
        Ok(())
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_record_a() {
        let pkg = Package::from_dist_info("xarray-0.21.1.dist-info", None).unwrap();
        let dir_temp = tempdir().unwrap(); // this is our site
        let site = PathShared::from_path_buf(dir_temp.path().to_path_buf());
        let dir_dist_info = dir_temp.path().join("xarray-0.21.1.dist-info");
        fs::create_dir(&dir_dist_info).unwrap();
        let fp_record = dir_dist_info.as_path().join("RECORD");

        let content = r#"
xarray-0.21.1.dist-info/INSTALLER,sha256=zuuue4knoyJ-UwPPXg8fezS7VCrXJQrAP7zeNuwvFQg,4
xarray-0.21.1.dist-info/LICENSE,sha256=c7p036pSC0mkAbXSFFmoUjoUbzt1GKgz7qXvqFEwv2g,10273
xarray-0.21.1.dist-info/METADATA,sha256=T6ewGJSP7S1OFMxt7eEcm-pKKjzyq0rx5pEGlFbe0ms,6008
xarray-0.21.1.dist-info/RECORD,,
xarray-0.21.1.dist-info/REQUESTED,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0
xarray-0.21.1.dist-info/WHEEL,sha256=G16H4A3IeoQmnOrYV4ueZGKSjhipXx8zc8nu9FGlvMA,92
xarray-0.21.1.dist-info/top_level.txt,sha256=OGV8AqTgYtuaw6YV6tevWXEdDI5vHJiARQCJgRyT7co,7
xarray/__init__.py,sha256=Kn7MQ1eaUQZVe5dyc8aYoVpr4iMaao5oEKWyA8TK_oQ,2826
xarray/__pycache__/__init__.cpython-311.pyc,,
xarray/__pycache__/tutorial.cpython-311.pyc,,
xarray/__pycache__/ufuncs.cpython-311.pyc,,
xarray/backends/__init__.py,sha256=SOkeUBf7KCR7ji-QYJranGhhto0sGfV3QHHeU_hSVoA,1100
xarray/backends/__pycache__/__init__.cpython-311.pyc,,
xarray/backends/__pycache__/store.cpython-311.pyc,,
xarray/backends/__pycache__/zarr.cpython-311.pyc,,
xarray/backends/api.py,sha256=NZsX3TXz_pUHKu9S50twpKlIri8AksMlpki2OaD_0jQ,54268
xarray/backends/rasterio_.py,sha256=S8cm7Zvz95rC9Ee3u7384x_E6GeBk6nVhnXKgmlcTTA,15482
xarray/backends/zarr.py,sha256=Ect1jrS0X4qlZlKCQ5CyrLxOWFzqh7i0vuUhzQ3-y_U,30946
xarray/coding/__init__.py,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0
xarray/coding/__pycache__/__init__.cpython-311.pyc,,
xarray/coding/__pycache__/variables.cpython-311.pyc,,
xarray/coding/calendar_ops.py,sha256=6Bt47kyLTjnxnV0wYnG7_glLtBWnyNU54ksuPWgICc8,13703
xarray/coding/cftime_offsets.py,sha256=WZQtWiCG34s7sR6OBo9ffqO6aZchpY-QofvmJwFNZ_c,42730
xarray/convert.py,sha256=E2Rocp9OeVll4le8WtqQWnlGVAZ7hhmXqtnDnL1G1Vk,9643
xarray/core/__init__.py,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0
xarray/core/__pycache__/__init__.cpython-311.pyc,,
xarray/core/__pycache__/_reductions.cpython-311.pyc,,
xarray/core/__pycache__/weighted.cpython-311.pyc,,
xarray/core/_reductions.py,sha256=ivDih2-exiXoLb5aXtvWbz5JDiiQxfIRKmjNHdAJ9w0,133749
xarray/core/_typed_ops.py,sha256=9Sw7vc3cLotwINQXSMZC5Or9R1dPExeslAQxxTODZKU,26243
xarray/core/_typed_ops.pyi,sha256=mPs9rWZaBwNfRG6dKbamkw_ryRPW9w_iIxB1no-TZds,31184
xarray/core/accessor_dt.py,sha256=0EoMBBMoGmje_efIez3BTq5WEvgYz5MtPt5GCgaqPus,18578
xarray/core/weighted.py,sha256=CQtBTXlLMCsiW_c6hsKSyduM1FwBS0LRe4gwTeL2zOw,11686
xarray/plot/__init__.py,sha256=mMMC5ySGsutPmnWQ5vbx9AKX0anr6z6Y2avjmNvo2Ro,329
xarray/plot/__pycache__/__init__.cpython-311.pyc,,
xarray/plot/__pycache__/dataset_plot.cpython-311.pyc,,
xarray/plot/__pycache__/utils.cpython-311.pyc,,
xarray/plot/dataset_plot.py,sha256=3RGm3LnFK62DDKAjt6RiNbhrxHAId-qpNBIYx_tJb_Y,20829
xarray/py.typed,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0
xarray/static/__init__.py,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0
xarray/static/__pycache__/__init__.cpython-311.pyc,,
xarray/static/css/__init__.py,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0
xarray/static/css/__pycache__/__init__.cpython-311.pyc,,43
xarray/testing.py,sha256=-WotNkBU9ch6AR9YK64ieaKtMkTBJNzzvUt_9xyhs5o,12412
xarray/tests/__init__.py,sha256=oUn-mBul4siTKundcFupxY6YuEHObLN8t8g11Rcq1TE,7077
xarray/tests/__pycache__/__init__.cpython-311.pyc,,
xarray/tests/__pycache__/conftest.cpython-311.pyc,,
xarray/tests/__pycache__/test_accessor_dt.cpython-311.pyc,,
xarray/tests/__pycache__/test_weighted.cpython-311.pyc,,
xarray/tests/conftest.py,sha256=E_8llREz2LY96ts6N2Pwoq08nQ96pC8sM3qfYJR5n24,169
xarray/tests/data/bears.nc,sha256=912tQ5fHIS-VDTBe3UplQi2rdRcreMSQ0tIdCOg9FRI,1184
xarray/tests/test_weighted.py,sha256=ryJPu4CD3sodDGN238HKUYzkcqCsgSl9jsYURTXBWZA,16265
xarray/tutorial.py,sha256=Aoau1-Tm1tCfxCWORaepseRwWZRL1CUooBLCbqUsxDc,7040
xarray/ufuncs.py,sha256=Rlykkrkk5ZAN-H6B9ZKYMP2uvbiwGmjEPfoY51dln0E,4562
xarray/util/__init__.py,sha256=47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU,0
xarray/util/__pycache__/__init__.cpython-311.pyc,,
xarray/util/__pycache__/generate_ops.cpython-311.pyc,,
xarray/util/generate_ops.py,sha256=amEIE7w5momaWtwbl1wZY9jfVe2liYkweOHCRm8dxMs,9227
xarray/util/print_versions.py,sha256=kSqlh0crnpEzanhYmV3F7RuGEys8nrOhM_Yf_i7D7bM,5145
        "#;
        let mut file = File::create(&fp_record).unwrap();
        write!(file, "{}", content).unwrap();
        let rc = Artifacts::from_package(&pkg, &site).unwrap();
        // println!("{:?}", rc);
        assert_eq!(rc.files.len(), 59);
        assert_eq!(rc.dirs.len(), 1);
    }
}
