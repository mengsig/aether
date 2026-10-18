//! Validate signature headers before reading any request body.

use hyper::HeaderMap;

use super::signature::{self, Stamp};

pub(super) struct SignedHeaders {
    pub stamp: Stamp,
    pub signature: String,
    pub delivery_id: Option<String>,
}

impl SignedHeaders {
    pub fn parse(headers: &HeaderMap) -> Result<Self, String> {
        let required = [
            signature::HEADER,
            signature::HEADER_TIMESTAMP,
            signature::HEADER_NONCE,
        ];
        let missing: Vec<_> = required
            .into_iter()
            .filter(|name| !headers.contains_key(*name))
            .collect();
        if !missing.is_empty() {
            return Err(format!("missing required headers: {}", missing.join(", ")));
        }
        for name in required.into_iter().chain([signature::HEADER_DELIVERY_ID]) {
            if headers.get_all(name).iter().count() > 1 {
                return Err(format!("duplicate header: {name}; send exactly one value"));
            }
        }
        let timestamp = value(headers, signature::HEADER_TIMESTAMP)?;
        let nonce = value(headers, signature::HEADER_NONCE)?;
        let stamp = Stamp::parse(timestamp, nonce)?;
        let signature = value(headers, signature::HEADER)?.to_owned();
        let delivery_id = if headers.contains_key(signature::HEADER_DELIVERY_ID) {
            let id = value(headers, signature::HEADER_DELIVERY_ID)?;
            if id.len() > 128
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err(format!(
                    "{} must be 1 to 128 letters, digits, - or _",
                    signature::HEADER_DELIVERY_ID
                ));
            }
            Some(id.to_owned())
        } else {
            None
        };
        Ok(Self {
            stamp,
            signature,
            delivery_id,
        })
    }
}

fn value<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, String> {
    let value = headers
        .get(name)
        .ok_or_else(|| format!("missing required header: {name}"))?;
    let text = value
        .to_str()
        .map_err(|_| format!("{name} must contain ASCII text"))?
        .trim();
    if text.is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    Ok(text)
}
