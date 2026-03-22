from flask import Flask, request, jsonify
import requests
import base64
import os

app = Flask(__name__)

GEMINI_KEY = os.environ.get('GEMINI_API_KEY', '')
MATRIX_HS = os.environ.get('MATRIX_HOMESERVER', 'http://conduit:6167')

@app.route('/health', methods=['GET'])
def health():
    return jsonify({'status': 'ok'})

@app.route('/transcribe', methods=['POST'])
def transcribe():
    data = request.json
    mxc_url = data.get('mxc_url', '')
    access_token = data.get('access_token', '')

    if not mxc_url.startswith('mxc://'):
        return jsonify({'text': '', 'error': 'Invalid mxc URL'}), 400

    # mxc://server/mediaId -> download URL
    parts = mxc_url[6:].split('/', 1)
    if len(parts) != 2:
        return jsonify({'text': '', 'error': 'Invalid mxc format'}), 400

    server, media_id = parts
    download_url = f"{MATRIX_HS}/_matrix/media/v3/download/{server}/{media_id}?access_token={access_token}"

    try:
        resp = requests.get(download_url, timeout=30)
        if resp.status_code != 200:
            return jsonify({'text': '', 'error': f'Download failed: {resp.status_code}'})
    except Exception as e:
        return jsonify({'text': '', 'error': str(e)})

    audio_data = resp.content
    audio_b64 = base64.b64encode(audio_data).decode('utf-8')

    # Detect mime type from content-type header
    content_type = resp.headers.get('Content-Type', 'audio/ogg')
    if 'opus' in content_type or 'ogg' in content_type:
        mime = 'audio/ogg'
    elif 'webm' in content_type:
        mime = 'audio/webm'
    elif 'mp4' in content_type or 'm4a' in content_type:
        mime = 'audio/mp4'
    elif 'wav' in content_type:
        mime = 'audio/wav'
    elif 'mp3' in content_type or 'mpeg' in content_type:
        mime = 'audio/mp3'
    else:
        mime = 'audio/ogg'

    # Use Gemini for transcription
    gemini_url = f"https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent?key={GEMINI_KEY}"

    gemini_body = {
        "contents": [{
            "parts": [
                {"text": "Transcribe this audio message exactly as spoken. Return ONLY the transcribed text, nothing else. If the audio is unclear, do your best. If completely unintelligible, return '[unverständlich]'."},
                {"inline_data": {
                    "mime_type": mime,
                    "data": audio_b64
                }}
            ]
        }]
    }

    try:
        gemini_resp = requests.post(gemini_url, json=gemini_body, timeout=30)
        if gemini_resp.status_code != 200:
            return jsonify({'text': '', 'error': f'Gemini error: {gemini_resp.status_code}'})

        result = gemini_resp.json()
        text = result.get('candidates', [{}])[0].get('content', {}).get('parts', [{}])[0].get('text', '')
        return jsonify({'text': text.strip()})
    except Exception as e:
        return jsonify({'text': '', 'error': str(e)})

if __name__ == '__main__':
    app.run(host='0.0.0.0', port=5000)
