use std::collections::HashMap;

use crate::package::Package;
use crate::path_shared::PathShared;
use crate::table::ColumnFormat;
use crate::table::Rowable;
use crate::table::RowableContext;
use crate::table::Tableable;

#[derive(Debug, Clone)]
pub(crate) struct InspectTarget {
    file: PathShared,
    contents: String,
}


#[derive(Debug, Clone)]
pub(crate) struct InspectRecord {
    site: PathShared,
    exes: Vec<PathShared>,
    files: Vec<InspectTarget>,
}

impl InspectRecord {
    pub(crate) fn new(site: PathShared, exes: Vec<PathShared>, files: Vec<InspectTarget>) -> Self {
        InspectRecord { site, exes, files }
    }
}

impl Rowable for InspectRecord {
    fn to_rows(&self, context: &RowableContext) -> Vec<Vec<String>> {
        let mut rows: Vec<Vec<String>> = Vec::new();

        let is_tty = *context == RowableContext::Tty;

        for (i, (file, contents)) in self.files.iter().enumerate() {
            let site = if i > 0 && is_tty {
                "".to_string()
            } else {
                self.site.to_string()
            };

            rows.push(vec![site, file.to_string(), contents.clone()]);
        }
        rows
    }
}

#[derive(Debug)]
pub struct InspectReport {
    records: Vec<InspectRecord>,
}

impl InspectReport {
    pub(crate) fn from_site_to_exes(
        site_to_exes: &HashMap<PathShared, Vec<PathShared>> ,
    ) -> Self {
        let mut records = Vec::new();
        InspectReport { records }
    }

}

impl Tableable<InspectRecord> for InspectReport {
    fn get_header(&self) -> Vec<ColumnFormat> {
        vec![
            ColumnFormat::new("Site".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Executables".to_string(), true, "#666666".to_string()),
            ColumnFormat::new("File".to_string(), true, "#666666".to_string()),
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
