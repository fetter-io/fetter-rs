use std::fmt;

use serde::{Deserialize, Serialize};

use crate::util::name_to_key;

//------------------------------------------------------------------------------
/// A package ecosystem: a family of packages sharing a naming, versioning, and distribution scheme. Each ecosystem maps to one OSV ecosystem.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Ecosystem {
    Python,
    Npm,
    Cargo,
    Gha,
    #[value(name = "vscode")]
    VsCode,
    Go,
    Ruby,
    Php,
    #[value(name = "precommit")]
    PreCommit,
    Swift,
}

impl Ecosystem {
    /// All ecosystems, in canonical order.
    pub const ALL: [Ecosystem; 10] = [
        Ecosystem::Python,
        Ecosystem::Npm,
        Ecosystem::Cargo,
        Ecosystem::Gha,
        Ecosystem::VsCode,
        Ecosystem::Go,
        Ecosystem::Ruby,
        Ecosystem::Php,
        Ecosystem::PreCommit,
        Ecosystem::Swift,
    ];

    /// The name used on the command line and in serialized output.
    pub fn cli_name(&self) -> &'static str {
        match self {
            Ecosystem::Python => "python",
            Ecosystem::Npm => "npm",
            Ecosystem::Cargo => "cargo",
            Ecosystem::Gha => "gha",
            Ecosystem::VsCode => "vscode",
            Ecosystem::Go => "go",
            Ecosystem::Ruby => "ruby",
            Ecosystem::Php => "php",
            Ecosystem::PreCommit => "precommit",
            Ecosystem::Swift => "swift",
        }
    }

    /// The ecosystem name used by OSV; see https://ossf.github.io/osv-schema/#affectedpackage-field. These are case-sensitive.
    pub fn osv_name(&self) -> &'static str {
        match self {
            Ecosystem::Python => "PyPI",
            Ecosystem::Npm => "npm",
            Ecosystem::Cargo => "crates.io",
            Ecosystem::Gha => "GitHub Actions",
            Ecosystem::VsCode => "VSCode",
            Ecosystem::Go => "Go",
            Ecosystem::Ruby => "RubyGems",
            Ecosystem::Php => "Packagist",
            // not in OSV; see `has_osv`
            Ecosystem::PreCommit => "",
            Ecosystem::Swift => "SwiftURL",
        }
    }

    /// True if the OSV API matches versions for this ecosystem server-side. GitHub Actions advisories carry only `ECOSYSTEM` ranges that OSV does not evaluate (verified live 2026-11-13), so its versions are matched client-side (`osv_match`).
    /// True if OSV has this ecosystem. pre-commit hooks are audited through the packages they install (`eco::precommit::map_hooks`).
    pub fn has_osv(&self) -> bool {
        !matches!(self, Ecosystem::PreCommit)
    }

    pub fn osv_server_side_versions(&self) -> bool {
        !matches!(self, Ecosystem::Gha)
    }

    /// Normalize a package name to a key used for identity comparisons within this ecosystem.
    pub fn name_to_key(&self, name: &str) -> String {
        match self {
            // Python and Cargo both treat `-` and `_` as equivalent, case-insensitively
            Ecosystem::Python | Ecosystem::Cargo => name_to_key(name),
            // Packagist names are case-insensitive, canonically lowercase
            Ecosystem::Npm
            | Ecosystem::Gha
            | Ecosystem::VsCode
            | Ecosystem::Php
            | Ecosystem::PreCommit
            | Ecosystem::Swift => name.to_lowercase(),
            // module paths and gem names are case-sensitive
            Ecosystem::Go | Ecosystem::Ruby => name.to_string(),
        }
    }
}

impl fmt::Display for Ecosystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.cli_name())
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_osv_name_a() {
        assert_eq!(Ecosystem::Python.osv_name(), "PyPI");
        assert_eq!(Ecosystem::VsCode.osv_name(), "VSCode");
    }

    #[test]
    fn test_name_to_key_a() {
        assert_eq!(Ecosystem::Python.name_to_key("Foo-Bar"), "foo_bar");
        assert_eq!(Ecosystem::Cargo.name_to_key("serde-json"), "serde_json");
        assert_eq!(Ecosystem::Npm.name_to_key("@Types/Node"), "@types/node");
        assert_eq!(
            Ecosystem::Gha.name_to_key("Actions/Checkout"),
            "actions/checkout"
        );
    }

    #[test]
    fn test_serde_a() {
        let s = serde_json::to_string(&Ecosystem::VsCode).unwrap();
        assert_eq!(s, "\"vscode\"");
        let e: Ecosystem = serde_json::from_str("\"gha\"").unwrap();
        assert_eq!(e, Ecosystem::Gha);
    }

    #[test]
    fn test_display_matches_cli_name_a() {
        for e in Ecosystem::ALL {
            assert_eq!(e.to_string(), e.cli_name());
        }
    }
}
