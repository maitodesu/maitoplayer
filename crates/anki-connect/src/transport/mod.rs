use std::{
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    time::Duration,
};

use contracts::{AppErrorV1, error_codes};
use serde_json::{Value, json};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 32 * 1024;

pub trait AnkiApi: Send + Sync {
    fn invoke(&self, action: &str, params: Value) -> Result<Value, AppErrorV1>;

    fn invoke_retryable(&self, action: &str, params: Value) -> Result<Value, AppErrorV1> {
        self.invoke(action, params)
    }
}

#[derive(Clone, Debug)]
pub struct HttpAnkiTransport {
    address: SocketAddr,
    timeout: Duration,
    api_version: u8,
}

impl Default for HttpAnkiTransport {
    fn default() -> Self {
        Self {
            address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8765),
            timeout: Duration::from_secs(5),
            api_version: 6,
        }
    }
}

impl HttpAnkiTransport {
    pub fn loopback(port: u16, timeout: Duration) -> Self {
        Self {
            address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            timeout,
            api_version: 6,
        }
    }

    pub fn handshake(&self) -> Result<u64, AppErrorV1> {
        self.invoke("version", json!({}))?
            .as_u64()
            .ok_or_else(|| response_error("AnkiConnect version was not an integer."))
    }

    pub fn require_compatible_version(&self, minimum: u64) -> Result<u64, AppErrorV1> {
        let version = self.handshake()?;
        if version < minimum {
            return Err(response_error(format!(
                "AnkiConnect API version {version} is older than required version {minimum}."
            )));
        }
        Ok(version)
    }
}

impl AnkiApi for HttpAnkiTransport {
    fn invoke(&self, action: &str, params: Value) -> Result<Value, AppErrorV1> {
        if action.is_empty()
            || action.len() > 128
            || !action.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(response_error("AnkiConnect action was invalid."));
        }
        let body = serde_json::to_vec(&json!({
            "action": action,
            "version": self.api_version,
            "params": params,
        }))
        .map_err(response_error)?;
        if body.len() > 64 * 1024 * 1024 {
            return Err(response_error(
                "AnkiConnect request exceeded its safety limit.",
            ));
        }
        let mut stream =
            TcpStream::connect_timeout(&self.address, self.timeout).map_err(offline_error)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .map_err(offline_error)?;
        stream
            .set_write_timeout(Some(self.timeout))
            .map_err(offline_error)?;
        let header = format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.address.port(),
            body.len()
        );
        stream
            .write_all(header.as_bytes())
            .and_then(|()| stream.write_all(&body))
            .map_err(offline_error)?;
        let mut response = Vec::with_capacity(64 * 1024);
        stream
            .take((MAX_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut response)
            .map_err(offline_error)?;
        if response.len() > MAX_RESPONSE_BYTES {
            return Err(response_error(
                "AnkiConnect response exceeded its safety limit.",
            ));
        }
        parse_http_response(&response)
    }

    fn invoke_retryable(&self, action: &str, params: Value) -> Result<Value, AppErrorV1> {
        if !is_read_only_action(action) {
            return Err(response_error(
                "Automatic retry was requested for a mutating AnkiConnect action.",
            ));
        }
        match self.invoke(action, params.clone()) {
            Err(error) if error.code == error_codes::ANKI_OFFLINE => self.invoke(action, params),
            result => result,
        }
    }
}

