

// given a set of packages (with defined specific versions), check if those version have vulnerabiltiies; if so provide vulnerability details for each. Vuln details can reuse AuditRecord, AuditReport

// so what we need are two specialized constructors:
// from name_or_dep_spec(Option<limit>); if a name, get all, if a dep-spec, apply filtering... or just any string can be made into a dep spec
// from_dep_manifest