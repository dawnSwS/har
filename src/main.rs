use base64::prelude::*;
use clap::Parser;
use serde_json::Value;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use url::Url;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    input: PathBuf,
    output: PathBuf,
    #[arg(short = 's', long)]
    static_dir: Option<PathBuf>,
}

#[derive(Default)]
struct ExportCounts {
    api: usize,
    static_resources: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let har_data =
        fs::read_to_string(&args.input).map_err(|error| format!("无法读取 HAR 文件: {error}"))?;
    let har: Value = serde_json::from_str(&har_data).map_err(|_| "HAR 文件不是有效的 JSON 格式")?;
    let entries = har
        .get("log")
        .and_then(|log| log.get("entries"))
        .and_then(Value::as_array)
        .ok_or("无效的 HAR 结构：找不到 log.entries")?;

    fs::create_dir_all(&args.output)?;
    if let Some(directory) = &args.static_dir {
        fs::create_dir_all(directory)?;
    }
    let counts = export_entries(entries, &args)?;
    print_summary(entries.len(), &counts, &args);
    Ok(())
}

fn export_entries(entries: &[Value], args: &Args) -> Result<ExportCounts> {
    let mut counts = ExportCounts::default();
    for (index, entry) in entries.iter().enumerate() {
        if is_static_resource(entry) {
            if let Some(directory) = &args.static_dir {
                if save_static_resource(entry, index, directory)? {
                    counts.static_resources += 1;
                }
            }
        } else {
            save_api_entry(entry, index, &args.output)?;
            counts.api += 1;
        }
    }
    Ok(counts)
}

fn text(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).unwrap_or("")
}

fn request_url(entry: &Value) -> &str {
    text(entry.get("request").and_then(|request| request.get("url")))
}

fn response_status(entry: &Value) -> Option<u64> {
    entry
        .get("response")
        .and_then(|response| response.get("status"))
        .and_then(Value::as_u64)
}

