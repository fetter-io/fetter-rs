use ureq;

pub trait UreqClient {
    /// A post request to the given URL with the provided JSON body.
    fn post(&self, url: &str, body: &str) -> Result<String, ureq::Error>;
    /// A get request
    fn get(&self, url: &str) -> Result<String, ureq::Error>;
}

pub struct UreqClientLive;

impl UreqClient for UreqClientLive {
    fn post(&self, url: &str, body: &str) -> Result<String, ureq::Error> {
        let response = ureq::post(url)
            .set("Content-Type", "application/json")
            .send_string(body)?;
        Ok(response.into_string()?)
    }
    fn get(&self, url: &str) -> Result<String, ureq::Error> {
        let response = ureq::get(url).call()?;
        Ok(response.into_string()?)
    }
}

pub struct UreqClientMock {
    pub mock_post: Option<String>,
    pub mock_get: Option<String>,
}

impl UreqClient for UreqClientMock {
    fn post(&self, _url: &str, _body: &str) -> Result<String, ureq::Error> {
        match &self.mock_post {
            Some(mock_post) => Ok(mock_post.clone()),
            None => Ok("".to_string()),
        }
    }
    fn get(&self, _url: &str) -> Result<String, ureq::Error> {
        match &self.mock_get {
            Some(mock_get) => Ok(mock_get.clone()),
            None => Ok("".to_string()),
        }
    }
}
