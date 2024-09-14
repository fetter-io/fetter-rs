use serde::{Deserialize, Serialize};

// see https://packaging.python.org/en/latest/specifications/direct-url/

#[derive(Debug, Serialize, Deserialize)]
struct VcsInfo {
    commit_id: String,
    vcs: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    requested_revision: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DirectURL {
    url: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    vcs_info: Option<VcsInfo>,
}


//------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_durl_a() {
        let json_str = r#"
        {
            "url": "ssh://git@github.com/uqfoundation/dill.git",
            "vcs_info": {
                "commit_id": "15d7c6d6ccf4781c624ffbf54c90d23c6e94dc52",
                "vcs": "git"
            }
        }
        "#;

        let durl: DirectURL = serde_json::from_str(json_str).expect("Failed to parse JSON");

        // Print the durl information
        assert_eq!("ssh://git@github.com/uqfoundation/dill.git", durl.url);
        assert_eq!("git", durl.vcs_info.as_ref().unwrap().vcs);
        assert_eq!(
            "15d7c6d6ccf4781c624ffbf54c90d23c6e94dc52",
            durl.vcs_info.as_ref().unwrap().commit_id
        );
        assert!(durl.vcs_info.as_ref().unwrap().requested_revision.is_none());
    }

    #[test]
    fn test_durl_b() {
        let json_str = r#"
        {"url": "ssh://git@github.com/uqfoundation/dill.git", "vcs_info": {"commit_id": "a0a8e86976708d0436eec5c8f7d25329da727cb5", "requested_revision": "0.3.8", "vcs": "git"}}
        "#;

        // Deserialize the JSON content into the DirectURL struct
        let durl: DirectURL = serde_json::from_str(json_str).expect("Failed to parse JSON");

        // Print the durl information
        assert_eq!("ssh://git@github.com/uqfoundation/dill.git", durl.url);
        assert_eq!("git", durl.vcs_info.as_ref().unwrap().vcs);
        assert_eq!(
            "a0a8e86976708d0436eec5c8f7d25329da727cb5",
            durl.vcs_info.as_ref().unwrap().commit_id
        );
        assert_eq!("0.3.8", durl.vcs_info.as_ref().unwrap().requested_revision.as_ref().unwrap());
    }
}

// install examples
// pip3 install "git+ssh://git@github.com/uqfoundation/dill.git"
// pip3 install "git+ssh://git@github.com/uqfoundation/dill.git@0.3.8"

//
