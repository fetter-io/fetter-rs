#![allow(clippy::result_large_err)]

use std::collections::HashMap;

pub trait UreqClient: Send + Sync {
    /// A post request to the given URL with the provided JSON body.
    fn post(&self, url: &str, body: &str) -> Result<String, ureq::Error>;
    /// A get request
    fn get(&self, url: &str) -> Result<String, ureq::Error>;
    /// A get request with an `Accept` header (e.g. npm's abbreviated metadata). By default the header is ignored.
    fn get_with_accept(&self, url: &str, _accept: &str) -> Result<String, ureq::Error> {
        self.get(url)
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
}