fn has_websocket_upgrade(message: Option<&Value>) -> bool {
    let Some(headers) = message
        .and_then(|message| message.get("headers"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    headers.iter().any(|header| {
        text(header.get("name")).eq_ignore_ascii_case("upgrade")
            && text(header.get("value"))
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("websocket"))
    })
}

fn is_websocket(entry: &Value) -> bool {
    if entry.get("_webSocketMessages").is_some()
        || text(entry.get("_resourceType")).eq_ignore_ascii_case("websocket")
    {
        return true;
    }
    let websocket_url =
        Url::parse(request_url(entry)).is_ok_and(|url| matches!(url.scheme(), "ws" | "wss"));
    websocket_url
        || has_websocket_upgrade(entry.get("request"))
        || has_websocket_upgrade(entry.get("response"))
}

fn is_static_resource(entry: &Value) -> bool {
    // Upgrade responses and WebSocket exchanges belong to the API export even
    // when a HAR exporter attaches a static MIME type to their handshake.
    if is_websocket(entry) || response_status(entry) == Some(101) {
        return false;
    }
    let mime_type = text(
        entry
            .get("response")
            .and_then(|response| response.get("content"))
            .and_then(|content| content.get("mimeType")),
    )
    .to_ascii_lowercase();
    ["image", "css", "javascript", "font"]
        .iter()
        .any(|extension| mime_type.contains(extension))
}

fn sanitize_filename(name: &str) -> String {
    name.replace(|character| "\\/*?:\"<>|".contains(character), "_")
}

fn static_filename(url: &str, index: usize) -> String {
    let filename = Url::parse(url).map_or_else(
        |_| "unknown".to_owned(),
        |url| sanitize_filename(url.path().rsplit('/').next().unwrap_or("unknown")),
    );
    let filename = if filename.is_empty() {
        "index".to_owned()
    } else {
        filename
    };
    let filename: String = filename.chars().take(100).collect();
    format!("{index:03}_{filename}")
}

fn save_static_resource(entry: &Value, index: usize, directory: &Path) -> Result<bool> {
    let Some(content) = entry
        .get("response")
        .and_then(|response| response.get("content"))
    else {
        return Ok(false);
    };
    let Some(raw_text) = content.get("text").and_then(Value::as_str) else {
        return Ok(false);
    };
    let data = if text(content.get("encoding")) == "base64" {
        BASE64_STANDARD.decode(raw_text)?
    } else {
        raw_text.as_bytes().to_vec()
    };
    fs::write(
        directory.join(static_filename(request_url(entry), index)),
        data,
    )?;
    Ok(true)
}

fn api_filename(entry: &Value, index: usize) -> String {
    let safe_path = Url::parse(request_url(entry)).map_or_else(
        |_| "invalid_url".to_owned(),
        |url| {
            let path = url.path().trim_matches('/');
            if path.is_empty() {
                sanitize_filename(url.host_str().unwrap_or("unknown_host"))
            } else {
                sanitize_filename(path)
            }
        },
    );
    let safe_path: String = safe_path.chars().take(50).collect();
    let method = request_method(entry);
    let safe_method = sanitize_filename(method);
    format!("{index:03}_{safe_method}_{safe_path}.txt")
}

fn request_method(entry: &Value) -> &str {
    entry
        .get("request")
        .and_then(|request| request.get("method"))
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN")
}

fn save_api_entry(entry: &Value, index: usize, directory: &Path) -> Result<()> {
    let file = File::create(directory.join(api_filename(entry, index)))?;
    let mut writer = BufWriter::new(file);
    write_request(&mut writer, entry)?;
    write_response(&mut writer, entry)?;
    if is_websocket(entry) {
        write_websocket(&mut writer, entry)?;
    }
    writer.flush()?;
    Ok(())
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

fn write_request(writer: &mut impl Write, entry: &Value) -> io::Result<()> {
    let request = entry.get("request");
    writeln!(writer, "================ REQUEST ================")?;
    writeln!(writer, "Method: {}", request_method(entry))?;
    writeln!(writer, "URL: {}\n", request_url(entry))?;
    writeln!(writer, "--- Request Headers ---")?;
    write_headers(writer, request)?;
    if let Some(body) = request
        .and_then(|request| request.get("postData"))
        .and_then(|post_data| post_data.get("text"))
        .and_then(Value::as_str)
    {
        writeln!(writer, "--- Request Payload / Body ---")?;
        writeln!(writer, "{body}\n")?;
    }
    Ok(())
}

fn write_response(writer: &mut impl Write, entry: &Value) -> Result<()> {
    let response = entry.get("response");
    let status = response_status(entry)
        .filter(|status| *status > 0)
        .map_or_else(|| "Unknown".to_owned(), |status| status.to_string());
    writeln!(writer, "================ RESPONSE ================")?;
    writeln!(writer, "Status: {status}\n")?;
    writeln!(writer, "--- Response Headers ---")?;
    write_headers(writer, response)?;
    if let Some(content) = response.and_then(|response| response.get("content")) {
        write_response_body(writer, content)?;
    }
    Ok(())
}

fn write_response_body(writer: &mut impl Write, content: &Value) -> Result<()> {
    let Some(raw_text) = content.get("text").and_then(Value::as_str) else {
        return Ok(());
    };
    writeln!(writer, "--- Response Body ---")?;
    let processed_text = if text(content.get("encoding")) == "base64" {
        decode_response_text(raw_text)
    } else {
        raw_text.to_owned()
    };
    match serde_json::from_str::<Value>(&processed_text) {
        Ok(body) => {
            serde_json::to_writer_pretty(&mut *writer, &body)?;
            writeln!(writer)?;
        }
        Err(_) => writeln!(writer, "{processed_text}")?,
    }
    Ok(())
}

fn decode_response_text(raw_text: &str) -> String {
    match BASE64_STANDARD.decode(raw_text) {
        Ok(bytes) => String::from_utf8(bytes)
            .unwrap_or_else(|_| "[警告: 响应体是无法解析为文本的二进制数据，已过滤]".to_owned()),
        Err(_) => "[错误: Base64 解码失败或格式异常]".to_owned(),
    }
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
        // Keep the original JSON instead of coercing payloads or guessing frame
        // fields. This preserves direction, timestamp, opcode, binary Base64,
        // extension metadata, and message order exactly as supplied by the HAR.
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
    let metadata: serde_json::Map<String, Value> = fields
        .iter()
        .filter(|(name, _)| name.starts_with('_') && *name != "_webSocketMessages")
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    if !metadata.is_empty() {
        writeln!(writer, "--- WebSocket HAR Extension Metadata ---")?;
        serde_json::to_writer_pretty(&mut *writer, &metadata)?;
        writeln!(writer)?;
    }
    Ok(())
}

fn print_summary(total: usize, counts: &ExportCounts, args: &Args) {
    println!("处理完成！从 HAR 文件中提取了 {total} 个条目。");
    if let Some(directory) = &args.static_dir {
        println!(
            "成功保存了 {} 个 API 接口文件到 '{}' 目录。",
            counts.api,
            args.output.display()
        );
        println!(
            "成功导出了 {} 个静态资源到 '{}' 目录。",
            counts.static_resources,
            directory.display()
        );
    } else {
        println!(
            "已过滤静态资源，成功保存了 {} 个 API 接口文件到 '{}' 目录。",
            counts.api,
            args.output.display()
        );
    }
}
