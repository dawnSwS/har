use crate::classifier::{request_method, request_url, response_status, text};
use base64::prelude::*;
use serde_json::Value;
use std::collections::BTreeMap;
use std::error::Error;
use std::io::{self, Write};
use std::path::Path;
use url::Url;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

const MAX_STATIC_FILENAME_CHARS: usize = 100;
const MAX_API_PATH_CHARS: usize = 50;

pub fn index_width(total: usize) -> usize {
    total.saturating_sub(1).to_string().len().max(3)
}

pub fn sanitize_filename(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|character| {
            if character.is_control() || "\\/*?:\"<>|".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    replaced.trim_end_matches([' ', '.']).to_owned()
}

pub fn extension_for_mime(mime_type: &str) -> Option<&'static str> {
    match mime_type {
        "text/css" => Some("css"),
        "text/javascript" | "application/javascript" | "application/x-javascript" => Some("js"),
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/svg+xml" => Some("svg"),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some("ico"),
        "image/avif" => Some("avif"),
        "font/woff2" => Some("woff2"),
        "font/woff" => Some("woff"),
        "font/ttf" => Some("ttf"),
        "font/otf" => Some("otf"),
        "application/wasm" => Some("wasm"),
        "application/pdf" => Some("pdf"),
        "application/zip" => Some("zip"),
        "application/gzip" => Some("gz"),
        "video/mp4" => Some("mp4"),
        "audio/mpeg" => Some("mp3"),
        _ => None,
    }
}

fn truncate_preserving_extension(filename: &str, max_chars: usize) -> String {
    if filename.chars().count() <= max_chars {
        return filename.to_owned();
    }
    if let Some((stem, extension)) = filename.rsplit_once('.') {
        let ext_chars = extension.chars().count();
        if !stem.is_empty() && !extension.is_empty() && ext_chars + 1 < max_chars {
            let max_stem = max_chars - ext_chars - 1;
            let truncated_stem: String = stem.chars().take(max_stem).collect();
            let clean_stem = truncated_stem.trim_end_matches([' ', '.']);
            if !clean_stem.is_empty() {
                return format!("{clean_stem}.{extension}");
            }
        }
    }
    let truncated: String = filename.chars().take(max_chars).collect();
    truncated.trim_end_matches([' ', '.']).to_owned()
}

pub fn static_filename(
    parsed_url: Option<&Url>,
    mime_type: &str,
    index: usize,
    width: usize,
) -> String {
    let raw_name = parsed_url.map_or_else(
        || "unknown".to_owned(),
        |url| sanitize_filename(url.path().rsplit('/').next().unwrap_or("unknown")),
    );
    let mut filename = if raw_name.is_empty() {
        "index".to_owned()
    } else {
        raw_name
    };
    if Path::new(&filename).extension().is_none() {
        if let Some(ext) = extension_for_mime(mime_type) {
            filename.push('.');
            filename.push_str(ext);
        }
    }
    let filename = truncate_preserving_extension(&filename, MAX_STATIC_FILENAME_CHARS);
    let filename = if filename.is_empty() {
        "index".to_owned()
    } else {
        filename
    };
    format!("{index:0width$}_{filename}")
}

fn api_stem(entry: &Value, parsed_url: Option<&Url>, index: usize, width: usize) -> String {
    let safe_path = parsed_url.map_or_else(
        || "invalid_url".to_owned(),
        |url| {
            let path = url.path().trim_matches('/');
            let candidate = if path.is_empty() {
                sanitize_filename(url.host_str().unwrap_or("unknown_host"))
            } else {
                sanitize_filename(path)
            };
            if candidate.is_empty() {
                "unknown_path".to_owned()
            } else {
                candidate
            }
        },
    );
    let safe_path: String = safe_path.chars().take(MAX_API_PATH_CHARS).collect();
    let safe_path = safe_path.trim_end_matches([' ', '.']);
    let safe_path = if safe_path.is_empty() {
        "unknown_path"
    } else {
        safe_path
    };
    let safe_method = sanitize_filename(request_method(entry));
    let safe_method = if safe_method.is_empty() {
        "UNKNOWN"
    } else {
        safe_method.as_str()
    };
    format!("{index:0width$}_{safe_method}_{safe_path}")
}

pub fn api_filename(entry: &Value, parsed_url: Option<&Url>, index: usize, width: usize) -> String {
    format!("{}.txt", api_stem(entry, parsed_url, index, width))
}

pub fn binary_filename(
    entry: &Value,
    parsed_url: Option<&Url>,
    mime_type: &str,
    index: usize,
    width: usize,
) -> String {
    let ext = extension_for_mime(mime_type).unwrap_or("bin");
    format!("{}.{ext}", api_stem(entry, parsed_url, index, width))
}

pub fn decode_base64(raw_text: &str) -> std::result::Result<Vec<u8>, base64::DecodeError> {
    if raw_text.bytes().any(|byte| byte.is_ascii_whitespace()) {
        let cleaned: String = raw_text
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect();
        BASE64_STANDARD.decode(cleaned)
    } else {
        BASE64_STANDARD.decode(raw_text)
    }
}

pub fn render_api_entry(
    entry: &Value,
    websocket: bool,
    decoded_body: Option<&str>,
    binary_path: Option<&str>,
) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    write_entry_summary(&mut buffer, entry)?;
    write_request(&mut buffer, entry)?;
    write_response(&mut buffer, entry, decoded_body, binary_path)?;
    if websocket {
        write_websocket(&mut buffer, entry)?;
    }
    Ok(buffer)
}

