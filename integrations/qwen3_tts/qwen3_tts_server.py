#!/usr/bin/env python3
"""
Qwen3-TTS HTTP Server for RWB integration.
Usage: python qwen3_tts_server.py --model Qwen/Qwen3-TTS-12Hz-1.7B-CustomVoice --port 8001

Supports:
- CustomVoice: generate custom voice TTS
- VoiceDesign: generate with voice description
- Base: voice cloning
"""

import argparse
import base64
import io
import json
import logging
import threading
from concurrent.futures import ThreadPoolExecutor, TimeoutError as FutureTimeout
from http.server import HTTPServer, BaseHTTPRequestHandler
from pathlib import Path
from urllib.parse import urlparse

GENERATION_TIMEOUT = 90

LANGUAGE_MAP = {
    "en": "English",
    "english": "English",
    "zh": "Chinese",
    "chinese": "Chinese",
    "ja": "Japanese",
    "japanese": "Japanese",
    "ko": "Korean",
    "korean": "Korean",
    "fr": "French",
    "french": "French",
    "de": "German",
    "german": "German",
    "es": "Spanish",
    "spanish": "Spanish",
    "pt": "Portuguese",
    "portuguese": "Portuguese",
    "ar": "Arabic",
    "arabic": "Arabic",
    "hi": "Hindi",
    "hindi": "Hindi",
}

def normalize_language(lang: str) -> str:
    return LANGUAGE_MAP.get(lang.lower().strip(), lang)

import soundfile as sf
import torch
from qwen_tts import Qwen3TTSModel

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

model = None
model_type = None
lock = threading.Lock()
executor = ThreadPoolExecutor(max_workers=1)


def load_model(model_name: str, model_type: str):
    global model
    device = "cuda:0" if torch.cuda.is_available() else "cpu"
    logger.info(f"Loading model {model_name} on {device}")

    model = Qwen3TTSModel.from_pretrained(
        model_name,
        device_map=device,
        dtype=torch.bfloat16,
        attn_implementation="eager",
    )
    logger.info(f"Model loaded successfully")
    return model


class TTSRequest:
    def __init__(self, data: dict):
        self.text = data.get("text", "")
        self.language = normalize_language(data.get("language", "English"))
        self.speaker = data.get("speaker", None)
        self.instruct = data.get("instruct", None)
        self.ref_audio = data.get("ref_audio", None)
        self.ref_text = data.get("ref_text", None)
        self.clone = data.get("clone", False)
        self.raw = data.get("raw", False)


def generate_custom_voice(req: TTSRequest) -> bytes:
    global model

    logger.info("Custom voice request: text_len=%d, language=%s, speaker=%s",
                len(req.text), req.language, req.speaker)

    def _generate():
        with lock:
            if req.speaker:
                return model.generate_custom_voice(
                    text=req.text,
                    language=req.language,
                    speaker=req.speaker,
                    instruct=req.instruct,
                )
            else:
                speakers = model.get_supported_speakers()
                speaker = speakers[0] if speakers else None
                logger.info("No speaker specified, using default: %s", speaker)
                return model.generate_custom_voice(
                    text=req.text,
                    language=req.language,
                    speaker=speaker,
                )

    try:
        future = executor.submit(_generate)
        wavs, sr = future.result(timeout=GENERATION_TIMEOUT)
    except FutureTimeout:
        logger.error("Custom voice generation timed out after %ds", GENERATION_TIMEOUT)
        raise TimeoutError(f"TTS generation timed out after {GENERATION_TIMEOUT}s")

    buffer = io.BytesIO()
    sf.write(buffer, wavs[0], sr, format='WAV')
    buffer.seek(0)
    return buffer.read()


def generate_voice_design(req: TTSRequest) -> bytes:
    global model

    def _generate():
        with lock:
            return model.generate_voice_design(
                text=req.text,
                language=req.language,
                instruct=req.instruct or "",
            )

    try:
        future = executor.submit(_generate)
        wavs, sr = future.result(timeout=GENERATION_TIMEOUT)
    except FutureTimeout:
        logger.error("Voice design generation timed out after %ds", GENERATION_TIMEOUT)
        raise TimeoutError(f"TTS generation timed out after {GENERATION_TIMEOUT}s")

    buffer = io.BytesIO()
    sf.write(buffer, wavs[0], sr, format='WAV')
    buffer.seek(0)
    return buffer.read()