fn parse_http_response(response: &[u8]) -> Result<Value, AppErrorV1> {
    let separator = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| response_error("Malformed HTTP response."))?;
    if separator > MAX_HEADER_BYTES {
        return Err(response_error("HTTP headers exceeded their safety limit."));
    }
    let header = std::str::from_utf8(&response[..separator]).map_err(response_error)?;
    if header.bytes().any(|byte| byte == 0) {
        return Err(response_error("HTTP headers contained a null byte."));
    }
    let mut status_parts = header
        .lines()
        .next()
        .ok_or_else(|| response_error("HTTP status was missing."))?
        .split_whitespace();
    let protocol = status_parts
        .next()
        .ok_or_else(|| response_error("HTTP protocol was missing."))?;
    if !matches!(protocol, "HTTP/1.0" | "HTTP/1.1") {
        return Err(response_error("HTTP protocol was invalid."));
    }
    let status = status_parts
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| response_error("HTTP status was invalid."))?;
    if !(200..300).contains(&status) {
        let detail = format!("AnkiConnect returned HTTP {status}.");
        return if status >= 500 {
            Err(offline_error(detail))
        } else {
            Err(response_error(detail))
        };
    }
    let body = &response[(separator + 4)..];
    validate_http_body_headers(header, body.len())?;
    let envelope: Value = serde_json::from_slice(body).map_err(response_error)?;
    let object = envelope
        .as_object()
        .ok_or_else(|| response_error("AnkiConnect envelope was not an object."))?;
    if object.len() != 2 || !object.contains_key("result") || !object.contains_key("error") {
        return Err(response_error(
            "AnkiConnect envelope had an unexpected shape.",
        ));
    }
    if !object["error"].is_null() {
        let safe = object["error"]
            .as_str()
            .unwrap_or("AnkiConnect reported an error.");
        return Err(AppErrorV1::new(
            error_codes::ANKI_SCHEMA_MISMATCH,
            "Anki rejected the request. Check the selected deck, note type, and field mapping.",
            true,
        )
        .with_diagnostics(safe.chars().take(1_024).collect::<String>()));
    }
    Ok(object["result"].clone())
}

fn validate_http_body_headers(header: &str, actual_body_len: usize) -> Result<(), AppErrorV1> {
    let mut content_length = None;
    for line in header.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            return Err(response_error("An HTTP header line was malformed."));
        };
        if name.eq_ignore_ascii_case("transfer-encoding") && !value.trim().is_empty() {
            return Err(response_error(
                "Transfer-encoded AnkiConnect responses are not supported.",
            ));
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(response_error("HTTP response repeated Content-Length."));
            }
            content_length = Some(value.trim().parse::<usize>().map_err(response_error)?);
        }
    }
    if let Some(expected) = content_length
        && expected != actual_body_len
    {
        return Err(response_error(
            "HTTP response body length did not match Content-Length.",
        ));
    }
    Ok(())
}

fn is_read_only_action(action: &str) -> bool {
    matches!(
        action,
        "version" | "deckNames" | "modelNames" | "modelFieldNames" | "findNotes" | "notesInfo"
    )
}

fn offline_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::ANKI_OFFLINE,
        "Start Anki Desktop, confirm AnkiConnect is installed, then retry.",
        true,
    )
    .with_diagnostics(error.to_string())
}

fn response_error(error: impl std::fmt::Display) -> AppErrorV1 {
    AppErrorV1::new(
        error_codes::ANKI_SCHEMA_MISMATCH,
        "AnkiConnect returned an invalid response. Check its version and configuration.",
        false,
    )
    .with_diagnostics(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_success_and_api_error() -> Result<(), AppErrorV1> {
        let success = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"result\":6,\"error\":null}";
        assert_eq!(parse_http_response(success)?, json!(6));
        let error = b"HTTP/1.1 200 OK\r\n\r\n{\"result\":null,\"error\":\"bad model\"}";
        let result = parse_http_response(error);
        assert!(matches!(
            result,
            Err(AppErrorV1 { ref code, .. }) if code == error_codes::ANKI_SCHEMA_MISMATCH
        ));
        Ok(())
    }

    #[test]
    fn rejects_extra_envelope_fields() {
        let response = b"HTTP/1.1 200 OK\r\n\r\n{\"result\":6,\"error\":null,\"extra\":true}";
        assert!(parse_http_response(response).is_err());
    }

    #[test]
    fn rejects_ambiguous_or_transfer_encoded_http_bodies() {
        let wrong_length =
            b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n{\"result\":6,\"error\":null}";
        assert!(parse_http_response(wrong_length).is_err());
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\n0\r\n";
        assert!(parse_http_response(chunked).is_err());
    }
}
