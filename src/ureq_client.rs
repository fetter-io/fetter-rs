#![allow(clippy::result_large_err)]

use std::collections::HashMap;

pub trait UreqClient: Send + Sync {
    /// A post request to the given URL with the provided JSON body.
    fn post(&self, url: &str, body: &str) -> Result<String, ureq::Error>;
    /// A post request, as `post`, that returns the status and body of any response, including an error status (whose body says why, e.g. a server's refusal); an `Err` is a failure to get a response. By default this is `post`, with status 200.
    fn post_status(&self, url: &str, body: &str) -> Result<(u16, String), ureq::Error> {
        self.post(url, body).map(|b| (200, b))
    }
    /// A get request
    fn get(&self, url: &str) -> Result<String, ureq::Error>;
    /// A get request with an `Accept` header (e.g. npm's abbreviated metadata). By default the header is ignored.
    fn get_with_accept(&self, url: &str, _accept: &str) -> Result<String, ureq::Error> {
        self.get(url)
    }
    /// The output of `git ls-remote --tags` for a repository URL (with `--refs` if `refs_only`, which omits peeled `^{}` lines); an error is git's stderr. Terminal prompts are disabled: GitHub asks for credentials for a repository that does not exist (or is private), which would otherwise hang. By default this runs `git`.
    fn git_ls_remote_tags(&self, url: &str, refs_only: bool) -> Result<String, String> {
        let mut args = vec!["ls-remote", "--tags"];
        if refs_only {
            args.push("--refs");
        }
        args.push(url);
        let output = std::process::Command::new("git")
            .args(&args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| {
                format!("Failed to run git (required for GitHub Actions tags): {e}")
            })?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    }
}

#[derive(Debug)]
pub struct UreqClientLive;

impl UreqClient for UreqClientLive {
    fn post(&self, url: &str, body: &str) -> Result<String, ureq::Error> {
        let mut response = ureq::post(url)
            .header("Content-Type", "application/json")
            .send(body)?;
        response.body_mut().read_to_string()
    }
    fn post_status(&self, url: &str, body: &str) -> Result<(u16, String), ureq::Error> {
        let mut response = ureq::post(url)
            .config()
            .http_status_as_error(false)
            .build()
            .header("Content-Type", "application/json")
            .send(body)?;
        let status = response.status().as_u16();
        // an unreadable body still has a status worth reporting
        let text = response.body_mut().read_to_string().unwrap_or_default();
        Ok((status, text))
    }
    fn get(&self, url: &str) -> Result<String, ureq::Error> {
        let mut response = ureq::get(url).call()?;
        response.body_mut().read_to_string()
    }
    fn get_with_accept(&self, url: &str, accept: &str) -> Result<String, ureq::Error> {
        let mut response = ureq::get(url).header("Accept", accept).call()?;
        response.body_mut().read_to_string()
    }
}

#[derive(Debug)]
pub struct UreqClientMock {
    pub mock_post: Option<HashMap<String, String>>,
    pub mock_get: Option<HashMap<String, String>>,
}

impl UreqClientMock {
    /// Helper to find a matching response based on URL prefix
    fn find_response(map: &Option<HashMap<String, String>>, url: &str) -> Option<String> {
        if let Some(responses) = map {
            for (key, value) in responses {
                if url.starts_with(key) {
                    return Some(value.clone());
                }
            }
        }
        None
    }
}

impl UreqClient for UreqClientMock {
    fn post(&self, url: &str, _body: &str) -> Result<String, ureq::Error> {
        Self::find_response(&self.mock_post, url).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("No mock response for POST {}", url),
            )
            .into()
        })
    }
    fn get(&self, url: &str) -> Result<String, ureq::Error> {
        // like a server without the resource
        Self::find_response(&self.mock_get, url).ok_or(ureq::Error::StatusCode(404))
    }
    /// Answers from `mock_get` under the key `git+<url>`; git is never run. Without a response, fails as git does for a missing repository.
    fn git_ls_remote_tags(&self, url: &str, _refs_only: bool) -> Result<String, String> {
        Self::find_response(&self.mock_get, &format!("git+{url}"))
            .ok_or_else(|| "remote: Repository not found.".to_string())
    }
}
