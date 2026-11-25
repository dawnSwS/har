use crate::classifier::{
    classify_entry, is_already_compressed, primary_mime_type, request_url, text, EntryCategory,
};
use crate::formatter::{
    api_filename, binary_filename, decode_base64, index_width, render_api_entry, static_filename,
    Result,
};
use serde_json::{json, Map, Value};
use std::borrow::Cow;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;
use url::Url;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

#[derive(Default)]
struct ExportCounts {
    api: usize,
    websocket: usize,
    static_resources: usize,
    binary: usize,
    failed: usize,
}

enum DecodedResponse<'a> {
    None,
    Text(Cow<'a, str>),
    Binary(Vec<u8>),
}

pub fn convert_har_to_zip(input: &Path, output: &Path) -> Result<()> {
    let file = File::open(input).map_err(|error| format!("无法读取 HAR 文件: {error}"))?;
    let reader = BufReader::new(file);
    let mut har: Value = serde_json::from_reader(reader)
        .map_err(|error| format!("HAR 文件不是有效的 JSON 格式: {error}"))?;

    let log_obj = har
        .get_mut("log")
        .and_then(Value::as_object_mut)
        .ok_or("无效的 HAR 结构：找不到 log 对象")?;
    let Some(Value::Array(entries)) = log_obj.remove("entries") else {
        return Err("无效的 HAR 结构：找不到 log.entries".into());
    };
    let log_meta = Value::Object(std::mem::take(log_obj));

    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temp_output = output.with_extension("zip.tmp");
    let result = write_archive(&temp_output, log_meta, entries);
    match result {
        Ok((total, counts)) => {
            fs::rename(&temp_output, output)?;
            print_summary(total, &counts, output);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temp_output);
            Err(error)
        }
    }
}

fn write_archive(
    archive_path: &Path,
    log_meta: Value,
    entries: Vec<Value>,
) -> Result<(usize, ExportCounts)> {
    let file = File::create(archive_path)?;
    let mut zip = ZipWriter::new(BufWriter::new(file));
    let total = entries.len();
    let width = index_width(total);

    let mut counts = ExportCounts::default();
    let mut manifest_entries = Vec::with_capacity(total);
    let mut errors = Vec::new();

    for (index, entry) in entries.into_iter().enumerate() {
        match process_entry(&mut zip, entry, index, width, &mut counts) {
            Ok(meta_entry) => manifest_entries.push(meta_entry),
            Err(error) => {
                let message = format!("第 {index} 个条目处理失败: {error}");
                eprintln!("警告: {message}");
                errors.push(json!({ "index": index, "error": error.to_string() }));
                counts.failed += 1;
            }
        }
    }

    let manifest = build_manifest(log_meta, manifest_entries, &counts, errors);
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    write_zip_entry(&mut zip, "manifest.json", &manifest_bytes)?;
    zip.finish()?;
    Ok((total, counts))
}

fn write_zip_entry(zip: &mut ZipWriter<BufWriter<File>>, path: &str, data: &[u8]) -> Result<()> {
    let method = if is_already_compressed(path) {
        CompressionMethod::Stored
    } else {
        CompressionMethod::Deflated
    };
    let options = SimpleFileOptions::default().compression_method(method);
    zip.start_file(path, options)?;
    zip.write_all(data)?;
    Ok(())
}

fn decode_response_content(entry: &Value) -> Result<DecodedResponse<'_>> {
    let Some(content) = entry
        .get("response")
        .and_then(|response| response.get("content"))
    else {
        return Ok(DecodedResponse::None);
    };
    let Some(raw_text) = content.get("text").and_then(Value::as_str) else {
        return Ok(DecodedResponse::None);
    };
    if text(content.get("encoding")).eq_ignore_ascii_case("base64") {
        let bytes = decode_base64(raw_text)?;
        match String::from_utf8(bytes) {
            Ok(decoded_text) => Ok(DecodedResponse::Text(Cow::Owned(decoded_text))),
            Err(utf8_err) => Ok(DecodedResponse::Binary(utf8_err.into_bytes())),
        }
    } else {
        Ok(DecodedResponse::Text(Cow::Borrowed(raw_text)))
    }
}

fn process_entry(
    zip: &mut ZipWriter<BufWriter<File>>,
    mut entry: Value,
    index: usize,
    width: usize,
    counts: &mut ExportCounts,
) -> Result<Value> {
    let parsed_url = Url::parse(request_url(&entry)).ok();
    let mime_type = primary_mime_type(&entry);
    let category = classify_entry(&entry, parsed_url.as_ref(), &mime_type);

    match category {
        EntryCategory::Static(kind) => {
            let static_path = export_static_entry(
                zip,
                &entry,
                parsed_url.as_ref(),
                &mime_type,
                kind.folder(),
                index,
                width,
            )?;
            if static_path.is_some() {
                counts.static_resources += 1;
            }
            strip_extracted_body(&mut entry, static_path.as_deref(), None, "static");
            Ok(entry)
        }
        EntryCategory::Api | EntryCategory::WebSocket => {
            let is_ws = category == EntryCategory::WebSocket;
            let folder = if is_ws { "websocket" } else { "api" };
            let text_path = format!(
                "{folder}/{}",
                api_filename(&entry, parsed_url.as_ref(), index, width)
            );
            let binary_path = export_api_or_ws_entry(
                zip,
                &entry,
                parsed_url.as_ref(),
                &mime_type,
                is_ws,
                &text_path,
                index,
                width,
                counts,
            )?;
            if is_ws {
                counts.websocket += 1;
            } else {
                counts.api += 1;
            }
            strip_extracted_body(&mut entry, binary_path.as_deref(), Some(&text_path), folder);
            Ok(entry)
        }
    }
}

