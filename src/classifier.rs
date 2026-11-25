use serde_json::Value;
use std::path::Path;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaticKind {
    Image,
    Script,
    Style,
    Font,
    Media,
    Other,
}

impl StaticKind {
    pub const fn folder(self) -> &'static str {
        match self {
            Self::Image => "static/image",
            Self::Script => "static/script",
            Self::Style => "static/style",
            Self::Font => "static/font",
            Self::Media => "static/media",
            Self::Other => "static/other",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryCategory {
    Api,
    WebSocket,
    Static(StaticKind),
}

pub fn text(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).unwrap_or("")
}

pub fn request_url(entry: &Value) -> &str {
    text(entry.get("request").and_then(|request| request.get("url")))
}

pub fn request_method(entry: &Value) -> &str {
    entry
        .get("request")
        .and_then(|request| request.get("method"))
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN")
}

pub fn response_status(entry: &Value) -> Option<u64> {
    entry
        .get("response")
        .and_then(|response| response.get("status"))
        .and_then(Value::as_u64)
}

pub fn header_value<'a>(message: Option<&'a Value>, target_name: &str) -> Option<&'a str> {
    let headers = message
        .and_then(|message| message.get("headers"))
        .and_then(Value::as_array)?;
    headers.iter().find_map(|header| {
        text(header.get("name"))
            .eq_ignore_ascii_case(target_name)
            .then(|| text(header.get("value")))
    })
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

pub fn is_websocket(entry: &Value, parsed_url: Option<&Url>) -> bool {
    if entry.get("_webSocketMessages").is_some()
        || text(entry.get("_resourceType")).eq_ignore_ascii_case("websocket")
    {
        return true;
    }
    let websocket_url = parsed_url.is_some_and(|url| matches!(url.scheme(), "ws" | "wss"));
    websocket_url
        || has_websocket_upgrade(entry.get("request"))
        || has_websocket_upgrade(entry.get("response"))
}

pub fn primary_mime_type(entry: &Value) -> String {
    let content_mime = text(
        entry
            .get("response")
            .and_then(|response| response.get("content"))
            .and_then(|content| content.get("mimeType")),
    );
    let raw_mime = if content_mime.trim().is_empty() {
        header_value(entry.get("response"), "content-type").unwrap_or("")
    } else {
        content_mime
    };
    raw_mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

fn static_kind_from_mime(mime_type: &str) -> Option<StaticKind> {
    if mime_type.starts_with("image/") {
        return Some(StaticKind::Image);
    }
    if mime_type == "text/css" {
        return Some(StaticKind::Style);
    }
    if matches!(
        mime_type,
        "text/javascript"
            | "application/javascript"
            | "application/x-javascript"
            | "application/ecmascript"
            | "text/ecmascript"
    ) {
        return Some(StaticKind::Script);
    }
    if mime_type.starts_with("font/")
        || mime_type.starts_with("application/font-")
        || mime_type.starts_with("application/x-font-")
        || mime_type == "application/vnd.ms-fontobject"
    {
        return Some(StaticKind::Font);
    }
    if mime_type.starts_with("audio/") || mime_type.starts_with("video/") {
        return Some(StaticKind::Media);
    }
    if mime_type == "application/wasm" {
        return Some(StaticKind::Other);
    }
    None
}

fn static_kind_from_resource_type(resource_type: &str) -> Option<StaticKind> {
    if resource_type.eq_ignore_ascii_case("image") {
        Some(StaticKind::Image)
    } else if resource_type.eq_ignore_ascii_case("stylesheet") {
        Some(StaticKind::Style)
    } else if resource_type.eq_ignore_ascii_case("script") {
        Some(StaticKind::Script)
    } else if resource_type.eq_ignore_ascii_case("font") {
        Some(StaticKind::Font)
    } else if resource_type.eq_ignore_ascii_case("media") {
        Some(StaticKind::Media)
    } else {
        None
    }
}

fn static_kind_from_extension(parsed_url: Option<&Url>) -> Option<StaticKind> {
    let extension = parsed_url
        .and_then(|url| Path::new(url.path()).extension())
        .and_then(|ext| ext.to_str())?;
    match extension.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "avif" | "bmp" => {
            Some(StaticKind::Image)
        }
        "css" => Some(StaticKind::Style),
        "js" | "mjs" | "cjs" => Some(StaticKind::Script),
        "woff" | "woff2" | "ttf" | "otf" | "eot" => Some(StaticKind::Font),
        "mp4" | "webm" | "mp3" | "ogg" | "wav" => Some(StaticKind::Media),
        "wasm" | "map" => Some(StaticKind::Other),
        _ => None,
    }
}

pub fn classify_entry(entry: &Value, parsed_url: Option<&Url>, mime_type: &str) -> EntryCategory {
    if is_websocket(entry, parsed_url) || response_status(entry) == Some(101) {
        return EntryCategory::WebSocket;
    }
    if let Some(kind) = static_kind_from_mime(mime_type) {
        return EntryCategory::Static(kind);
    }
    let unknown_mime =
        mime_type.is_empty() || matches!(mime_type, "application/octet-stream" | "x-unknown");
    if unknown_mime {
        if let Some(kind) = static_kind_from_resource_type(text(entry.get("_resourceType")))
            .or_else(|| static_kind_from_extension(parsed_url))
        {
            return EntryCategory::Static(kind);
        }
    }
    EntryCategory::Api
}

pub fn is_already_compressed(path: &str) -> bool {
    let Some(extension) = Path::new(path).extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "avif"
            | "woff"
            | "woff2"
            | "mp4"
            | "webm"
            | "mp3"
            | "ogg"
            | "zip"
            | "gz"
            | "br"
            | "pdf"
    )
}
