// Copyright (c) 2026 Randi Apriansyah
// SPDX-License-Identifier: MIT

//! AWS Signature Version 4 request signing, implemented directly on `hmac`/`sha2` so this crate
//! depends on no AWS SDK or crate. Only the `Authorization` header flavour is supported (what
//! RustFS's S3 Tables catalog expects).

use std::fmt::Write;

use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, Url};
use sha2::{Digest, Sha256};

const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// A request whose body is a stream: its bytes aren't known up front, so it can't be hashed.
#[derive(Debug, thiserror::Error)]
#[error("a streaming request body can't be SigV4-signed")]
pub struct StreamingBody;

#[derive(Clone)]
pub struct Signer {
    access_key_id: String,
    secret_access_key: String,
    region: String,
    service: String,
}

impl Signer {
    pub fn new(
        access_key_id: String,
        secret_access_key: String,
        region: String,
        service: String,
    ) -> Self {
        Self {
            access_key_id,
            secret_access_key,
            region,
            service,
        }
    }

    /// Signs a request in place: sets `x-amz-content-sha256`, `x-amz-date` and `authorization`
    /// for `method` on `url` with `body`, replacing any `x-amz-*` or `authorization` header
    /// already in `headers`. `host` is signed but not set: the HTTP client sends the same value,
    /// derived from `url`.
    pub fn sign(
        &self,
        method: &Method,
        url: &Url,
        headers: &mut HeaderMap,
        body: &[u8],
        now: DateTime<Utc>,
    ) {
        let host = match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (host, None) => host.unwrap_or_default().to_string(),
            (None, Some(_)) => String::new(),
        };
        let date = amz_date(now);
        let payload_hash = sha256_hex(body);
        let authorization = self.authorization(
            method.as_str(),
            url.path(),
            url.query().unwrap_or_default(),
            &[
                ("host", &host),
                ("x-amz-content-sha256", &payload_hash),
                ("x-amz-date", &date),
            ],
            &payload_hash,
            now,
        );

        let stale: Vec<HeaderName> = headers
            .keys()
            .filter(|name| name.as_str().starts_with("x-amz-") || **name == AUTHORIZATION)
            .cloned()
            .collect();
        for name in stale {
            headers.remove(name);
        }
        for (name, value) in [
            ("x-amz-content-sha256", payload_hash),
            ("x-amz-date", date),
            (AUTHORIZATION.as_str(), authorization),
        ] {
            headers.insert(
                HeaderName::from_static(name),
                HeaderValue::try_from(value).expect("signing output is valid header text"),
            );
        }
    }

    /// [`Self::sign`] for a built `reqwest` request, at the current time.
    pub fn sign_request(&self, request: &mut reqwest::Request) -> Result<(), StreamingBody> {
        let body = match request.body() {
            None => Vec::new(),
            Some(body) => body.as_bytes().ok_or(StreamingBody)?.to_vec(),
        };
        let (method, url) = (request.method().clone(), request.url().clone());
        self.sign(&method, &url, request.headers_mut(), &body, Utc::now());
        Ok(())
    }

    /// Builds the `Authorization` header value for a request. `headers` are exactly the headers
    /// to sign (at least `host` and `x-amz-date`, plus `x-amz-content-sha256` for S3) and must
    /// be sent unchanged; `payload_hash` is the lowercase hex SHA-256 of the body.
    pub fn authorization(
        &self,
        method: &str,
        path: &str,
        query: &str,
        headers: &[(&str, &str)],
        payload_hash: &str,
        now: DateTime<Utc>,
    ) -> String {
        let mut headers: Vec<(String, &str)> = headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim()))
            .collect();
        headers.sort();

        let canonical_headers: String = headers
            .iter()
            .map(|(name, value)| format!("{name}:{value}\n"))
            .collect();
        let signed_headers = headers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(";");
        let canonical_request = format!(
            "{method}\n{}\n{}\n{canonical_headers}\n{signed_headers}\n{payload_hash}",
            canonical_path(path),
            canonical_query(query),
        );

        let date = now.format("%Y%m%d");
        let scope = format!("{date}/{}/{}/aws4_request", self.region, self.service);
        let string_to_sign = format!(
            "{ALGORITHM}\n{}\n{scope}\n{}",
            amz_date(now),
            hex(&Sha256::digest(canonical_request.as_bytes())),
        );

        let mut key = hmac(
            format!("AWS4{}", self.secret_access_key).as_bytes(),
            date.to_string().as_bytes(),
        );
        for part in [&self.region, &self.service, "aws4_request"] {
            key = hmac(&key, part.as_bytes());
        }
        let signature = hex(&hmac(&key, string_to_sign.as_bytes()));

        format!(
            "{ALGORITHM} Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.access_key_id
        )
    }
}

