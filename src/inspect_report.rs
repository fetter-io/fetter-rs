use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::io;
use std::io::BufRead;
// use crate::package::Package;
use crate::path_shared::PathShared;
use crate::table::ColumnFormat;
use crate::table::Rowable;
use crate::table::RowableContext;
use crate::table::Tableable;
use crate::util::ResultDynError;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) struct InspectTarget {
    name: String,
    contents: String,
}

impl InspectTarget {
    fn from_path(fp: &Path) -> ResultDynError<InspectTarget> {
        let name = fp
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<invalid utf8>")
            .to_string();

        let file = File::open(fp)?;
        let reader = io::BufReader::new(file);
        let contents: String = reader
            .lines()
            .take(10) // take first 10 lines; will truncate later
            .filter_map(Result::ok)
            .collect::<Vec<_>>()
            .join(" "); // could be /n
        Ok(InspectTarget { name, contents })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct InspectRecord {
    site: PathShared,
    files: Vec<InspectTarget>,
}

impl Rowable for InspectRecord {
    fn to_rows(&self, context: &RowableContext) -> Vec<Vec<String>> {
        let mut rows: Vec<Vec<String>> = Vec::new();

        // let exes_display = self
        //     .exes
        //     .iter()
        //     .map(|p| p.to_string())
        //     .collect::<Vec<_>>()
        //     .join(",");

        let is_tty = *context == RowableContext::Tty;
        for (i, InspectTarget { name, contents }) in self.files.iter().enumerate() {
            let site = if i > 0 && is_tty {
                "".to_string()
            } else {
                self.site.to_string()
            };
            rows.push(vec![
                site,
                name.clone(),
                contents.chars().take(60).collect(), // trim content to no more than 20 chars
            ]);
        }
        rows
    }
}

#[derive(Debug)]
pub struct InspectReport {
    records: Vec<InspectRecord>,
}

const EXT_KEEP: [&str; 2] = ["py", "pth"];
pub(crate) const PY_NAME_KEEP: [&str; 2] = ["sitecustomize.py", "usercustomize.py"];

impl InspectReport {
    /// Given a `site_to_exes` mapping from a `ScanFS`, search all sites for non-directory content.
    pub(crate) fn from_site_to_exes(
        site_to_exes: &HashMap<PathShared, Vec<PathShared>>,
    ) -> ResultDynError<Self> {
        let mut records = Vec::new();

        for site in site_to_exes.keys() {
            let mut files: Vec<InspectTarget> = Vec::new();
            // Skip sites that don't exist or are not dirs
            if !site.as_path().is_dir() {
                // eprintln!("Missing dir: {:?}", site);
                continue;
            }
            // read_dir errors
            let rd = match fs::read_dir(site) {
                Ok(it) => it,
                Err(e) => {
                    eprintln!("Cannot read_dir {site:?}: {e}");
                    continue;
                }
            };

            for dir_item in rd {
                let entry = match dir_item {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("Failed reading a DirEntry in {site:?}: {e}");
                        continue;
                    }
                };
                let fp: PathBuf = entry.path();
                // if a dir is here it should be standard package; could validate that both .distinfo and package exist
                if fp.is_dir() {
                    continue;
                }
                // skip extensions we do not care about
                let ext = fp.extension().and_then(|e| e.to_str()).unwrap_or("");

                if EXT_KEEP.contains(&ext) {
                    let name = fp.file_name().and_then(|s| s.to_str()).unwrap_or("");

                    if ext == "py" && !PY_NAME_KEEP.contains(&name) {
                        continue;
                    }
                    match InspectTarget::from_path(&fp) {
                        Ok(it) => files.push(it),
                        Err(e) => eprintln!("Cannot load file {fp:?}: {e}"),
                    }
                }
            }
            records.push(InspectRecord {
                site: site.clone(),
                files,
            });
        }

        Ok(InspectReport { records })
    }
}

impl Tableable<InspectRecord> for InspectReport {
    fn get_header(&self) -> Vec<ColumnFormat> {
        vec![
            ColumnFormat::new("Site".to_string(), true, "#666666".to_string()),
            ColumnFormat::new("File".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Content".to_string(), true, "#666666".to_string()),
        ]
    }
    fn get_records(&self) -> &Vec<InspectRecord> {
        &self.records
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_inspect_target_from_path_basic() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.py");
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "import sys").unwrap();
        writeln!(file, "print('hello')").unwrap();

        let target = InspectTarget::from_path(&file_path).unwrap();
        assert_eq!(target.name, "test.py");
        assert!(target.contents.contains("import sys"));
        assert!(target.contents.contains("print('hello')"));
    }

    #[test]
    fn test_inspect_target_from_path_long_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("long.py");
        let mut file = File::create(&file_path).unwrap();

        // Write 20 lines, but only first 10 should be captured
        for i in 0..20 {
            writeln!(file, "line {}", i).unwrap();
        }

