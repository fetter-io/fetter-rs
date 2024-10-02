use ureq;

pub trait UreqClient {
    /// A post request to the given URL with the provided JSON body.
    fn post(&self, url: &str, body: &str) -> Result<String, ureq::Error>;
}

pub struct UreqClientLive;

impl UreqClient for UreqClientLive {
    fn post(&self, url: &str, body: &str) -> Result<String, ureq::Error> {
        let response = ureq::post(url)
            .set("Content-Type", "application/json")
            .send_string(body)?;
        Ok(response.into_string()?)
    }
}

pub struct UreqClientMock {
    pub mock_response: String,
}

impl UreqClient for UreqClientMock {
    fn post(&self, _url: &str, _body: &str) -> Result<String, ureq::Error> {
        Ok(self.mock_response.clone())
    }
}
