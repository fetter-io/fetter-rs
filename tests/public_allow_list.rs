//! The allow-list API as used by a library client (e.g. a server storing `fetter.toml` bounds and observed packages), through public items only.

use std::collections::HashMap;

use fetter::AllowList;
use fetter::Anchor;
use fetter::Dependency;
use fetter::Ecosystem;
use fetter::Package;
use fetter::Site;
use fetter::SiteKind;
use fetter::ValidationExplain;
use fetter::ValidationFlags;
use fetter::ValidationReport;
use fetter::VersionSpec;

fn observed() -> (Vec<Package>, HashMap<Package, Vec<Site>>) {
    let mk = |e, n, v| Package::new(e, n, VersionSpec::new(v), None);
    let mut packages = vec![
        mk(Ecosystem::Python, "requests", "2.31.0"),
        mk(Ecosystem::Npm, "lodash", "4.17.15"),
        mk(Ecosystem::Npm, "lodash", "4.17.21"),
        mk(Ecosystem::Cargo, "serde", "1.0.200"),
    ];
    packages.sort();
    let p2s = packages
        .iter()
        .map(|p| {
            let site = Site::new(p.ecosystem, SiteKind::Lockfile, "/x".into());
            (p.clone(), vec![site])
        })
        .collect();
    (packages, p2s)
}

#[test]
fn test_derive_write_read_validate() {
    let (packages, p2s) = observed();
    // derive from observed packages, write, and read back as stored text
    let derived = AllowList::from_packages(&packages, Anchor::Lower).unwrap();
    let toml = derived.to_toml();
    assert!(toml.contains("[npm]\n\"lodash\" = \">=4.17.15\"\n"));
    let al = AllowList::from_text(&toml).unwrap();
    assert_eq!(
        al.ecosystems(),
        vec![Ecosystem::Python, Ecosystem::Npm, Ecosystem::Cargo]
    );
    assert_eq!(al.counts()[&Ecosystem::Npm], 1);

    let vf = ValidationFlags {
        permit_superset: false,
        permit_subset: false,
    };
    let ecosystems: Vec<Ecosystem> = Ecosystem::ALL.to_vec();
    let vr = ValidationReport::from_allow_list(
        &ecosystems,
        &packages,
        &p2s,
        &HashMap::new(),
        &None,
        &al,
        &vf,
        None,
        false,
    );
    assert!(vr.is_empty());

    // tighten npm: the older lodash is misdefined
    let al = AllowList::from_text("[npm]\nlodash = \"^4.17.21\"\n").unwrap();
    let vr = ValidationReport::from_allow_list(
        &ecosystems,
        &packages,
        &p2s,
        &HashMap::new(),
        &None,
        &al,
        &vf,
        None,
        false,
    );
    assert_eq!(vr.records.len(), 1);
    let r = &vr.records[0];
    assert_eq!(r.ecosystem, Ecosystem::Npm);
    assert!(matches!(r.explain(), ValidationExplain::Misdefined));
    let Some(Dependency::Other(entry)) = &r.dependency else {
        panic!("expected an npm requirement");
    };
    assert_eq!(
        (entry.name(), entry.specs()),
        ("lodash", &["^4.17.21".to_string()][..])
    );
    assert_eq!(
        r.package.as_ref().unwrap().purl().as_deref(),
        Some("pkg:npm/lodash@4.17.15")
    );
}

#[test]
fn test_python_text() {
    let al = AllowList::from_text("requests>=2.31\n").unwrap();
    assert!(al.is_python_only());
    let dm = al.python().unwrap();
    assert_eq!(
        dm.to_dep_manifest_report().records[0].dep_spec.to_string(),
        "requests>=2.31"
    );
}
