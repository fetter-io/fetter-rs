// Normalize all names
pub(crate) fn name_to_key(name: &String) -> String {
    name.to_lowercase().replace("-", "_")
}

pub(crate) fn url_strip_user(url: &String) -> String {
    if let Some(pos_protocol) = url.find("://") {
        let pos_start = pos_protocol + 3;
        // get span to first @ if it exists
        if let Some(pos_span) = url[pos_start..].find('@') {
            let pos_end = pos_start + pos_span;
            // within start and end, there should not be
            if url[pos_start..pos_end].find('/').is_none() {
                return format!("{}{}", &url[..pos_start], &url[pos_end..]);
            }
        }
    }
    url.to_string()
}