fn write_entry_summary(writer: &mut impl Write, entry: &Value) -> io::Result<()> {
    let started = text(entry.get("startedDateTime"));
    let server_ip = text(entry.get("serverIPAddress"));
    let time = entry.get("time");
    if started.is_empty() && server_ip.is_empty() && time.is_none() {
        return Ok(());
    }
    writeln!(writer, "================ META ================")?;
    if !started.is_empty() {
        writeln!(writer, "Started: {started}")?;
    }
    if let Some(duration) = time {
        writeln!(writer, "Time(ms): {duration}")?;
    }
    if !server_ip.is_empty() {
        writeln!(writer, "Server IP: {server_ip}")?;
    }
    writeln!(writer)
}

fn write_headers(writer: &mut impl Write, message: Option<&Value>) -> io::Result<()> {
    if let Some(headers) = message
        .and_then(|message| message.get("headers"))
        .and_then(Value::as_array)
    {
        for header in headers {
            writeln!(
                writer,
                "{}: {}",
                text(header.get("name")),
                text(header.get("value"))
            )?;
        }
    }
    writeln!(writer)
}

fn write_formatted_body(writer: &mut impl Write, body: &str) -> Result<()> {
    match serde_json::from_str::<Value>(body) {
        Ok(parsed) => {
            serde_json::to_writer_pretty(&mut *writer, &parsed)?;
            writeln!(writer)?;
        }
        Err(_) => writeln!(writer, "{body}")?,
    }
    Ok(())
}

fn write_request(writer: &mut impl Write, entry: &Value) -> Result<()> {
    let request = entry.get("request");
    writeln!(writer, "================ REQUEST ================")?;
    writeln!(writer, "Method: {}", request_method(entry))?;
    writeln!(writer, "URL: {}\n", request_url(entry))?;
    writeln!(writer, "--- Request Headers ---")?;
    write_headers(writer, request)?;
    if let Some(post_data) = request.and_then(|request| request.get("postData")) {
        write_post_data(writer, post_data)?;
    }
    Ok(())
}

fn write_post_data(writer: &mut impl Write, post_data: &Value) -> Result<()> {
    if let Some(body) = post_data
        .get("text")
        .and_then(Value::as_str)
        .filter(|body| !body.is_empty())
    {
        writeln!(writer, "--- Request Payload / Body ---")?;
        write_formatted_body(writer, body)?;
        writeln!(writer)?;
        return Ok(());
    }
    if let Some(params) = post_data
        .get("params")
        .and_then(Value::as_array)
        .filter(|params| !params.is_empty())
    {
        writeln!(writer, "--- Request Payload / Params ---")?;
        for param in params {
            writeln!(
                writer,
                "{}: {}",
                text(param.get("name")),
                text(param.get("value"))
            )?;
        }
        writeln!(writer)?;
    }
    Ok(())
}

fn write_response(
    writer: &mut impl Write,
    entry: &Value,
    decoded_body: Option<&str>,
    binary_path: Option<&str>,
) -> Result<()> {
    let response = entry.get("response");
    let status = response_status(entry)
        .filter(|status| *status > 0)
        .map_or_else(|| "Unknown".to_owned(), |status| status.to_string());
    writeln!(writer, "================ RESPONSE ================")?;
    writeln!(writer, "Status: {status}\n")?;
    writeln!(writer, "--- Response Headers ---")?;
    write_headers(writer, response)?;
    if let Some(path) = binary_path {
        writeln!(writer, "--- Response Body ---")?;
        writeln!(writer, "[二进制响应体已完整保存至: {path}]")?;
    } else if let Some(body) = decoded_body {
        writeln!(writer, "--- Response Body ---")?;
        write_formatted_body(writer, body)?;
    }
    Ok(())
}

fn write_websocket(writer: &mut impl Write, entry: &Value) -> Result<()> {
    writeln!(
        writer,
        "\n================ WEBSOCKET MESSAGES ================"
    )?;
    if let Some(messages) = entry.get("_webSocketMessages") {
        writeln!(
            writer,
            "--- Captured WebSocket Data (HAR _webSocketMessages) ---"
        )?;
        serde_json::to_writer_pretty(&mut *writer, messages)?;
        writeln!(writer)?;
        if messages.as_array().is_some_and(Vec::is_empty) {
            writeln!(
                writer,
                "No WebSocket messages were captured in this HAR entry."
            )?;
        }
    } else {
        writeln!(
            writer,
            "No WebSocket messages were captured in this HAR entry."
        )?;
    }
    write_websocket_metadata(writer, entry)
}

fn write_websocket_metadata(writer: &mut impl Write, entry: &Value) -> Result<()> {
    let Some(fields) = entry.as_object() else {
        return Ok(());
    };
    let metadata: BTreeMap<&str, &Value> = fields
        .iter()
        .filter(|(name, _)| name.starts_with('_') && name.as_str() != "_webSocketMessages")
        .map(|(name, value)| (name.as_str(), value))
        .collect();
    if !metadata.is_empty() {
        writeln!(writer, "--- WebSocket HAR Extension Metadata ---")?;
        serde_json::to_writer_pretty(&mut *writer, &metadata)?;
        writeln!(writer)?;
    }
    Ok(())
}
