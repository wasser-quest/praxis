"""Acknowledge a semantic request; Praxis independently runs the test verifier."""
import json
import os

request = json.loads(os.environ["PLUGIN_ARGS"])
if request != {"scope": "workspace"}:
    raise SystemExit("Unsupported test scope")
print(json.dumps({"scope": "workspace", "requested": True}))