        let target = InspectTarget::from_path(&file_path).unwrap();
        assert_eq!(target.name, "long.py");
        assert!(target.contents.contains("line 0"));
        assert!(target.contents.contains("line 9"));
        assert!(!target.contents.contains("line 10"));
        assert!(!target.contents.contains("line 19"));
    }

    #[test]
    fn test_inspect_target_from_path_empty_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("empty.pth");
        File::create(&file_path).unwrap();

        let target = InspectTarget::from_path(&file_path).unwrap();
        assert_eq!(target.name, "empty.pth");
        assert_eq!(target.contents, "");
    }

    #[test]
    fn test_inspect_target_from_path_nonexistent() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("nonexistent.py");

        let result = InspectTarget::from_path(&file_path);
        assert!(result.is_err());
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_inspect_report_from_site_to_exes() {
        let dir = tempdir().unwrap();

        // Create a site directory with various files
        let site1 = dir.path().join("site-packages");
        fs::create_dir(&site1).unwrap();

        // Create a .py file that should be included (sitecustomize.py)
        let sitecustomize = site1.join("sitecustomize.py");
        let mut file = File::create(&sitecustomize).unwrap();
        writeln!(file, "# Site customization").unwrap();
        writeln!(file, "import sys").unwrap();

        // Create a .pth file that should be included
        let pth_file = site1.join("custom.pth");
        let mut file = File::create(&pth_file).unwrap();
        writeln!(file, "/some/custom/path").unwrap();

        // Create a regular .py file that should be excluded
        let regular_py = site1.join("regular.py");
        let mut file = File::create(&regular_py).unwrap();
        writeln!(file, "print('regular')").unwrap();

        // Create a directory (should be skipped)
        let package_dir = site1.join("somepackage");
        fs::create_dir(&package_dir).unwrap();

        // Create a .txt file (should be excluded)
        let txt_file = site1.join("readme.txt");
        let mut file = File::create(&txt_file).unwrap();
        writeln!(file, "Some readme").unwrap();

        // Create site_to_exes mapping with dummy exe paths
        let mut site_to_exes = HashMap::new();
        site_to_exes.insert(
            PathShared::from(site1.clone()),
            vec![PathShared::from(PathBuf::from("/fake/python3"))],
        );

        let report = InspectReport::from_site_to_exes(&site_to_exes).unwrap();
        let records = report.get_records();

        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.site.as_path(), site1);

        // Should have found 2 files: sitecustomize.py and custom.pth
        assert_eq!(record.files.len(), 2);

        let file_names: Vec<String> =
            record.files.iter().map(|f| f.name.clone()).collect();
        assert!(file_names.contains(&"sitecustomize.py".to_string()));
        assert!(file_names.contains(&"custom.pth".to_string()));

        // Verify contents
        let sitecustomize_target = record
            .files
            .iter()
            .find(|f| f.name == "sitecustomize.py")
            .unwrap();
        assert!(sitecustomize_target.contents.contains("Site customization"));

        let pth_target = record
            .files
            .iter()
            .find(|f| f.name == "custom.pth")
            .unwrap();
        assert!(pth_target.contents.contains("/some/custom/path"));
    }

    #[test]
    fn test_inspect_report_multiple_sites() {
        let dir = tempdir().unwrap();

        // Create two site directories
        let site1 = dir.path().join("site-packages1");
        let site2 = dir.path().join("site-packages2");
        fs::create_dir(&site1).unwrap();
        fs::create_dir(&site2).unwrap();

        // Add usercustomize.py to site1
        let usercustomize = site1.join("usercustomize.py");
        let mut file = File::create(&usercustomize).unwrap();
        writeln!(file, "# User customization").unwrap();

        // Add .pth file to site2
        let pth_file = site2.join("extra.pth");
        let mut file = File::create(&pth_file).unwrap();
        writeln!(file, "/extra/path").unwrap();

        // Create site_to_exes mapping
        let mut site_to_exes = HashMap::new();
        site_to_exes.insert(
            PathShared::from(site1.clone()),
            vec![PathShared::from(PathBuf::from("/fake/python3.11"))],
        );
        site_to_exes.insert(
            PathShared::from(site2.clone()),
            vec![PathShared::from(PathBuf::from("/fake/python3.12"))],
        );

        let report = InspectReport::from_site_to_exes(&site_to_exes).unwrap();
        let records = report.get_records();

        assert_eq!(records.len(), 2);

        // Find each site's record
        let record1 = records.iter().find(|r| r.site.as_path() == site1).unwrap();
        let record2 = records.iter().find(|r| r.site.as_path() == site2).unwrap();

        assert_eq!(record1.files.len(), 1);
        assert_eq!(record1.files[0].name, "usercustomize.py");

        assert_eq!(record2.files.len(), 1);
        assert_eq!(record2.files[0].name, "extra.pth");
    }
}
