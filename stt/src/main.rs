use axum::{extract::Json, routing::{get, post}, Router};
use base64::Engine;
use printpdf::*;
use serde::{Deserialize, Serialize};
use std::env;
use std::io::BufWriter;

// ============================================================
// STT (Speech-to-Text)
// ============================================================

#[derive(Deserialize)]
struct TranscribeReq {
    mxc_url: String,
    access_token: String,
}

#[derive(Serialize)]
struct TranscribeResp {
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
}

#[derive(Serialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum GeminiPart {
    Text { text: String },
    InlineData { inline_data: InlineData },
}

#[derive(Serialize)]
struct InlineData {
    mime_type: String,
    data: String,
}

async fn health() -> &'static str {
    "ok"
}

async fn transcribe(Json(req): Json<TranscribeReq>) -> Json<TranscribeResp> {
    match do_transcribe(&req.mxc_url, &req.access_token).await {
        Ok(text) => Json(TranscribeResp { text, error: None }),
        Err(e) => Json(TranscribeResp {
            text: String::new(),
            error: Some(e),
        }),
    }
}

async fn do_transcribe(mxc_url: &str, access_token: &str) -> Result<String, String> {
    let stripped = mxc_url
        .strip_prefix("mxc://")
        .ok_or("Invalid mxc URL")?;
    let (server, media_id) = stripped
        .split_once('/')
        .ok_or("Invalid mxc format")?;

    let hs = env::var("MATRIX_HOMESERVER").unwrap_or_else(|_| "http://conduit:6167".into());
    let url = format!(
        "{}/_matrix/media/v3/download/{}/{}?access_token={}",
        hs, server, media_id, access_token
    );

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Download HTTP {}", resp.status()));
    }

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("audio/ogg")
        .to_string();

    let mime = if content_type.contains("opus") || content_type.contains("ogg") {
        "audio/ogg"
    } else if content_type.contains("webm") {
        "audio/webm"
    } else if content_type.contains("mp4") || content_type.contains("m4a") {
        "audio/mp4"
    } else if content_type.contains("wav") {
        "audio/wav"
    } else if content_type.contains("mp3") || content_type.contains("mpeg") {
        "audio/mp3"
    } else {
        "audio/ogg"
    };

    let audio_bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Read body failed: {e}"))?;
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&audio_bytes);

    let gemini_key = env::var("GEMINI_API_KEY").map_err(|_| "GEMINI_API_KEY not set")?;
    let gemini_url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key={}",
        gemini_key
    );

    let gemini_body = GeminiRequest {
        contents: vec![GeminiContent {
            parts: vec![
                GeminiPart::Text {
                    text: "Transcribe this audio message exactly as spoken. Return ONLY the transcribed text, nothing else. If the audio is unclear, do your best. If completely unintelligible, return '[unverständlich]'.".into(),
                },
                GeminiPart::InlineData {
                    inline_data: InlineData {
                        mime_type: mime.into(),
                        data: audio_b64,
                    },
                },
            ],
        }],
    };

    let gemini_resp = client
        .post(&gemini_url)
        .json(&gemini_body)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Gemini request failed: {e}"))?;

    if !gemini_resp.status().is_success() {
        return Err(format!("Gemini HTTP {}", gemini_resp.status()));
    }

    let body: serde_json::Value = gemini_resp
        .json()
        .await
        .map_err(|e| format!("Gemini parse failed: {e}"))?;

    let text = body["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();

    Ok(text)
}

// ============================================================
// PDF Generation + Matrix Upload
// ============================================================

#[derive(Deserialize)]
struct PdfSection {
    #[serde(default)]
    heading: String,
    #[serde(default)]
    body: String,
}

#[derive(Deserialize)]
struct GeneratePdfReq {
    /// PDF document title (rendered as header on page 1)
    title: String,
    /// Content sections, each with optional heading + body text
    sections: Vec<PdfSection>,
    /// Filename for the PDF (e.g. "report.pdf")
    #[serde(default = "default_filename")]
    filename: String,
    /// Matrix homeserver URL (overrides env)
    #[serde(default)]
    homeserver: String,
    /// Matrix access token for upload
    access_token: String,
}

fn default_filename() -> String {
    "document.pdf".into()
}

#[derive(Serialize)]
struct GeneratePdfResp {
    /// mxc:// URI of the uploaded PDF
    #[serde(default)]
    mxc_uri: String,
    /// File size in bytes
    #[serde(default)]
    size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn generate_pdf(Json(req): Json<GeneratePdfReq>) -> Json<GeneratePdfResp> {
    match do_generate_pdf(&req).await {
        Ok((mxc_uri, size)) => Json(GeneratePdfResp {
            mxc_uri,
            size,
            error: None,
        }),
        Err(e) => Json(GeneratePdfResp {
            mxc_uri: String::new(),
            size: 0,
            error: Some(e),
        }),
    }
}

fn render_pdf(title: &str, sections: &[PdfSection]) -> Result<Vec<u8>, String> {
    let (doc, page1, layer1) =
        PdfDocument::new(title, Mm(210.0), Mm(297.0), "Layer 1");

    let font_regular = doc
        .add_builtin_font(BuiltinFont::Helvetica)
        .map_err(|e| format!("Font error: {e}"))?;
    let font_bold = doc
        .add_builtin_font(BuiltinFont::HelveticaBold)
        .map_err(|e| format!("Font bold error: {e}"))?;

    let margin_left = Mm(25.0);
    let margin_top = Mm(270.0);
    let page_bottom = Mm(30.0);
    let page_width = Mm(160.0); // usable width
    let line_height_body = Mm(5.5);
    let line_height_heading = Mm(8.0);
    let chars_per_line: usize = 75;

    let mut current_y = margin_top;
    let mut current_layer = doc.get_page(page1).get_layer(layer1);

    // Helper: create new page and return its layer
    let mut new_page = |doc: &PdfDocumentReference| -> PdfLayerReference {
        let (page, layer) = doc.add_page(Mm(210.0), Mm(297.0), "Layer 1");
        doc.get_page(page).get_layer(layer)
    };

    // --- Title ---
    current_layer.use_text(title, 18.0, margin_left, current_y, &font_bold);
    current_y -= Mm(4.0);

    // Separator line
    let points = vec![
        (printpdf::Point::new(margin_left, current_y), false),
        (
            printpdf::Point::new(margin_left + page_width, current_y),
            false,
        ),
    ];
    let line = printpdf::Line {
        points,
        is_closed: false,
    };
    current_layer.set_outline_color(printpdf::Color::Rgb(Rgb::new(0.3, 0.3, 0.3, None)));
    current_layer.set_outline_thickness(0.5);
    current_layer.add_line(line);
    current_y -= Mm(10.0);

    // --- Sections ---
    for section in sections {
        // Section heading
        if !section.heading.is_empty() {
            if current_y < page_bottom + Mm(15.0) {
                current_layer = new_page(&doc);
                current_y = margin_top;
            }
            current_layer.use_text(
                &section.heading,
                13.0,
                margin_left,
                current_y,
                &font_bold,
            );
            current_y -= line_height_heading;
        }

        // Section body — word-wrap
        if !section.body.is_empty() {
            let lines = word_wrap(&section.body, chars_per_line);
            for line_text in &lines {
                if current_y < page_bottom {
                    current_layer = new_page(&doc);
                    current_y = margin_top;
                }
                current_layer.use_text(
                    line_text,
                    10.0,
                    margin_left,
                    current_y,
                    &font_regular,
                );
                current_y -= line_height_body;
            }
            current_y -= Mm(3.0); // spacing after section
        }
    }

    // Serialize to bytes
    let mut buf = BufWriter::new(Vec::new());
    doc.save(&mut buf)
        .map_err(|e| format!("PDF save error: {e}"))?;
    let bytes = buf
        .into_inner()
        .map_err(|e| format!("Buffer error: {e}"))?;

    Ok(bytes)
}

/// Simple word-wrap: respects explicit \n, wraps at word boundaries
fn word_wrap(text: &str, max_chars: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }
        let words: Vec<&str> = paragraph.split_whitespace().collect();
        let mut current_line = String::new();
        for word in words {
            if current_line.is_empty() {
                current_line = word.to_string();
            } else if current_line.len() + 1 + word.len() > max_chars {
                lines.push(current_line);
                current_line = word.to_string();
            } else {
                current_line.push(' ');
                current_line.push_str(word);
            }
        }
        if !current_line.is_empty() {
            lines.push(current_line);
        }
    }
    lines
}

