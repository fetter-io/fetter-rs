// use std::fmt;
use std::cmp;
use std::path::PathBuf;

use crate::dep_spec::DepSpec;
use crate::package::Package;

// #[derive(PartialEq, Eq, Hash, Clone)]
#[derive(Debug)]
pub(crate) struct ValidationItem {
    package: Package,
    dep_spec: DepSpec,
    sites: Vec<PathBuf>,
}

impl ValidationItem {
    pub(crate) fn new(package: Package, dep_spec: DepSpec, sites: Vec<PathBuf>) -> Self {
        ValidationItem {
            package,
            dep_spec,
            sites,
        }
    }
}

#[derive(Debug)]
pub struct Validation {
    pub items: Vec<ValidationItem>,
}

impl Validation {
    pub fn display(&self) {
        let mut package_displays: Vec<String> = Vec::new();
        let mut dep_spec_displays: Vec<String> = Vec::new();
        let mut max_package_width = 0;
        let mut max_dep_spec_width = 0;

        for item in &self.items {
            let pkg_display = format!("{}", item.package);
            let dep_display = format!("{}", item.dep_spec);

            max_package_width = cmp::max(max_package_width, pkg_display.len());
            max_dep_spec_width = cmp::max(max_dep_spec_width, dep_display.len());

            package_displays.push(pkg_display);
            dep_spec_displays.push(dep_display);
        }
        // Header
        println!(
            "{:<package_width$} {:<dep_spec_width$}",
            "Package",
            "Dependency",
            package_width = max_package_width,
            dep_spec_width = max_dep_spec_width
        );

        // Print each item with alignment
        for (pkg_display, dep_display) in package_displays.iter().zip(dep_spec_displays.iter()) {
            println!(
                "{:<package_width$} {:<dep_spec_width$}",
                pkg_display,
                dep_display,
                package_width = max_package_width,
                dep_spec_width = max_dep_spec_width
            );
        }
    }
}
