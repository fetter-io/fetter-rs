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
            .take(10)
            .filter_map(Result::ok)
            .collect::<Vec<_>>()
            .join(" "); // could be /n
        Ok(InspectTarget { name, contents })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct InspectRecord {
    site: PathShared,
    exes: Vec<PathShared>,
    files: Vec<InspectTarget>,
}

// impl InspectRecord {
//     pub(crate) fn new(
//         site: PathShared,
//         exes: Vec<PathShared>,
//         files: Vec<InspectTarget>,
//     ) -> Self {
//         InspectRecord { site, exes, files }
//     }
// }

impl Rowable for InspectRecord {
    fn to_rows(&self, context: &RowableContext) -> Vec<Vec<String>> {
        let mut rows: Vec<Vec<String>> = Vec::new();

        let exes_display = self
            .exes
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let is_tty = *context == RowableContext::Tty;
        for (i, InspectTarget { name, contents }) in self.files.iter().enumerate() {
            let (site, exes) = if i > 0 && is_tty {
                ("".to_string(), "".to_string())
            } else {
                (self.site.to_string(), exes_display.clone())
            };
            // trim content to no more than 20 chars
            rows.push(vec![
                site,
                name.clone(),
                contents.chars().take(40).collect(),
            ]);
        }
        rows
    }
}

#[derive(Debug)]
pub struct InspectReport {
    records: Vec<InspectRecord>,
}

impl InspectReport {
    /// Given a `site_to_exes` mapping from a `ScanFS`, search all sites for non-directory content.
    pub(crate) fn from_site_to_exes(
        site_to_exes: &HashMap<PathShared, Vec<PathShared>>,
    ) -> ResultDynError<Self> {
        let mut records = Vec::new();

        for (site, exes) in site_to_exes {
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
                    eprintln!("Cannot read_dir {:?}: {}", site, e);
                    continue;
                }
            };

            for dir_item in rd {
                let entry = match dir_item {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("Failed reading a DirEntry in {:?}: {}", site, e);
                        continue;
                    }
                };
                let fp: PathBuf = entry.path();
                // if a dir is here it should be standard package; could validate that both .distinfo and package exist
                if fp.is_dir() {
                    continue;
                }
                match InspectTarget::from_path(&fp) {
                    Ok(it) => files.push(it),
                    Err(e) => eprintln!("Cannot load file {:?}: {}", fp, e),
                }
            }
            records.push(InspectRecord {
                site: site.clone(),
                exes: exes.clone(),
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
            // ColumnFormat::new("Executables".to_string(), true, "#666666".to_string()),
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
    use crate::scan_fs::ScanFS;
    use std::fs::File;
    use std::io;
    use std::io::BufRead;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn test_to_file_a() {
        println("test");
    }
}
