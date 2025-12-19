#![allow(clippy::result_large_err)]

pub trait UreqClient: Send + Sync {
    /// A post request to the given URL with the provided JSON body.
    fn post(&self, url: &str, body: &str) -> Result<String, ureq::Error>;
    /// A get request
    fn get(&self, url: &str) -> Result<String, ureq::Error>;
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
}

#[cfg(test)]
use std::collections::HashMap;

#[cfg(test)]
#[derive(Debug)]
pub struct UreqClientMock {
    pub mock_post: Option<HashMap<String, String>>,
    pub mock_get: Option<HashMap<String, String>>,
}

#[cfg(test)]
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

#[cfg(test)]
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
        Self::find_response(&self.mock_get, url).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("No mock response for GET {}", url),
            )
            .into()
        })
    }
}