def generate_voice_clone(req: TTSRequest) -> bytes:
    global model

    ref_audio_path = req.ref_audio
    tmp_path = None
    if ref_audio_path:
        is_file = False
        try:
            is_file = Path(ref_audio_path).is_file()
        except OSError:
            is_file = False
        if not is_file:
            try:
                audio_bytes = base64.b64decode(ref_audio_path)
                import tempfile
                tmp = tempfile.NamedTemporaryFile(suffix=".wav", delete=False)
                tmp.write(audio_bytes)
                tmp.close()
                tmp_path = tmp.name
                ref_audio_path = tmp_path
                logger.info("Decoded base64 ref_audio to temp file: %s (%d bytes)", tmp_path, len(audio_bytes))
            except Exception as e:
                logger.warning("Failed to decode ref_audio as base64: %s", e)
                ref_audio_path = None
    else:
        logger.info("No ref_audio provided, using reference-free voice clone")

    logger.info("Voice clone request: text_len=%d, language=%s, ref_audio=%s, ref_text=%s",
                len(req.text), req.language, ref_audio_path, req.ref_text)

    def _generate():
        with lock:
            logger.info("Starting voice clone generation (ref_audio=%s)...", ref_audio_path)
            result = model.generate_voice_clone(
                text=req.text,
                language=req.language,
                ref_audio=ref_audio_path,
                ref_text=req.ref_text,
            )
            logger.info("Voice clone generation complete: samples=%d, sr=%s",
                       len(result[0]) if result and len(result) > 0 else 0,
                       result[1] if result and len(result) > 1 else 'unknown')
            return result

    try:
        future = executor.submit(_generate)
        wavs, sr = future.result(timeout=GENERATION_TIMEOUT)
    except FutureTimeout:
        logger.error("Voice clone generation timed out after %ds", GENERATION_TIMEOUT)
        raise TimeoutError(f"TTS generation timed out after {GENERATION_TIMEOUT}s")
    except Exception as e:
        logger.error("Voice clone generation failed: %s", e, exc_info=True)
        raise

    if tmp_path:
        try:
            Path(tmp_path).unlink()
        except OSError:
            pass

    audio_duration = len(wavs[0]) / sr if sr > 0 else 0
    logger.info("Generated audio: %d samples, %d Hz, %.2f seconds", len(wavs[0]), sr, audio_duration)

    if len(wavs[0]) == 0:
        raise ValueError("Generated audio is empty (0 samples)")

    buffer = io.BytesIO()
    sf.write(buffer, wavs[0], sr, format='WAV')
    buffer.seek(0)
    return buffer.read()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        logger.info(f"{self.address_string()} - {format % args}")

    def do_POST(self):
        parsed = urlparse(self.path)
        path = parsed.path

        if path == '/tts':
            content_length = int(self.headers.get('Content-Length', 0))
            data = self.rfile.read(content_length)
            req_data = json.loads(data.decode('utf-8'))
            req = TTSRequest(req_data)

            logger.info("TTS request: model_type=%s, text_len=%d, language=%s, speaker=%s, clone=%s, raw=%s, has_ref_audio=%s",
                       model_type, len(req.text), req.language, req.speaker, req.clone, req.raw, req.ref_audio is not None)

            try:
                if model_type == "CustomVoice":
                    audio = generate_custom_voice(req)
                elif model_type == "VoiceDesign":
                    audio = generate_voice_design(req)
                elif model_type == "Base":
                    audio = generate_voice_clone(req)
                else:
                    audio = generate_custom_voice(req)

                if req.raw:
                    self.send_response(200)
                    self.send_header('Content-Type', 'audio/wav')
                    self.send_header('Content-Length', str(len(audio)))
                    self.end_headers()
                    self.wfile.write(audio)
                else:
                    audio_b64 = base64.b64encode(audio).decode('utf-8')
                    response = json.dumps({"audio": audio_b64, "sample_rate": 24000})
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/json')
                    self.end_headers()
                    self.wfile.write(response.encode('utf-8'))
            except Exception as e:
                logger.error(f"TTS generation failed: {e}")
                self.send_response(500)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(json.dumps({"error": str(e)}).encode('utf-8'))

        elif path == '/health':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({"status": "ok", "model_loaded": model is not None}).encode('utf-8'))

        else:
            self.send_response(404)
            self.end_headers()

    def do_GET(self):
        if self.path == '/health':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps({"status": "ok", "model_loaded": model is not None}).encode('utf-8'))
        else:
            self.send_response(404)
            self.end_headers()


def main():
    parser = argparse.ArgumentParser(description='Qwen3-TTS HTTP Server')
    parser.add_argument('--model', type=str, default='Qwen/Qwen3-TTS-12Hz-1.7B-CustomVoice',
                        help='Model name or path')
    parser.add_argument('--type', type=str, default='CustomVoice',
                        choices=['CustomVoice', 'VoiceDesign', 'Base'],
                        help='Model type')
    parser.add_argument('--port', type=int, default=8001,
                        help='Port to listen on')
    args = parser.parse_args()

    global model_type
    model_type = args.type

    load_model(args.model, args.type)

    server = HTTPServer(('0.0.0.0', args.port), Handler)
    logger.info(f"Qwen3-TTS server listening on port {args.port}")
    server.serve_forever()


if __name__ == '__main__':
    main()
