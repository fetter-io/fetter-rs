use crate::util::url_strip_user;
use serde::{Deserialize, Serialize};
use serde_json;
use std::fs::File;
use std::path::PathBuf;

// see https://packaging.python.org/en/latest/specifications/direct-url/

// NOTE: DirectURL includes url and one of three other keys:
// vcs_info: VCS request
// archive_info: direct download from a url to a whl or similar
// dir_info: url is a local directory
// assume we only need vcs_info for matching rquirements

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Hash, Clone)]
struct VcsInfo {
    commit_id: String,
    vcs: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    requested_revision: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Hash, Clone)]
pub(crate) struct DirectURL {
    url: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    vcs_info: Option<VcsInfo>,
}

impl DirectURL {
    pub(crate) fn from_file(path: &PathBuf) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("failed to open file: {}", e));
        serde_json::from_reader(file.unwrap())
            .map_err(|e| format!("failed to parse JSON: {}", e))
    }

    // Alternate constructor for test.
    #[allow(dead_code)]
    pub(crate) fn from_url_vcs_cid(
        url: String,
        vcs: Option<String>,
        commit_id: Option<String>,
    ) -> Result<Self, String> {
        let vcs_info: Option<VcsInfo>;
        if vcs.is_some() && commit_id.is_some() {
            vcs_info = Some(VcsInfo {
                vcs: vcs.unwrap(),
                commit_id: commit_id.unwrap(),
                requested_revision: None,
            });
        } else {
            vcs_info = None;
        }
        Ok(DirectURL { url, vcs_info })
    }

    //--------------------------------------------------------------------------

    // Combine components to produce a URL string as used in a DepSpec
    // fn get_url_origin(&self) -> String {
    //     // if we have vcs_info, need to put vcs+ in from and @commit id in the back
    //     if let Some(vcs_info) = &self.vcs_info {
    //         // use requested_revision if defined, else commit_id
    //         let target = match &vcs_info.requested_revision {
    //             Some(requested_revision) => requested_revision,
    //             None => &vcs_info.commit_id,
    //         };
    //         format!("{}+{}@{}", vcs_info.vcs, self.url, target)
    //     } else {
    //         self.url.clone()
    //     }
    // }

    // pub(crate) fn validate(&self, url: &String) -> bool {
    //     // println!(
    //     //     "package durl url origin:\n{}\ndepspec url:\n{}\n",
    //     //     self.get_url_origin(),
    //     //     *url
    //     // );
    //     let url_origin = self.get_url_origin();
    //     url_strip_user(&url_origin) == url_strip_user(url)
    // }

    // Given a URL from a DepSpec, validate against this URL from a Package's DirectURL. We strip the user in comparison from both sides as inconsistencies are found in how DirectURL records these.
    pub(crate) fn validate(&self, url: &String) -> bool {
        let url_dep_spec = url_strip_user(url);
        let url_durl = url_strip_user(&self.url);

        if let Some(vcs_info) = &self.vcs_info {
            // use requested_revision if defined, else commit_id
            if let Some(requested_revision) = &vcs_info.requested_revision {
                if format!("{}+{}@{}", vcs_info.vcs, url_durl, requested_revision)
                    == url_dep_spec
                {
                    return true;
                }
            }
            if format!("{}+{}@{}", vcs_info.vcs, url_durl, vcs_info.commit_id)
                == url_dep_spec
            {
                return true;
            }
            return false;
        }
        return url_durl == url_dep_spec;
    }
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

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

        let durl: DirectURL =
            serde_json::from_str(json_str).expect("Failed to parse JSON");
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

        let durl: DirectURL =
            serde_json::from_str(json_str).expect("Failed to parse JSON");
        assert_eq!("ssh://git@github.com/uqfoundation/dill.git", durl.url);
        assert_eq!("git", durl.vcs_info.as_ref().unwrap().vcs);
        assert_eq!(
            "a0a8e86976708d0436eec5c8f7d25329da727cb5",
            durl.vcs_info.as_ref().unwrap().commit_id
        );
        assert_eq!(
            "0.3.8",
            durl.vcs_info
                .as_ref()
                .unwrap()
                .requested_revision
                .as_ref()
                .unwrap()
        );
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

    //--------------------------------------------------------------------------
    #[test]
    fn test_durl_from_file_a() {
        let temp_dir = tempdir().unwrap();
        let fp_durl = temp_dir.path().join("direct_url.json");
        let content = r#"
        {"url": "ssh://git@github.com/uqfoundation/dill.git", "vcs_info": {"commit_id": "a0a8e86976708d0436eec5c8f7d25329da727cb5", "requested_revision": "0.3.8", "vcs": "git"}}
        "#;
        let mut file = File::create(&fp_durl).unwrap();
        write!(file, "{}", content).unwrap();

        let durl = DirectURL::from_file(&fp_durl).unwrap();
        assert_eq!("ssh://git@github.com/uqfoundation/dill.git", durl.url);
    }

    //--------------------------------------------------------------------------
    #[test]
    fn test_validate_a() {
        // from pip3 install "git+ssh://git@github.com/uqfoundation/dill.git@0.3.8"
        let json_str = r#"
        {"url": "ssh://git@github.com/uqfoundation/dill.git", "vcs_info": {"commit_id": "a0a8e86976708d0436eec5c8f7d25329da727cb5", "requested_revision": "0.3.8", "vcs": "git"}}
        "#;
        let durl: DirectURL = serde_json::from_str(json_str).unwrap();
        assert_eq!(
            durl.validate(
                &"git+ssh://git@github.com/uqfoundation/dill.git@0.3.8".to_string()
            ),
            true
        );
        assert_eq!(
            durl.validate(
                &"git+ssh://git@github.com/uqfoundation/dill.git@0.3.7".to_string()
            ),
            false
        );
        assert_eq!(
            durl.validate(
                &"git+ssh://github.com/uqfoundation/dill.git@0.3.8".to_string()
            ),
            true
        );
        assert_eq!(
            durl.validate(&"git+ssh://github.com/uqfoundation/dill.git@a0a8e86976708d0436eec5c8f7d25329da727cb5".to_string()),
            true
        );
        assert_eq!(
            durl.validate(&"git+ssh://github.com/uqfoundation/dill.git@a0a8e86976708d0436e5c8f7d25329da727cb5".to_string()),
            false
        );
    }
}