async fn do_generate_pdf(req: &GeneratePdfReq) -> Result<(String, usize), String> {
    // 1. Render PDF
    let pdf_bytes = render_pdf(&req.title, &req.sections)?;
    let size = pdf_bytes.len();

    // 2. Upload to Matrix
    let hs = if req.homeserver.is_empty() {
        env::var("MATRIX_HOMESERVER").unwrap_or_else(|_| "http://conduit:6167".into())
    } else {
        req.homeserver.clone()
    };

    let upload_url = format!(
        "{}/_matrix/media/v3/upload?access_token={}&filename={}",
        hs, req.access_token, req.filename
    );

    let client = reqwest::Client::new();
    let resp = client
        .post(&upload_url)
        .header("Content-Type", "application/pdf")
        .body(pdf_bytes)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Upload failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Upload HTTP {}: {}", status, body));
    }

    let upload_resp: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Upload parse failed: {e}"))?;

    let mxc_uri = upload_resp["content_uri"]
        .as_str()
        .unwrap_or("")
        .to_string();

    if mxc_uri.is_empty() {
        return Err("No content_uri in upload response".into());
    }

    Ok((mxc_uri, size))
}

// ============================================================
// Web Search (DuckDuckGo HTML)
// ============================================================

#[derive(Deserialize)]
struct WebSearchReq {
    query: String,
    #[serde(default = "default_max_results")]
    max_results: usize,
}

fn default_max_results() -> usize {
    5
}

#[derive(Serialize)]
struct SearchResult {
    title: String,
    snippet: String,
    url: String,
}

#[derive(Serialize)]
struct WebSearchResp {
    results: Vec<SearchResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn web_search(Json(req): Json<WebSearchReq>) -> Json<WebSearchResp> {
    match do_web_search(&req.query, req.max_results).await {
        Ok(results) => Json(WebSearchResp {
            results,
            error: None,
        }),
        Err(e) => Json(WebSearchResp {
            results: vec![],
            error: Some(e),
        }),
    }
}

async fn do_web_search(query: &str, max: usize) -> Result<Vec<SearchResult>, String> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0")
        .build()
        .map_err(|e| format!("Client error: {e}"))?;

    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding(&query)
    );

    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("Search failed: {e}"))?;

    let html = resp
        .text()
        .await
        .map_err(|e| format!("Read failed: {e}"))?;

    let document = scraper::Html::parse_document(&html);
    let result_sel = scraper::Selector::parse(".result").unwrap();
    let title_sel = scraper::Selector::parse(".result__a").unwrap();
    let snippet_sel = scraper::Selector::parse(".result__snippet").unwrap();
    let link_sel = scraper::Selector::parse(".result__url").unwrap();

    let mut results = Vec::new();
    for element in document.select(&result_sel) {
        if results.len() >= max {
            break;
        }
        let title = element
            .select(&title_sel)
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default()
            .trim()
            .to_string();
        let snippet = element
            .select(&snippet_sel)
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default()
            .trim()
            .to_string();
        let url = element
            .select(&link_sel)
            .next()
            .map(|e| e.text().collect::<String>())
            .unwrap_or_default()
            .trim()
            .to_string();

        if !title.is_empty() {
            results.push(SearchResult {
                title,
                snippet,
                url,
            });
        }
    }

    Ok(results)
}

fn urlencoding(s: &str) -> String {
    let mut result = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char);
            }
            b' ' => result.push('+'),
            _ => {
                result.push('%');
                result.push_str(&format!("{:02X}", b));
            }
        }
    }
    result
}

// ============================================================
// URL Fetch (HTML → Text)
// ============================================================

#[derive(Deserialize)]
struct FetchUrlReq {
    url: String,
    #[serde(default = "default_max_chars")]
    max_chars: usize,
}

fn default_max_chars() -> usize {
    8000
}