/// Leaves the secret out: [`Signer`]s end up inside `Debug` types (the REST catalog's auth
/// manager, the catalog config).
impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signer")
            .field("access_key_id", &self.access_key_id)
            .field("region", &self.region)
            .field("service", &self.service)
            .finish_non_exhaustive()
    }
}

/// `x-amz-date` timestamp format (`20150830T123600Z`).
pub fn amz_date(now: DateTime<Utc>) -> String {
    now.format("%Y%m%dT%H%M%SZ").to_string()
}

/// Lowercase hex SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Percent-decodes then re-encodes each path segment exactly once (S3 style: no double
/// encoding), keeping `/`.
fn canonical_path(path: &str) -> String {
    if path.is_empty() {
        return "/".to_string();
    }
    encode(&decode(path), true)
}

/// Decodes, re-encodes and sorts the query pairs by name, then value.
fn canonical_query(query: &str) -> String {
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (encode(&decode(name), false), encode(&decode(value), false))
        })
        .collect();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn decode(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|digits| u8::from_str_radix(digits, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    out
}

fn encode(bytes: &[u8], keep_slash: bool) -> String {
    bytes.iter().fold(String::new(), |mut out, &byte| {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || (keep_slash && byte == b'/')
        {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    /// The `get-vanilla` case from AWS's published SigV4 test suite.
    #[test]
    fn matches_the_published_get_vanilla_vector() {
        let signer = Signer::new(
            "AKIDEXAMPLE".to_string(),
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            "us-east-1".to_string(),
            "service".to_string(),
        );
        let now = Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap();

        let authorization = signer.authorization(
            "GET",
            "/",
            "",
            &[
                ("Host", "example.amazonaws.com"),
                ("X-Amz-Date", "20150830T123600Z"),
            ],
            &sha256_hex(b""),
            now,
        );

        assert_eq!(
            authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn sign_sets_the_signed_headers_and_replaces_stale_ones() {
        let signer = Signer::new(
            "AKIDEXAMPLE".to_string(),
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            "us-east-1".to_string(),
            "s3".to_string(),
        );
        let now = Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap();
        let url = Url::parse("http://rustfs:9000/iceberg/v1/config?warehouse=pipa").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-amz-security-token", HeaderValue::from_static("stale"));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer stale"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        signer.sign(&Method::POST, &url, &mut headers, b"{}", now);

        let expected = signer.authorization(
            "POST",
            "/iceberg/v1/config",
            "warehouse=pipa",
            &[
                ("host", "rustfs:9000"),
                ("x-amz-content-sha256", &sha256_hex(b"{}")),
                ("x-amz-date", "20150830T123600Z"),
            ],
            &sha256_hex(b"{}"),
            now,
        );
        assert_eq!(headers[AUTHORIZATION], expected.as_str());
        assert_eq!(headers["x-amz-date"], "20150830T123600Z");
        assert_eq!(headers["x-amz-content-sha256"], sha256_hex(b"{}").as_str());
        assert_eq!(headers["content-type"], "application/json");
        assert!(!headers.contains_key("x-amz-security-token"));
        assert!(!format!("{signer:?}").contains("EXAMPLEKEY"));
    }

    #[test]
    fn canonicalizes_paths_and_queries_once() {
        assert_eq!(canonical_path(""), "/");
        assert_eq!(
            canonical_path("/iceberg/v1/pipa/namespaces/a%1Fb"),
            "/iceberg/v1/pipa/namespaces/a%1Fb"
        );
        assert_eq!(canonical_path("/a b/%7Euser"), "/a%20b/~user");
        assert_eq!(
            canonical_query("warehouse=pipa&Zed=1&a=%2f&a=b c"),
            "Zed=1&a=%2F&a=b%20c&warehouse=pipa"
        );
    }
}
