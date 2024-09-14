use serde::{Deserialize, Serialize};

// see https://packaging.python.org/en/latest/specifications/direct-url/

// NOTE: DirectURL includes url and one of three other keys:
// vcs_info: VCS request
// archive_info: direct download from a url to a whl or similar
// dir_info: url is a local directory

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
        // from pip3 install "git+ssh://git@github.com/uqfoundation/dill.git"
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
        // from pip3 install "git+ssh://git@github.com/uqfoundation/dill.git@0.3.8"
        let json_str = r#"
        {"url": "ssh://git@github.com/uqfoundation/dill.git", "vcs_info": {"commit_id": "a0a8e86976708d0436eec5c8f7d25329da727cb5", "requested_revision": "0.3.8", "vcs": "git"}}
        "#;

        let durl: DirectURL = serde_json::from_str(json_str).expect("Failed to parse JSON");
        assert_eq!("ssh://git@github.com/uqfoundation/dill.git", durl.url);
        assert_eq!("git", durl.vcs_info.as_ref().unwrap().vcs);
        assert_eq!(
            "a0a8e86976708d0436eec5c8f7d25329da727cb5",
            durl.vcs_info.as_ref().unwrap().commit_id
        );
        assert_eq!("0.3.8", durl.vcs_info.as_ref().unwrap().requested_revision.as_ref().unwrap());
    }

    #[test]
    fn test_durl_c() {
        // from: pip install https://files.pythonhosted.org/packages/d9/5a/e7c31adbe875f2abbb91bd84cf2dc52d792b5a01506781dbcf25c91daf11/six-1.16.0-py2.py3-none-any.whl
        let json_str = r#"
          {
            "archive_info": {
              "hash": "sha256=8abb2f1d86890a2dfb989f9a77cfcfd3e47c2a354b01111771326f8aa26e0254",
              "hashes": {
                "sha256": "8abb2f1d86890a2dfb989f9a77cfcfd3e47c2a354b01111771326f8aa26e0254"
              }
            },
            "url": "https://files.pythonhosted.org/packages/d9/5a/e7c31adbe875f2abbb91bd84cf2dc52d792b5a01506781dbcf25c91daf11/six-1.16.0-py2.py3-none-any.whl"
          }
          "#;
        let durl: DirectURL = serde_json::from_str(json_str).unwrap();
        assert_eq!("https://files.pythonhosted.org/packages/d9/5a/e7c31adbe875f2abbb91bd84cf2dc52d792b5a01506781dbcf25c91daf11/six-1.16.0-py2.py3-none-any.whl", durl.url);

    }
}