#[derive(Serialize)]
struct FetchUrlResp {
    title: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn fetch_url(Json(req): Json<FetchUrlReq>) -> Json<FetchUrlResp> {
    match do_fetch_url(&req.url, req.max_chars).await {
        Ok((title, text)) => Json(FetchUrlResp {
            title,
            text,
            error: None,
        }),
        Err(e) => Json(FetchUrlResp {
            title: String::new(),
            text: String::new(),
            error: Some(e),
        }),
    }
}

async fn do_fetch_url(url: &str, max_chars: usize) -> Result<(String, String), String> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; VargBot/1.0)")
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| format!("Client error: {e}"))?;

    let resp = client
        .get(url)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Fetch failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let html = resp
        .text()
        .await
        .map_err(|e| format!("Read failed: {e}"))?;

    let document = scraper::Html::parse_document(&html);

    // Extract title
    let title_sel = scraper::Selector::parse("title").unwrap();
    let title = document
        .select(&title_sel)
        .next()
        .map(|e| e.text().collect::<String>())
        .unwrap_or_default()
        .trim()
        .to_string();

    // Remove script/style tags, extract text from body
    let body_sel = scraper::Selector::parse("body").unwrap();
    let script_sel = scraper::Selector::parse("script, style, nav, footer, header").ok();

    let mut text = String::new();
    if let Some(body) = document.select(&body_sel).next() {
        // Collect all text nodes, skip scripts/styles
        for node in body.text() {
            let trimmed = node.trim();
            if !trimmed.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(trimmed);
                if text.len() > max_chars {
                    text.truncate(max_chars);
                    text.push_str("...");
                    break;
                }
            }
        }
    }

    // Clean up multiple whitespace
    let re = regex::Regex::new(r"\s+").unwrap();
    let text = re.replace_all(&text, " ").trim().to_string();

    Ok((title, text))
}

// ============================================================
// Image Analysis (Matrix image → Gemini Vision)
// ============================================================

#[derive(Deserialize)]
struct AnalyzeImageReq {
    mxc_url: String,
    access_token: String,
    #[serde(default = "default_image_prompt")]
    prompt: String,
}

fn default_image_prompt() -> String {
    "Beschreibe dieses Bild detailliert auf Deutsch. Was siehst du?".into()
}

#[derive(Serialize)]
struct AnalyzeImageResp {
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn analyze_image(Json(req): Json<AnalyzeImageReq>) -> Json<AnalyzeImageResp> {
    match do_analyze_image(&req.mxc_url, &req.access_token, &req.prompt).await {
        Ok(desc) => Json(AnalyzeImageResp {
            description: desc,
            error: None,
        }),
        Err(e) => Json(AnalyzeImageResp {
            description: String::new(),
            error: Some(e),
        }),
    }
}

async fn do_analyze_image(
    mxc_url: &str,
    access_token: &str,
    prompt: &str,
) -> Result<String, String> {
    // Download image from Matrix
    let stripped = mxc_url
        .strip_prefix("mxc://")
        .ok_or("Invalid mxc URL")?;
    let (server, media_id) = stripped
        .split_once('/')
        .ok_or("Invalid mxc format")?;

    let hs = env::var("MATRIX_HOMESERVER").unwrap_or_else(|_| "http://conduit:6167".into());
    let url = format!(
        "{}/_matrix/media/v3/download/{}/{}?access_token={}",
        hs, server, media_id, access_token
    );

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Download HTTP {}", resp.status()));
    }

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();

    let mime = if content_type.contains("png") {
        "image/png"
    } else if content_type.contains("gif") {
        "image/gif"
    } else if content_type.contains("webp") {
        "image/webp"
    } else {
        "image/jpeg"
    };

    let img_bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Read body failed: {e}"))?;
    let img_b64 = base64::engine::general_purpose::STANDARD.encode(&img_bytes);

    // Send to Gemini Vision
    let gemini_key = env::var("GEMINI_API_KEY").map_err(|_| "GEMINI_API_KEY not set")?;
    let gemini_url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key={}",
        gemini_key
    );

    let gemini_body = GeminiRequest {
        contents: vec![GeminiContent {
            parts: vec![
                GeminiPart::Text {
                    text: prompt.to_string(),
                },
                GeminiPart::InlineData {
                    inline_data: InlineData {
                        mime_type: mime.into(),
                        data: img_b64,
                    },
                },
            ],
        }],
    };

    let gemini_resp = client
        .post(&gemini_url)
        .json(&gemini_body)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Gemini request failed: {e}"))?;

    if !gemini_resp.status().is_success() {
        return Err(format!("Gemini HTTP {}", gemini_resp.status()));
    }

    let body: serde_json::Value = gemini_resp
        .json()
        .await
        .map_err(|e| format!("Gemini parse failed: {e}"))?;

    let text = body["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();

    Ok(text)
}

// ============================================================
// TTS (Text-to-Speech via Gemini → upload to Matrix as m.audio)
// ============================================================

#[derive(Deserialize)]
struct TtsReq {
    text: String,
    access_token: String,
    #[serde(default = "default_tts_filename")]
    filename: String,
}

fn default_tts_filename() -> String {
    "voice.ogg".into()
}

#[derive(Serialize)]
struct TtsResp {
    mxc_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn tts(Json(req): Json<TtsReq>) -> Json<TtsResp> {
    match do_tts(&req.text, &req.access_token, &req.filename).await {
        Ok(uri) => Json(TtsResp {
            mxc_uri: uri,
            error: None,
        }),
        Err(e) => Json(TtsResp {
            mxc_uri: String::new(),
            error: Some(e),
        }),
    }
}

async fn do_tts(text: &str, access_token: &str, filename: &str) -> Result<String, String> {
    // Use Gemini TTS API
    let gemini_key = env::var("GEMINI_API_KEY").map_err(|_| "GEMINI_API_KEY not set")?;
    let gemini_url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-preview-tts:generateContent?key={}",
        gemini_key
    );

