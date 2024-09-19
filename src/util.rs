// Normalize all names
pub(crate) fn name_to_key(name: &String) -> String {
    name.replace("-", "_")
}
