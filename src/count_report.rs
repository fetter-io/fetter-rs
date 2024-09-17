use std::cmp;
use std::collections::HashSet;
use std::fs::File;
use std::io;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug)]
pub(crate) struct CountRecord {
    key: String,
    value: usize,
}

impl CountRecord {
    pub(crate) fn new(key: String, value: usize) -> Self {
        CountRecord { key, value }
    }
}

#[derive(Debug)]
pub struct CountReport {
    records: Vec<CountRecord>,
}

impl CountReport {
    pub(crate) fn new(records: Vec<CountRecord>) -> Self {
        CountReport { records }
    }

    fn to_writer<W: Write>(
        &self,
        mut writer: W,
        delimiter: char,
        repeat_package: bool,
    ) -> io::Result<()> {
        let mut package_displays: Vec<String> = Vec::new();
        let mut max_package_width = 0;

        let mut records: Vec<&CountRecord> = self.records.iter().collect();
        records.sort_by_key(|item| &item.key);

        for item in &records {
            let pkg_display = format!("{}", item.key);
            max_package_width = cmp::max(max_package_width, pkg_display.len());
            package_displays.push(pkg_display);
        }
        writeln!(
            writer,
            "{:<package_width$}{}{}",
            "String",
            delimiter,
            "Site",
            package_width = max_package_width,
        );

        for (pkg_display, record) in package_displays.iter().zip(records.iter()) {
            writeln!(
                writer,
                "{:<package_width$}{}{}",
                pkg_display,
                delimiter,
                record.value,
                package_width = max_package_width,
            );
        }
        Ok(())
    }

    pub fn to_file(&self, file_path: &PathBuf, delimiter: char) -> io::Result<()> {
        let file = File::create(file_path)?;
        self.to_writer(file, delimiter, true)
    }

    pub(crate) fn to_stdout(&self) {
        let stdout = io::stdout();
        let handle = stdout.lock();
        self.to_writer(handle, ' ', false).unwrap();
    }
}

// TODO: need tests