    // Gemini TTS: request audio output via response_modalities
    let body = serde_json::json!({
        "contents": [{"parts": [{"text": text}]}],
        "generationConfig": {
            "response_modalities": ["AUDIO"],
            "speech_config": {
                "voiceConfig": {
                    "prebuiltVoiceConfig": {
                        "voiceName": "Kore"
                    }
                }
            }
        }
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&gemini_url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("TTS request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let err_body = resp.text().await.unwrap_or_default();
        return Err(format!("Gemini TTS HTTP {}: {}", status, err_body));
    }

    let resp_json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("TTS parse failed: {e}"))?;

    // Extract audio data from inline_data
    let audio_b64 = resp_json["candidates"][0]["content"]["parts"][0]["inlineData"]["data"]
        .as_str()
        .ok_or("No audio data in TTS response")?;
    let audio_mime = resp_json["candidates"][0]["content"]["parts"][0]["inlineData"]["mimeType"]
        .as_str()
        .unwrap_or("audio/ogg");

    // Decode base64 to bytes
    let audio_bytes = base64::engine::general_purpose::STANDARD
        .decode(audio_b64)
        .map_err(|e| format!("Base64 decode failed: {e}"))?;

    // Upload to Matrix
    let hs = env::var("MATRIX_HOMESERVER").unwrap_or_else(|_| "http://conduit:6167".into());
    let upload_url = format!(
        "{}/_matrix/media/v3/upload?access_token={}&filename={}",
        hs, access_token, filename
    );

    let upload_resp = client
        .post(&upload_url)
        .header("Content-Type", audio_mime)
        .body(audio_bytes)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Upload failed: {e}"))?;

    if !upload_resp.status().is_success() {
        return Err(format!("Upload HTTP {}", upload_resp.status()));
    }

    let upload_json: serde_json::Value = upload_resp
        .json()
        .await
        .map_err(|e| format!("Upload parse failed: {e}"))?;

    let mxc_uri = upload_json["content_uri"]
        .as_str()
        .unwrap_or("")
        .to_string();

    Ok(mxc_uri)
}

// ============================================================
// Image Generation (Gemini Imagen → upload to Matrix)
// ============================================================

#[derive(Deserialize)]
struct GenerateImageReq {
    prompt: String,
    access_token: String,
    #[serde(default = "default_image_filename")]
    filename: String,
}

fn default_image_filename() -> String {
    "generated.png".into()
}

#[derive(Serialize)]
struct GenerateImageResp {
    mxc_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn generate_image(Json(req): Json<GenerateImageReq>) -> Json<GenerateImageResp> {
    match do_generate_image(&req.prompt, &req.access_token, &req.filename).await {
        Ok(uri) => Json(GenerateImageResp {
            mxc_uri: uri,
            error: None,
        }),
        Err(e) => Json(GenerateImageResp {
            mxc_uri: String::new(),
            error: Some(e),
        }),
    }
}

async fn do_generate_image(
    prompt: &str,
    access_token: &str,
    filename: &str,
) -> Result<String, String> {
    let gemini_key = env::var("GEMINI_API_KEY").map_err(|_| "GEMINI_API_KEY not set")?;

    // Use Gemini's image generation model
    let gemini_url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-image:generateContent?key={}",
        gemini_key
    );

