//! ADR-0046 §3.3：在 HTTP 边界转换头部，保留端到端字段的重复值顺序。

use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use std::collections::HashSet;

fn hop(name: &str, connection: &HashSet<String>) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    ) || connection.contains(name)
}

pub(super) fn request(headers: &HeaderMap) -> Result<Vec<(String, String)>, StatusCode> {
    let values = headers
        .iter()
        .map(|(name, value)| {
            std::str::from_utf8(value.as_bytes())
                .map(|value| (name.as_str().to_owned(), value.to_owned()))
                .map_err(|_| StatusCode::BAD_REQUEST)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let connection = nominated(&values);
    Ok(values
        .into_iter()
        .filter(|(name, _)| !hop(name, &connection))
        .collect())
}

pub(super) fn response(values: Vec<(String, String)>) -> Result<HeaderMap, StatusCode> {
    let connection = nominated(&values);
    let mut headers = HeaderMap::new();
    for (name, value) in values {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| StatusCode::BAD_GATEWAY)?;
        if name == header::CONTENT_LENGTH || hop(name.as_str(), &connection) {
            continue;
        }
        let value =
            HeaderValue::from_bytes(value.as_bytes()).map_err(|_| StatusCode::BAD_GATEWAY)?;
        headers.append(name, value);
    }
    Ok(headers)
}

fn nominated(values: &[(String, String)]) -> HashSet<String> {
    values
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| {
            value
                .split(',')
                .map(|name| name.trim().to_ascii_lowercase())
        })
        .collect()
}
