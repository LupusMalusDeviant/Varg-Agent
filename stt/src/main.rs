use axum::{extract::Json, routing::{get, post}, Router};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::env;

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
    // Parse mxc://server/mediaId
    let stripped = mxc_url
        .strip_prefix("mxc://")
        .ok_or("Invalid mxc URL")?;
    let (server, media_id) = stripped
        .split_once('/')
        .ok_or("Invalid mxc format")?;

    // Download audio from Matrix
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

    // Detect MIME type
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

    // Base64 encode
    let audio_bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Read body failed: {e}"))?;
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&audio_bytes);

    // Call Gemini
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

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/health", get(health))
        .route("/transcribe", post(transcribe));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:5000")
        .await
        .expect("Failed to bind port 5000");

    println!("[STT] Rust sidecar listening on :5000");
    axum::serve(listener, app).await.expect("Server failed");
}