    let body = serde_json::json!({
        "contents": [{"parts": [{"text": prompt}]}],
        "generationConfig": {
            "response_modalities": ["TEXT", "IMAGE"]
        }
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&gemini_url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("Image gen request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let err_body = resp.text().await.unwrap_or_default();
        return Err(format!("Gemini Image HTTP {}: {}", status, err_body));
    }

    let resp_json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Image gen parse failed: {e}"))?;

    // Find image part in response
    let parts = resp_json["candidates"][0]["content"]["parts"]
        .as_array()
        .ok_or("No parts in image response")?;

    let mut image_b64 = "";
    let mut image_mime = "image/png";

    for part in parts {
        if let Some(inline) = part.get("inlineData") {
            image_b64 = inline["data"].as_str().unwrap_or("");
            image_mime = inline["mimeType"].as_str().unwrap_or("image/png");
            break;
        }
    }

    if image_b64.is_empty() {
        return Err("No image data in response".into());
    }

    let image_bytes = base64::engine::general_purpose::STANDARD
        .decode(image_b64)
        .map_err(|e| format!("Base64 decode failed: {e}"))?;

    // Upload to Matrix
    let hs = env::var("MATRIX_HOMESERVER").unwrap_or_else(|_| "http://conduit:6167".into());
    let upload_url = format!(
        "{}/_matrix/media/v3/upload?access_token={}&filename={}",
        hs, access_token, filename
    );

    let upload_resp = client
        .post(&upload_url)
        .header("Content-Type", image_mime)
        .body(image_bytes)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Upload failed: {e}"))?;

    if !upload_resp.status().is_success() {
        return Err(format!("Upload HTTP {}", upload_resp.status()));
    }

    let upload_json: serde_json::Value = upload_resp
        .json()
        .await
        .map_err(|e| format!("Upload parse failed: {e}"))?;

    let mxc_uri = upload_json["content_uri"]
        .as_str()
        .unwrap_or("")
        .to_string();

    Ok(mxc_uri)
}

// ============================================================
// Email (SMTP send, IMAP list/read)
// ============================================================

#[derive(Deserialize)]
struct EmailSendReq {
    to: String,
    subject: String,
    body: String,
    // Optional overrides from Matrix-configured values
    smtp_host: Option<String>,
    smtp_port: Option<String>,
    smtp_username: Option<String>,
    smtp_password: Option<String>,
    smtp_from: Option<String>,
}

#[derive(Deserialize)]
struct EmailListReq {
    count: Option<String>,
    folder: Option<String>,
    imap_host: Option<String>,
    imap_port: Option<String>,
    imap_username: Option<String>,
    imap_password: Option<String>,
}

#[derive(Deserialize)]
struct EmailReadReq {
    uid: String,
    imap_host: Option<String>,
    imap_port: Option<String>,
    imap_username: Option<String>,
    imap_password: Option<String>,
}

async fn email_send(Json(req): Json<EmailSendReq>) -> Json<serde_json::Value> {
    let smtp_host = req.smtp_host.clone().unwrap_or_else(|| env::var("SMTP_HOST").unwrap_or_default());
    let smtp_port: u16 = req.smtp_port.clone().unwrap_or_else(|| env::var("SMTP_PORT").unwrap_or_else(|_| "587".into())).parse().unwrap_or(587);
    let smtp_user = req.smtp_username.clone().unwrap_or_else(|| env::var("SMTP_USERNAME").unwrap_or_default());
    let smtp_pass = req.smtp_password.clone().unwrap_or_else(|| env::var("SMTP_PASSWORD").unwrap_or_default());
    let smtp_from = req.smtp_from.clone().unwrap_or_else(|| env::var("SMTP_FROM").unwrap_or_else(|_| smtp_user.clone()));

    if smtp_host.is_empty() {
        return Json(serde_json::json!({"error": "SMTP not configured"}));
    }

    use lettre::{Message, SmtpTransport, Transport};
    use lettre::transport::smtp::authentication::Credentials;

    let email = match Message::builder()
        .from(smtp_from.parse().unwrap_or_else(|_| "bot@localhost".parse().unwrap()))
        .to(req.to.parse().unwrap_or_else(|_| "nobody@localhost".parse().unwrap()))
        .subject(&req.subject)
        .body(req.body.clone())
    {
        Ok(e) => e,
        Err(e) => return Json(serde_json::json!({"error": format!("Build email failed: {e}")})),
    };

    let creds = Credentials::new(smtp_user, smtp_pass);
    let mailer = match SmtpTransport::starttls_relay(&smtp_host) {
        Ok(b) => b.port(smtp_port).credentials(creds).build(),
        Err(e) => return Json(serde_json::json!({"error": format!("SMTP connect failed: {e}")})),
    };

    match mailer.send(&email) {
        Ok(_) => Json(serde_json::json!({"success": true})),
        Err(e) => Json(serde_json::json!({"error": format!("SMTP send failed: {e}")})),
    }
}

async fn email_list(Json(req): Json<EmailListReq>) -> Json<serde_json::Value> {
    let imap_host = req.imap_host.clone().unwrap_or_else(|| env::var("IMAP_HOST").unwrap_or_default());
    let imap_port: u16 = req.imap_port.clone().unwrap_or_else(|| env::var("IMAP_PORT").unwrap_or_else(|_| "993".into())).parse().unwrap_or(993);
    let imap_user = req.imap_username.clone().unwrap_or_else(|| env::var("IMAP_USERNAME").unwrap_or_default());
    let imap_pass = req.imap_password.clone().unwrap_or_else(|| env::var("IMAP_PASSWORD").unwrap_or_default());
    let folder = req.folder.unwrap_or_else(|| "INBOX".into());
    let count: u32 = req.count.unwrap_or_else(|| "10".into()).parse().unwrap_or(10);

    if imap_host.is_empty() {
        return Json(serde_json::json!({"error": "IMAP not configured"}));
    }

    // Use tokio blocking task for IMAP (synchronous TLS)
    let result = tokio::task::spawn_blocking(move || -> Result<Vec<serde_json::Value>, String> {
        use std::net::TcpStream;
        use std::io::{Read, Write};

        // Connect with native-tls
        let tcp = TcpStream::connect(format!("{}:{}", imap_host, imap_port))
            .map_err(|e| format!("TCP connect: {e}"))?;
        let connector = native_tls::TlsConnector::new().map_err(|e| format!("TLS: {e}"))?;
        let mut tls = connector.connect(&imap_host, tcp).map_err(|e| format!("TLS connect: {e}"))?;

        let mut buf = vec![0u8; 4096];
        let _ = tls.read(&mut buf); // greeting

        // Login
        let login_cmd = format!("a1 LOGIN {} {}\r\n", imap_user, imap_pass);
        tls.write_all(login_cmd.as_bytes()).map_err(|e| format!("Write: {e}"))?;
        let _ = tls.read(&mut buf);

        // Select folder
        let sel_cmd = format!("a2 SELECT {}\r\n", folder);
        tls.write_all(sel_cmd.as_bytes()).map_err(|e| format!("Write: {e}"))?;
        buf.fill(0);
        let n = tls.read(&mut buf).map_err(|e| format!("Read: {e}"))?;
        let sel_resp = String::from_utf8_lossy(&buf[..n]).to_string();

        // Extract EXISTS count
        let mut total: u32 = 0;
        for line in sel_resp.lines() {
            if line.contains("EXISTS") {
                if let Some(num) = line.split_whitespace().nth(1) {
                    total = num.parse().unwrap_or(0);
                }
            }
        }

        if total == 0 {
            let _ = tls.write_all(b"a9 LOGOUT\r\n");
            return Ok(vec![]);
        }

        let start = if total > count { total - count + 1 } else { 1 };
        let fetch_cmd = format!("a3 FETCH {}:{} (UID FLAGS BODY.PEEK[HEADER.FIELDS (FROM SUBJECT DATE)])\r\n", start, total);
        tls.write_all(fetch_cmd.as_bytes()).map_err(|e| format!("Write: {e}"))?;

        // Read all response data
        let mut all_data = String::new();
        loop {
            buf.fill(0);
            let n = tls.read(&mut buf).unwrap_or(0);
            if n == 0 { break; }
            let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
            all_data.push_str(&chunk);
            if all_data.contains("a3 OK") || all_data.contains("a3 NO") || all_data.contains("a3 BAD") {
                break;
            }
        }

        let _ = tls.write_all(b"a9 LOGOUT\r\n");

        // Parse emails from FETCH response
        let mut emails: Vec<serde_json::Value> = Vec::new();
        let mut current_uid = String::new();
        let mut current_from = String::new();
        let mut current_subject = String::new();
        let mut current_date = String::new();

        for line in all_data.lines() {
            if line.contains("UID") {
                // Extract UID from line like "* 1 FETCH (UID 123 FLAGS ...)"
                if let Some(pos) = line.find("UID ") {
                    let rest = &line[pos + 4..];
                    current_uid = rest.split_whitespace().next().unwrap_or("").to_string();
                }
            }
            let trimmed = line.trim();
            if trimmed.starts_with("From:") {
                current_from = trimmed[5..].trim().to_string();
            }
            if trimmed.starts_with("Subject:") {
                current_subject = trimmed[8..].trim().to_string();
            }
            if trimmed.starts_with("Date:") {
                current_date = trimmed[5..].trim().to_string();
            }
            // End of headers block — emit entry
            if trimmed == ")" && !current_uid.is_empty() {
                emails.push(serde_json::json!({
                    "uid": current_uid,
                    "from": current_from,
                    "subject": current_subject,
                    "date": current_date,
                }));
                current_uid.clear();
                current_from.clear();
                current_subject.clear();
                current_date.clear();
            }
        }

        Ok(emails)
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {e}")));

    match result {
        Ok(emails) => {
            let mut text = String::new();
            for (i, e) in emails.iter().enumerate() {
                text.push_str(&format!("{}. [UID {}] {} - {} ({})\n",
                    i + 1,
                    e["uid"].as_str().unwrap_or("?"),
                    e["from"].as_str().unwrap_or("?"),
                    e["subject"].as_str().unwrap_or("(kein Betreff)"),
                    e["date"].as_str().unwrap_or("?"),
                ));
            }
            if text.is_empty() { text = "Keine Emails gefunden".into(); }
            Json(serde_json::json!({"emails": text}))
        }
        Err(e) => Json(serde_json::json!({"error": e})),
    }
}

async fn email_read(Json(req): Json<EmailReadReq>) -> Json<serde_json::Value> {
    let imap_host = req.imap_host.clone().unwrap_or_else(|| env::var("IMAP_HOST").unwrap_or_default());
    let imap_port: u16 = req.imap_port.clone().unwrap_or_else(|| env::var("IMAP_PORT").unwrap_or_else(|_| "993".into())).parse().unwrap_or(993);
    let imap_user = req.imap_username.clone().unwrap_or_else(|| env::var("IMAP_USERNAME").unwrap_or_default());
    let imap_pass = req.imap_password.clone().unwrap_or_else(|| env::var("IMAP_PASSWORD").unwrap_or_default());

    if imap_host.is_empty() {
        return Json(serde_json::json!({"error": "IMAP not configured"}));
    }

    let uid = req.uid.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
        use std::net::TcpStream;
        use std::io::{Read, Write};

        let tcp = TcpStream::connect(format!("{}:{}", imap_host, imap_port))
            .map_err(|e| format!("TCP: {e}"))?;
        let connector = native_tls::TlsConnector::new().map_err(|e| format!("TLS: {e}"))?;
        let mut tls = connector.connect(&imap_host, tcp).map_err(|e| format!("TLS connect: {e}"))?;

        let mut buf = vec![0u8; 32768];
        let _ = tls.read(&mut buf);

        let login_cmd = format!("a1 LOGIN {} {}\r\n", imap_user, imap_pass);
        tls.write_all(login_cmd.as_bytes()).map_err(|e| format!("Write: {e}"))?;
        let _ = tls.read(&mut buf);

        tls.write_all(b"a2 SELECT INBOX\r\n").map_err(|e| format!("Write: {e}"))?;
        let _ = tls.read(&mut buf);

        let fetch_cmd = format!("a3 UID FETCH {} (BODY.PEEK[HEADER] BODY.PEEK[TEXT])\r\n", uid);
        tls.write_all(fetch_cmd.as_bytes()).map_err(|e| format!("Write: {e}"))?;

        let mut all_data = String::new();
        loop {
            buf.fill(0);
            let n = tls.read(&mut buf).unwrap_or(0);
            if n == 0 { break; }
            all_data.push_str(&String::from_utf8_lossy(&buf[..n]));
            if all_data.contains("a3 OK") || all_data.contains("a3 NO") { break; }
        }

        let _ = tls.write_all(b"a9 LOGOUT\r\n");

        // Truncate to 8000 chars
        if all_data.len() > 8000 {
            all_data.truncate(8000);
            all_data.push_str("\n...[truncated]");
        }

        Ok(all_data)
    }).await.unwrap_or_else(|e| Err(format!("Task: {e}")));

    match result {
        Ok(body) => Json(serde_json::json!({"body": body})),
        Err(e) => Json(serde_json::json!({"error": e})),
    }
}

// ============================================================
// Image Conversion
// ============================================================

#[derive(Deserialize)]
struct ConvertImageReq {
    mxc_url: String,
    access_token: String,
    target_format: String,
    width: Option<u32>,
    height: Option<u32>,
}

async fn convert_image(Json(req): Json<ConvertImageReq>) -> Json<serde_json::Value> {
    let client = reqwest::Client::new();
    let hs = env::var("MATRIX_HOMESERVER").unwrap_or_else(|_| "http://conduit:6167".into());

    // Download from Matrix
    let mxc = req.mxc_url.trim_start_matches("mxc://");
    let download_url = format!("{}/_matrix/media/v3/download/{}?access_token={}", hs, mxc, req.access_token);

    let dl_resp = match client.get(&download_url).send().await {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": format!("Download: {e}")})),
    };

    let bytes = match dl_resp.bytes().await {
        Ok(b) => b,
        Err(e) => return Json(serde_json::json!({"error": format!("Read bytes: {e}")})),
    };

    // Decode image (use ::image to avoid conflict with printpdf's image module)
    let img: ::image::DynamicImage = match ::image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
    {
        Ok(reader) => match reader.decode() {
            Ok(i) => i,
            Err(e) => return Json(serde_json::json!({"error": format!("Decode: {e}")})),
        },
        Err(e) => return Json(serde_json::json!({"error": format!("Guess format: {e}")})),
    };

    // Resize if requested
    let img: ::image::DynamicImage = if let (Some(w), Some(h)) = (req.width, req.height) {
        img.resize_exact(w, h, ::image::imageops::FilterType::Lanczos3)
    } else if let Some(w) = req.width {
        let ratio = w as f64 / img.width() as f64;
        let h = (img.height() as f64 * ratio) as u32;
        img.resize_exact(w, h, ::image::imageops::FilterType::Lanczos3)
    } else if let Some(h) = req.height {
        let ratio = h as f64 / img.height() as f64;
        let w = (img.width() as f64 * ratio) as u32;
        img.resize_exact(w, h, ::image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    // Encode to target format
    let mut output = std::io::Cursor::new(Vec::new());
    let (mime, ext) = match req.target_format.to_lowercase().as_str() {
        "jpg" | "jpeg" => ("image/jpeg", "jpg"),
        "webp" => ("image/webp", "webp"),
        _ => ("image/png", "png"),
    };

    let format = match ext {
        "jpg" => ::image::ImageFormat::Jpeg,
        "webp" => ::image::ImageFormat::WebP,
        _ => ::image::ImageFormat::Png,
    };

    if let Err(e) = img.write_to(&mut output, format) {
        return Json(serde_json::json!({"error": format!("Encode: {e}")}));
    }

    let out_bytes = output.into_inner();

    // Upload to Matrix
    let filename = format!("converted.{}", ext);
    let upload_url = format!("{}/_matrix/media/v3/upload?access_token={}&filename={}", hs, req.access_token, filename);

    let upload_resp = match client.post(&upload_url)
        .header("Content-Type", mime)
        .body(out_bytes)
        .send().await
    {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": format!("Upload: {e}")})),
    };

    let upload_json: serde_json::Value = upload_resp.json().await.unwrap_or_default();
    let mxc_uri = upload_json["content_uri"].as_str().unwrap_or("").to_string();

    Json(serde_json::json!({"mxc_uri": mxc_uri}))
}

// ============================================================
// Convert to PDF (text/html/markdown → PDF via printpdf)
// ============================================================

#[derive(Deserialize)]
struct ConvertToPdfReq {
    mxc_url: String,
    access_token: String,
    filename: Option<String>,
}

async fn convert_to_pdf(Json(req): Json<ConvertToPdfReq>) -> Json<serde_json::Value> {
    let client = reqwest::Client::new();
    let hs = env::var("MATRIX_HOMESERVER").unwrap_or_else(|_| "http://conduit:6167".into());

    // Download from Matrix
    let mxc = req.mxc_url.trim_start_matches("mxc://");
    let download_url = format!("{}/_matrix/media/v3/download/{}?access_token={}", hs, mxc, req.access_token);

    let dl_resp = match client.get(&download_url).send().await {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": format!("Download: {e}")})),
    };

    let text = match dl_resp.text().await {
        Ok(t) => t,
        Err(e) => return Json(serde_json::json!({"error": format!("Read text: {e}")})),
    };

    // Strip HTML tags if present (done in sync block to avoid Send issues with scraper)
    let clean_text = {
        let t = text;
        if t.contains("<html") || t.contains("<body") || t.contains("<p>") {
            let html_doc = scraper::Html::parse_document(&t);
            let mut result = String::new();
            for text_node in html_doc.root_element().text() {
                result.push_str(text_node);
            }
            result
        } else {
            t
        }
    };

    // Generate PDF in blocking task (printpdf types are not Send)
    let pdf_result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let (doc, page1, layer1) = PdfDocument::new("Converted Document", Mm(210.0), Mm(297.0), "Layer 1");
        let font = doc.add_builtin_font(BuiltinFont::Helvetica).unwrap();

        let mut current_layer = doc.get_page(page1).get_layer(layer1);
        let mut y = 277.0_f32;
        let left = 20.0_f32;
        let max_width = 170.0_f32;
        let line_height = 5.0_f32;
        let font_size = 10.0_f32;
        let char_width = font_size * 0.45;
        let chars_per_line = (max_width / char_width) as usize;

        for line in clean_text.lines() {
            if y < 20.0 {
                let (new_page, new_layer) = doc.add_page(Mm(210.0), Mm(297.0), "Layer 1");
                current_layer = doc.get_page(new_page).get_layer(new_layer);
                y = 277.0;
            }

            if line.trim().is_empty() {
                y -= line_height;
                continue;
            }

            let words: Vec<&str> = line.split_whitespace().collect();
            let mut current_line = String::new();
            for word in &words {
                if current_line.len() + word.len() + 1 > chars_per_line {
                    current_layer.use_text(&current_line, font_size, Mm(left), Mm(y), &font);
                    y -= line_height;
                    current_line = word.to_string();
                    if y < 20.0 {
                        let (np, nl) = doc.add_page(Mm(210.0), Mm(297.0), "Layer 1");
                        current_layer = doc.get_page(np).get_layer(nl);
                        y = 277.0;
                    }
                } else {
                    if !current_line.is_empty() { current_line.push(' '); }
                    current_line.push_str(word);
                }
            }
            if !current_line.is_empty() {
                current_layer.use_text(&current_line, font_size, Mm(left), Mm(y), &font);
                y -= line_height;
            }
        }

        let mut buf = BufWriter::new(std::io::Cursor::new(Vec::new()));
        doc.save(&mut buf).map_err(|e| format!("PDF save: {e}"))?;
        Ok(buf.into_inner().unwrap().into_inner())
    }).await.unwrap_or_else(|e| Err(format!("Task: {e}")));

    let pdf_bytes = match pdf_result {
        Ok(b) => b,
        Err(e) => return Json(serde_json::json!({"error": e})),
    };

    // Upload PDF to Matrix
    let out_filename = req.filename.unwrap_or_else(|| "converted.pdf".into());
    let upload_url = format!("{}/_matrix/media/v3/upload?access_token={}&filename={}", hs, req.access_token, out_filename);

    let upload_resp = match client.post(&upload_url)
        .header("Content-Type", "application/pdf")
        .body(pdf_bytes)
        .send().await
    {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": format!("Upload: {e}")})),
    };

    let upload_json: serde_json::Value = upload_resp.json().await.unwrap_or_default();
    let mxc_uri = upload_json["content_uri"].as_str().unwrap_or("").to_string();

    Json(serde_json::json!({"mxc_uri": mxc_uri}))
}

// ============================================================
// Home Assistant state filter (parse JSON array for Varg)
// ============================================================

#[derive(Deserialize)]
struct HassFilterReq {
    states_json: String,
    domain: Option<String>,
}

async fn hass_filter(Json(req): Json<HassFilterReq>) -> Json<serde_json::Value> {
    // The states_json contains the HTTP wrapper from Varg, need to extract body
    let wrapper: serde_json::Value = match serde_json::from_str(&req.states_json) {
        Ok(v) => v,
        Err(_) => {
            // Try treating it as the direct body
            match serde_json::from_str::<serde_json::Value>(&req.states_json) {
                Ok(v) => v,
                Err(e) => return Json(serde_json::json!({"error": format!("JSON parse: {e}")})),
            }
        }
    };

    // Get the body field if it exists (Varg HTTP wrapper)
    let states_str = if let Some(body) = wrapper.get("body") {
        body.as_str().unwrap_or("").to_string()
    } else {
        req.states_json.clone()
    };

    let states: Vec<serde_json::Value> = match serde_json::from_str(&states_str) {
        Ok(v) => v,
        Err(_) => return Json(serde_json::json!({"error": "Cannot parse states array"})),
    };

    let domain_filter = req.domain.unwrap_or_default();
    let mut lines: Vec<String> = Vec::new();

    for state in &states {
        let entity_id = state["entity_id"].as_str().unwrap_or("");
        if !domain_filter.is_empty() && !entity_id.starts_with(&format!("{}.", domain_filter)) {
            continue;
        }

        let friendly_name = state["attributes"]["friendly_name"].as_str().unwrap_or(entity_id);
        let state_val = state["state"].as_str().unwrap_or("unknown");
        let unit = state["attributes"]["unit_of_measurement"].as_str().unwrap_or("");

        lines.push(format!("{} ({}): {} {}", friendly_name, entity_id, state_val, unit));

        if lines.len() >= 50 { break; } // Limit output
    }

    let text = if lines.is_empty() {
        "Keine Geraete gefunden".to_string()
    } else {
        lines.join("\n")
    };

    Json(serde_json::json!(text))
}

// ============================================================
// Calendar (CalDAV)
// ============================================================

#[derive(Deserialize)]
struct CalendarListReq {
    days: Option<String>,
    caldav_url: Option<String>,
    caldav_auth: Option<String>,
}

#[derive(Deserialize)]
struct CalendarCreateReq {
    title: String,
    start: String,
    end: Option<String>,
    description: Option<String>,
    location: Option<String>,
    caldav_url: Option<String>,
    caldav_auth: Option<String>,
}

async fn calendar_list(Json(req): Json<CalendarListReq>) -> Json<serde_json::Value> {
    let caldav_url = req.caldav_url.clone().unwrap_or_else(|| env::var("CALDAV_URL").unwrap_or_default());
    let caldav_auth = req.caldav_auth.clone().unwrap_or_else(|| env::var("CALDAV_AUTH_HEADER").unwrap_or_default());

    if caldav_url.is_empty() {
        return Json(serde_json::json!({"error": "CALDAV_URL not configured"}));
    }

    let days: i64 = req.days.unwrap_or_else(|| "7".into()).parse().unwrap_or(7);
    let now = chrono::Utc::now();
    let end = now + chrono::Duration::days(days);

    let start_str = now.format("%Y%m%dT%H%M%SZ").to_string();
    let end_str = end.format("%Y%m%dT%H%M%SZ").to_string();

    // CalDAV REPORT request for VEVENT in time range
    let xml_body = format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT">
        <c:time-range start="{}" end="{}"/>
      </c:comp-filter>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#, start_str, end_str);

    let client = reqwest::Client::new();
    let resp = match client
        .request(reqwest::Method::from_bytes(b"REPORT").unwrap(), &caldav_url)
        .header("Content-Type", "application/xml")
        .header("Depth", "1")
        .header("Authorization", format!("Basic {}", caldav_auth))
        .body(xml_body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": format!("CalDAV request: {e}")})),
    };

    let body = resp.text().await.unwrap_or_default();

    // Parse VEVENT blocks from response
    let mut events: Vec<String> = Vec::new();
    for chunk in body.split("BEGIN:VEVENT") {
        if !chunk.contains("END:VEVENT") { continue; }
        let mut summary = String::new();
        let mut dtstart = String::new();
        let mut dtend = String::new();
        let mut location = String::new();

        for line in chunk.lines() {
            let line = line.trim();
            if line.starts_with("SUMMARY:") { summary = line[8..].to_string(); }
            if line.starts_with("DTSTART") {
                if let Some(val) = line.split(':').last() { dtstart = val.to_string(); }
            }
            if line.starts_with("DTEND") {
                if let Some(val) = line.split(':').last() { dtend = val.to_string(); }
            }
            if line.starts_with("LOCATION:") { location = line[9..].to_string(); }
        }

        let mut event_str = format!("- {} ({} bis {})", summary, dtstart, dtend);
        if !location.is_empty() {
            event_str.push_str(&format!(" @ {}", location));
        }
        events.push(event_str);
    }

    let text = if events.is_empty() {
        format!("Keine Termine in den naechsten {} Tagen", days)
    } else {
        format!("Termine (naechste {} Tage):\n{}", days, events.join("\n"))
    };

    Json(serde_json::json!(text))
}

async fn calendar_create(Json(req): Json<CalendarCreateReq>) -> Json<serde_json::Value> {
    let caldav_url = req.caldav_url.clone().unwrap_or_else(|| env::var("CALDAV_URL").unwrap_or_default());
    let caldav_auth = req.caldav_auth.clone().unwrap_or_else(|| env::var("CALDAV_AUTH_HEADER").unwrap_or_default());

    if caldav_url.is_empty() {
        return Json(serde_json::json!({"error": "CALDAV_URL not configured"}));
    }

    let uid = uuid::Uuid::new_v4().to_string();
    let end = req.end.clone().unwrap_or_else(|| {
        // Default: 1 hour after start
        req.start.clone()
    });

    let mut vevent = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//VargAgent//EN\r\nBEGIN:VEVENT\r\nUID:{}\r\nSUMMARY:{}\r\nDTSTART:{}\r\nDTEND:{}\r\n",
        uid, req.title, req.start, end
    );

    if let Some(desc) = &req.description {
        vevent.push_str(&format!("DESCRIPTION:{}\r\n", desc));
    }
    if let Some(loc) = &req.location {
        vevent.push_str(&format!("LOCATION:{}\r\n", loc));
    }
    vevent.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");

    let event_url = format!("{}{}.ics", caldav_url.trim_end_matches('/'), uid);

    let client = reqwest::Client::new();
    let resp = match client
        .put(&event_url)
        .header("Content-Type", "text/calendar")
        .header("Authorization", format!("Basic {}", caldav_auth))
        .body(vevent)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": format!("CalDAV PUT: {e}")})),
    };

