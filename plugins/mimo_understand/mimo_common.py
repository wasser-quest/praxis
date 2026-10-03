#!/usr/bin/env python3
import json
import os
import sys
import urllib.request
import urllib.error

MIMO_API_BASE = "https://api.xiaomimimo.com/v1"
DEFAULT_MODEL = "mimo-v2.5"

def get_config():
    args = json.loads(os.environ.get("PLUGIN_ARGS", "{}"))

    sec = {}
    sec_raw = os.environ.get("PLUGIN_SECRETS", "")
    if sec_raw:
        try:
            sec = json.loads(sec_raw)
        except json.JSONDecodeError:
            pass

    api_key = sec.get("mimo_api_key", "")
    if not api_key:
        api_key = os.environ.get("MIMO_API_KEY", "")

    if not api_key:
        print(json.dumps({"error": "mimo_api_key must be set in secrets or MIMO_API_KEY env var"}))
        sys.exit(1)

    return args, api_key

def chat_completion(api_key, messages, model=None, max_tokens=1024):
    base_url = os.environ.get("MIMO_API_BASE", MIMO_API_BASE)
    url = f"{base_url}/chat/completions"

    body = {
        "model": model or DEFAULT_MODEL,
        "messages": messages,
        "max_completion_tokens": max_tokens,
    }

    data = json.dumps(body).encode("utf-8")
    headers = {
        "User-Agent": "Praxis (+https://getpraxis.boo)",
        "Content-Type": "application/json",
        "api-key": api_key,
    }

    req = urllib.request.Request(url, data=data, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            result = json.loads(resp.read().decode())
            choices = result.get("choices", [])
            if choices:
                return choices[0].get("message", {}).get("content", "No response")
            return "No response from model"
    except urllib.error.HTTPError as e:
        body_text = e.read().decode() if e.fp else ""
        return f"MiMo API error {e.code}: {body_text}"
    except Exception as e:
        return f"MiMo API error: {str(e)}"
