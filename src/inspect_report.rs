use std::collections::HashMap;

use rayon::prelude::*;

use crate::eco::scanner_for;
use crate::eco::Hook;
use crate::ecosystem::Ecosystem;
use crate::origin_site::Origin;
use crate::origin_site::Site;
use crate::path_shared::PathShared;
use crate::table::ColumnFormat;
use crate::table::Rowable;
use crate::table::RowableContext;
use crate::table::Tableable;
use crate::util::ResultDynError;

/// Characters of a hook's detail shown per row.
const DETAIL_CHARS: usize = 60;

//------------------------------------------------------------------------------
/// The hooks found in one site.
#[derive(Debug, Clone)]
pub(crate) struct InspectRecord {
    ecosystem: Ecosystem,
    site: PathShared,
    hooks: Vec<Hook>,
}

impl Rowable for InspectRecord {
    fn to_rows(&self, context: &RowableContext) -> Vec<Vec<String>> {
        let is_tty = *context == RowableContext::Tty;
        self.hooks
            .iter()
            .enumerate()
            .map(|(i, Hook { name, detail })| {
                // in a TTY, show the ecosystem and site only on a site's first row
                let (eco, site) = if i > 0 && is_tty {
                    (String::new(), String::new())
                } else {
                    (self.ecosystem.to_string(), self.site.to_string())
                };
                vec![
                    eco,
                    site,
                    name.clone(),
                    detail.chars().take(DETAIL_CHARS).collect(),
                ]
            })
            .collect()
    }
}

#[derive(Debug)]
pub struct InspectReport {
    records: Vec<InspectRecord>,
}

impl InspectReport {
    /// Collect hooks from every site, via each site's ecosystem scanner (sites of ecosystems without hooks yield none). Records are sorted by site, and hooks within a site by name.
    pub(crate) fn from_site_to_origins(
        site_to_origins: &HashMap<Site, Vec<Origin>>,
    ) -> ResultDynError<Self> {
        let mut sites: Vec<&Site> = site_to_origins.keys().collect();
        sites.sort();
        let records = sites
            .into_par_iter()
            .filter_map(|site| {
                let mut hooks = scanner_for(site.ecosystem)?.site_to_hooks(site);
                if hooks.is_empty() {
                    return None;
                }
                hooks.sort();
                hooks.dedup();
                Some(InspectRecord {
                    ecosystem: site.ecosystem,
                    site: site.path.clone(),
                    hooks,
                })
            })
            .collect();
        Ok(InspectReport { records })
    }
}

impl Tableable<InspectRecord> for InspectReport {
    fn get_header(&self) -> Vec<ColumnFormat> {
        vec![
            ColumnFormat::new("Ecosystem".to_string(), false, "#666666".to_string()),
            ColumnFormat::new("Site".to_string(), true, "#666666".to_string()),
            ColumnFormat::new("Hook".to_string(), false, "#666666".to_string()),
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
    use crate::origin_site::SiteKind;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn python_site(path: &Path) -> (Site, Vec<Origin>) {
        (
            Site::python(PathShared::from(path.to_path_buf())),
            vec![Origin::new(
                Ecosystem::Python,
                PathShared::from(PathBuf::from("/fake/python3")),
            )],
        )
    }

    #[test]
    fn test_inspect_report_python_a() {
        let dir = tempdir().unwrap();
        let site1 = dir.path().join("site-packages");
        fs::create_dir(&site1).unwrap();
        fs::write(
            site1.join("sitecustomize.py"),
            "# Site customization\nimport sys\n",
        )
        .unwrap();
        fs::write(site1.join("custom.pth"), "/some/custom/path\n").unwrap();
        fs::write(site1.join("regular.py"), "print('regular')\n").unwrap();
        fs::write(site1.join("readme.txt"), "Some readme\n").unwrap();
        fs::create_dir(site1.join("package")).unwrap();

        let site_to_origins: HashMap<Site, Vec<Origin>> =
            [python_site(&site1)].into_iter().collect();
        let report = InspectReport::from_site_to_origins(&site_to_origins).unwrap();
        let records = report.get_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].site.as_path(), site1);
        let names: Vec<&str> = records[0].hooks.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["custom.pth", "sitecustomize.py"]);

        let rows = records[0].to_rows(&RowableContext::Delimited);
        assert_eq!(rows[0][0], "python");
        assert_eq!(rows[0][2], "custom.pth");
        assert_eq!(rows[1][3], "# Site customization import sys");
        // in a TTY, ecosystem and site appear on the first row only
        let rows = records[0].to_rows(&RowableContext::Tty);
        assert_eq!(rows[1][0], "");
        assert_eq!(rows[1][1], "");
    }

    #[test]
    fn test_inspect_report_multiple_sites_a() {
        let dir = tempdir().unwrap();
        let site1 = dir.path().join("b/site-packages");
        let site2 = dir.path().join("a/site-packages");
        let empty = dir.path().join("c/site-packages");
        for s in [&site1, &site2, &empty] {
            fs::create_dir_all(s).unwrap();
        }
        fs::write(site1.join("usercustomize.py"), "import os\n").unwrap();
        fs::write(site2.join("extra.pth"), "/extra/path\n").unwrap();

        let site_to_origins: HashMap<Site, Vec<Origin>> = [
            python_site(&site1),
            python_site(&site2),
            python_site(&empty),
        ]
        .into_iter()
        .collect();
        let report = InspectReport::from_site_to_origins(&site_to_origins).unwrap();
        let records = report.get_records();
        // sorted by site; sites without hooks are omitted
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].site.as_path(), site2);
        assert_eq!(records[1].site.as_path(), site1);
    }

    #[test]
    fn test_inspect_report_truncates_detail_a() {
        let record = InspectRecord {
            ecosystem: Ecosystem::Npm,
            site: PathShared::from("/p/node_modules"),
            hooks: vec![Hook {
                name: "esbuild-0.21.5 postinstall".to_string(),
                detail: "x".repeat(200),
            }],
        };
        let rows = record.to_rows(&RowableContext::Delimited);
        assert_eq!(rows[0][3].chars().count(), DETAIL_CHARS);
    }

    #[test]
    fn test_inspect_report_ecosystems_without_hooks_a() {
        // a cargo lockfile site has no hooks and yields no record
        let dir = tempdir().unwrap();
        let lock = dir.path().join("Cargo.lock");
        fs::write(&lock, "version = 4\n").unwrap();
        let site =
            Site::new(Ecosystem::Cargo, SiteKind::Lockfile, PathShared::from(lock));
        let site_to_origins: HashMap<Site, Vec<Origin>> =
            [(site, Vec::new())].into_iter().collect();
        let report = InspectReport::from_site_to_origins(&site_to_origins).unwrap();
        assert!(report.get_records().is_empty());
    }
}