    if resp.status().is_success() || resp.status().as_u16() == 201 {
        Json(serde_json::json!({"success": true, "uid": uid}))
    } else {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Json(serde_json::json!({"error": format!("CalDAV {} - {}", status, body)}))
    }
}

// ============================================================
// Upload Base64 data to Matrix (for native PDF generation in agent)
// ============================================================

#[derive(Deserialize)]
struct UploadBase64Req {
    data_base64: String,
    filename: String,
    content_type: Option<String>,
    access_token: String,
    homeserver: Option<String>,
}

async fn upload_base64(Json(req): Json<UploadBase64Req>) -> Json<serde_json::Value> {
    use base64::Engine;
    let hs = req.homeserver.as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| env::var("MATRIX_HOMESERVER").unwrap_or_else(|_| "http://conduit:6167".into()));

    let content_type = req.content_type.as_deref().unwrap_or("application/octet-stream");

    let file_bytes = match base64::engine::general_purpose::STANDARD.decode(&req.data_base64) {
        Ok(b) => b,
        Err(e) => return Json(serde_json::json!({"error": format!("Base64 decode failed: {e}")})),
    };

    let upload_url = format!(
        "{}/_matrix/media/v3/upload?access_token={}&filename={}",
        hs, req.access_token, req.filename
    );

