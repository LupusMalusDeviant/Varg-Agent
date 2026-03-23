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
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent?key={}",
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
// Main
// ============================================================

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/health", get(health))
        .route("/transcribe", post(transcribe))
        .route("/generate-pdf", post(generate_pdf));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:5000")
        .await
        .expect("Failed to bind port 5000");

    println!("[MEDIA] Rust media sidecar listening on :5000");
    axum::serve(listener, app).await.expect("Server failed");
}
