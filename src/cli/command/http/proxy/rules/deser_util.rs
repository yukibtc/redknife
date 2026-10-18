use std::collections::HashMap;
use std::str::FromStr;

use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use http::uri::Authority;
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use serde::{Deserialize, Deserializer, de};

pub(super) fn deserialize_status_code<'de, D>(deserializer: D) -> Result<StatusCode, D::Error>
where
    D: Deserializer<'de>,
{
    let status_code: u16 = Deserialize::deserialize(deserializer)?;
    StatusCode::from_u16(status_code).map_err(de::Error::custom)
}

pub(super) fn deserialize_optional_authority<'de, D>(
    deserializer: D,
) -> Result<Option<Authority>, D::Error>
where
    D: Deserializer<'de>,
{
    let authority: Option<String> = Deserialize::deserialize(deserializer)?;
    match authority {
        Some(authority) => Ok(Some(authority.parse().map_err(de::Error::custom)?)),
        None => Ok(None),
    }
}

pub(super) fn deserialize_optional_method<'de, D>(
    deserializer: D,
) -> Result<Option<Method>, D::Error>
where
    D: Deserializer<'de>,
{
    let method: Option<String> = Deserialize::deserialize(deserializer)?;
    match method {
        Some(method) => Ok(Some(Method::from_str(&method).map_err(de::Error::custom)?)),
        None => Ok(None),
    }
}

pub(super) fn deserialize_optional_header_map<'de, D>(
    deserializer: D,
) -> Result<Option<HeaderMap>, D::Error>
where
    D: Deserializer<'de>,
{
    let map: Option<HashMap<String, String>> = Deserialize::deserialize(deserializer)?;

    match map {
        Some(map) => {
            let mut header_map: HeaderMap = HeaderMap::with_capacity(map.len());

            for (key, value) in map {
                let key: HeaderName = key.parse().map_err(de::Error::custom)?;
                let value: HeaderValue = value.parse().map_err(de::Error::custom)?;

                header_map.insert(key, value);
            }

            Ok(Some(header_map))
        }
        None => Ok(None),
    }
}

pub(super) fn deserialize_header_names<'de, D>(deserializer: D) -> Result<Vec<HeaderName>, D::Error>
where
    D: Deserializer<'de>,
{
    let names: Option<Vec<String>> = Deserialize::deserialize(deserializer)?;

    match names {
        Some(names) => {
            let mut header_names: Vec<HeaderName> = Vec::with_capacity(names.len());

            for name in names {
                let name: HeaderName = name.parse().map_err(de::Error::custom)?;
                header_names.push(name);
            }

            Ok(header_names)
        }
        None => Ok(Vec::new()),
    }
}

pub(super) fn deserialize_base64<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let base64: String = Deserialize::deserialize(deserializer)?;
    BASE64_STANDARD.decode(base64).map_err(de::Error::custom)
}