    let client = reqwest::Client::new();
    let resp = match client
        .post(&upload_url)
        .header("Content-Type", content_type)
        .body(file_bytes)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return Json(serde_json::json!({"error": format!("Upload failed: {e}")})),
    };

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Json(serde_json::json!({"error": format!("Upload HTTP {}: {}", status, body)}));
    }

    let upload_resp: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return Json(serde_json::json!({"error": format!("Upload parse failed: {e}")})),
    };

    let mxc_uri = upload_resp["content_uri"].as_str().unwrap_or("").to_string();
    if mxc_uri.is_empty() {
        return Json(serde_json::json!({"error": "No content_uri in upload response"}));
    }

    Json(serde_json::json!({"mxc_uri": mxc_uri}))
}

// ============================================================
// Main
// ============================================================

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/health", get(health))
        .route("/transcribe", post(transcribe))
        .route("/generate-pdf", post(generate_pdf))
        .route("/web-search", post(web_search))
        .route("/fetch-url", post(fetch_url))
        .route("/analyze-image", post(analyze_image))
        .route("/tts", post(tts))
        .route("/generate-image", post(generate_image))
        .route("/email-send", post(email_send))
        .route("/email-list", post(email_list))
        .route("/email-read", post(email_read))
        .route("/convert-image", post(convert_image))
        .route("/convert-to-pdf", post(convert_to_pdf))
        .route("/hass-filter", post(hass_filter))
        .route("/calendar-list", post(calendar_list))
        .route("/calendar-create", post(calendar_create))
        .route("/upload-base64", post(upload_base64));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:5000")
        .await
        .expect("Failed to bind port 5000");

    println!("[MEDIA] Rust media sidecar v0.3 listening on :5000");
    println!("[MEDIA] Endpoints: /transcribe, /generate-pdf, /web-search, /fetch-url, /analyze-image, /tts, /generate-image, /email-send, /email-list, /email-read, /convert-image, /convert-to-pdf, /hass-filter, /calendar-list, /calendar-create, /upload-base64");
    axum::serve(listener, app).await.expect("Server failed");
}
