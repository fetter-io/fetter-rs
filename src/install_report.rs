use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;

use rayon::prelude::*;

use crate::package::Package;
use crate::path_shared::PathShared;

//------------------------------------------------------------------------------
/// This contains the explicit files found in a RECORD file, as well as all discovered directories that contain one or more of those file.
#[derive(Debug, Clone)]
struct Artifacts {
    files: Vec<(PathBuf, bool)>,
    dirs: HashSet<PathBuf>,
}

fn dist_info_to_artifacts(dist_info_fp: &PathBuf) -> io::Result<Artifacts> {
    // parent of RECORD is the dist-info dir, and must exist
    let dir_site = dist_info_fp.parent().unwrap(); //.parent().unwrap();
    let fp_record = dist_info_fp.join("RECORD");

    let mut dirs = HashSet::new();
    let mut files = Vec::new();

    let file = fs::File::open(fp_record)?;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(fp_rel) = line.split(',').next() {
            let fp = dir_site.join(fp_rel);
            let exists = fp.exists();
            files.push((fp.to_path_buf(), exists));
            if exists {
                if let Some(dir) = fp.parent() {
                    dirs.insert(dir.to_path_buf());
                }
            }
        }
    }
    Ok(Artifacts { files, dirs })
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

//------------------------------------------------------------------------------
#[derive(Debug, Clone)]
struct InstallRecord {
    package: Package,
    site: PathShared,
    artifacts: Artifacts,
}

pub(crate) struct InstallReport {
    records: Vec<InstallRecord>,
}

impl InstallReport {
    pub(crate) fn from_package_to_sites(
        package_to_sites: &HashMap<Package, Vec<PathShared>>,
    ) -> InstallReport {
        let records: Vec<InstallRecord> = package_to_sites
            .par_iter()
            .flat_map(|(package, sites)| {
                sites.par_iter().filter_map(|site| {
                    let fp_dist_info = package.to_dist_info_dir(site);
                    if let Ok(artifacts) = dist_info_to_artifacts(&fp_dist_info) {
                        Some(InstallRecord {
                            package: package.clone(),
                            site: site.clone(),
                            artifacts,
                        })
                    } else {
                        eprintln!("Failed to read artifacts: {:?}", fp_dist_info);
                        None
                    }
                })
            })
            .collect();

        // let mut records = Vec::new();
        // for (package, sites) in package_to_sites {
        //     for site in sites {
        //         let fp_dist_info = package.to_dist_info_dir(&site);
        //         if let Ok(artifacts) = dist_info_to_artifacts(&fp_dist_info) {
        //             records.push(InstallRecord {
        //                 package: package.clone(),
        //                 site: site.clone(),
        //                 artifacts: artifacts,
        //             });
        //         } else {
        //             eprintln!("Failed to read artifacts: {:?}", fp_dist_info);
        //         }
        //     }
        // }
        InstallReport { records }
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
        let dir_temp = tempdir().unwrap();
        let dir_dist_info = dir_temp.path().join("xarray-0.21.1.dist-info");
        fs::create_dir(&dir_dist_info).unwrap();
        let fp_record = dir_pkg.as_path().join("RECORD");

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
        let rc = dist_info_to_artifacts(&dir_dist_info).unwrap();
        // println!("{:?}", rc);
        assert_eq!(rc.files.len(), 59);
        assert_eq!(rc.dirs.len(), 1);
    }
}