fn export_static_entry(
    zip: &mut ZipWriter<BufWriter<File>>,
    entry: &Value,
    parsed_url: Option<&Url>,
    mime_type: &str,
    folder: &str,
    index: usize,
    width: usize,
) -> Result<Option<String>> {
    let Some(content) = entry
        .get("response")
        .and_then(|response| response.get("content"))
    else {
        return Ok(None);
    };
    let Some(raw_text) = content.get("text").and_then(Value::as_str) else {
        return Ok(None);
    };
    let data: Cow<'_, [u8]> = if text(content.get("encoding")).eq_ignore_ascii_case("base64") {
        Cow::Owned(decode_base64(raw_text)?)
    } else {
        Cow::Borrowed(raw_text.as_bytes())
    };
    let zip_path = format!(
        "{folder}/{}",
        static_filename(parsed_url, mime_type, index, width)
    );
    write_zip_entry(zip, &zip_path, data.as_ref())?;
    Ok(Some(zip_path))
}

#[allow(clippy::too_many_arguments)]
fn export_api_or_ws_entry(
    zip: &mut ZipWriter<BufWriter<File>>,
    entry: &Value,
    parsed_url: Option<&Url>,
    mime_type: &str,
    is_ws: bool,
    text_path: &str,
    index: usize,
    width: usize,
    counts: &mut ExportCounts,
) -> Result<Option<String>> {
    let decoded = decode_response_content(entry)?;
    let (decoded_text, binary_path) = match decoded {
        DecodedResponse::None => (None, None),
        DecodedResponse::Text(body) => (Some(body), None),
        DecodedResponse::Binary(bytes) => {
            let bin_path = format!(
                "binary/{}",
                binary_filename(entry, parsed_url, mime_type, index, width)
            );
            write_zip_entry(zip, &bin_path, &bytes)?;
            counts.binary += 1;
            (None, Some(bin_path))
        }
    };
    let rendered = render_api_entry(
        entry,
        is_ws,
        decoded_text.as_deref(),
        binary_path.as_deref(),
    )?;
    write_zip_entry(zip, text_path, &rendered)?;
    Ok(binary_path)
}

fn strip_extracted_body(
    entry: &mut Value,
    extracted_file: Option<&str>,
    view_file: Option<&str>,
    category: &str,
) {
    if let Some(obj) = entry.as_object_mut() {
        obj.insert(
            "_archiveCategory".to_owned(),
            Value::String(category.to_owned()),
        );
        if let Some(path) = view_file {
            obj.insert(
                "_archiveViewFile".to_owned(),
                Value::String(path.to_owned()),
            );
        }
        if let Some(path) = extracted_file {
            obj.insert(
                "_archiveBodyFile".to_owned(),
                Value::String(path.to_owned()),
            );
        }
    }
    if extracted_file.is_some() {
        if let Some(content) = entry
            .get_mut("response")
            .and_then(|response| response.get_mut("content"))
            .and_then(Value::as_object_mut)
        {
            content.remove("text");
            content.remove("encoding");
        }
    }
}

fn build_manifest(
    log_meta: Value,
    entries: Vec<Value>,
    counts: &ExportCounts,
    errors: Vec<Value>,
) -> Value {
    let mut root = Map::new();
    root.insert(
        "summary".to_owned(),
        json!({
            "totalEntries": entries.len() + counts.failed,
            "api": counts.api,
            "websocket": counts.websocket,
            "staticResources": counts.static_resources,
            "binaryBodies": counts.binary,
            "failed": counts.failed,
        }),
    );
    if !errors.is_empty() {
        root.insert("errors".to_owned(), Value::Array(errors));
    }
    let mut log_map = match log_meta {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    log_map.insert("entries".to_owned(), Value::Array(entries));
    root.insert("log".to_owned(), Value::Object(log_map));
    Value::Object(root)
}

fn print_summary(total: usize, counts: &ExportCounts, output: &Path) {
    println!(
        "处理完成！从 HAR 文件中转换了 {total} 个条目到 '{}'。",
        output.display()
    );
    println!(
        "分类统计: API {} 个 | WebSocket {} 个 | 静态资源 {} 个 | 二进制响应 {} 个。",
        counts.api, counts.websocket, counts.static_resources, counts.binary
    );
    if counts.failed > 0 {
        println!(
            "有 {} 个条目因异常被跳过（详情已记录至 manifest.json）。",
            counts.failed
        );
    }
}
