use std::{collections::HashMap, str::FromStr};

use hyper::{
    Body, Request, Method, Response, http::HeaderName,
};
use regex::Regex;
use serde_json::Value;
use tokio::fs;
use serde::{Deserialize, Serialize};


#[derive(Serialize, Deserialize, Debug)]
pub struct Mock {
    host: String,
    method: String,
    match_uri_regex: String, 
    status: u16,
    mocked_response: HashMap<String, Value>,
    headers: HashMap<String, String> 
}

impl Mock {
    pub fn request_match(&self, req: &Request<Body>) -> bool {
        let mock_method = match Method::from_str(&self.method) {
            Ok(mock_method) => mock_method,
            Err(err) => {
                log::error!("Unable to parse regex for mock {:?}, cause {}", self, err);
                return false
            },
        };

        if *req.method() != mock_method {
            return false
        };

        let reg = match Regex::new(&self.match_uri_regex) {
            Ok(reg) => reg,
            Err(err) => {
                log::error!("Unable to parse regex for mock {:?}, cause: {}", self, err);
                return false
            },
        };

        //FIXME this regex doesn't work...
        if !reg.is_match(req.uri().path()) {
            return false;
        }

        true
    }

    pub fn construct_response_from_mock(&self) -> Response<Body> {
        let status = self.status;
        
        //for now, lets crash app when unable to serialize
        let response_body = serde_json::to_string(&self.mocked_response).unwrap();

        let mut response = Response::builder().status(status).body(response_body.into()).unwrap();
        for header in &self.headers {
            let header_key = HeaderName::from_str(&header.0).unwrap();
            let header_val = header.1.parse().unwrap();
            response.headers_mut().append(header_key, header_val);
        }

        response
    }
}

pub async fn parse_mocks(file: &str) -> Option<Vec<Mock>> {
    match read_mocks(file).await {
        Some(mocks_as_string) => {
            let deserialized: Vec<Mock> = serde_json::from_str(&mocks_as_string).unwrap();
            log::info!("deserialized mocks = {:?}", deserialized);
            Some(deserialized)
        },
        None => None,
    }
}

async fn read_mocks(file: &str) -> Option<String> {
    match fs::read_to_string(file).await {
        Ok(content) => Some(content),
        Err(err) => {
            log::error!("unable to read {} file - no mocks will be used this session ! cause {}", file, err);
            None
        },
    }
}
