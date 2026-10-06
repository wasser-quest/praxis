# ElevenLabs TTS und OpenRouter-Bilder

Zwei unabhängig installierbare Praxis-Ordner-Plugins, ohne zusätzliche Python-Pakete (Python 3.9+ mit `python3` im PATH). Sie sind **Agent-Tools**, keine Änderung des Chat-Providers oder der automatischen Voice-/TTS-Einstellungen.

| Plugin-Ordner | Tool | Ausgabe |
|---|---|---|
| `plugins/elevenlabs_tts/` | `elevenlabs_tts` | MP3 (`mp3_44100_128`) |
| `plugins/openrouter_image/` | `openrouter_image_generate` | PNG, JPEG oder WebP |
| `plugins/minimax_image/` | `minimax_image_generate`, `minimax_image_analyze` | gespeichertes PNG/JPEG/WebP bzw. Bildanalyse (MiniMax, siehe [README](../plugins/minimax_image/README.md)) |

## Installation und Schlüssel

Aus dem Arbeitsverzeichnis der gewünschten Installation, nicht aus einem bereits mit diesen Plugins bestückten Source-Checkout:

```bash
./praxis plugin install /path/to/praxis-source/plugins/elevenlabs_tts
./praxis plugin install /path/to/praxis-source/plugins/openrouter_image
./praxis plugin list
```

Alternativ die jeweiligen **vollständigen Ordner** nach `$PLUGINS_DIR` (Standard `./plugins`) kopieren. `generate.py` und `media_common.py` müssen zusammenbleiben. Die Plugins sind laut Manifest aktiviert; `enabled: false` deaktiviert ihre Tools. Der Gateway lädt Plugins beim Start: Nach Installation/Änderung ist ein kontrollierter Neustart nötig. `repair-assets` installiert diese optionalen Plugins nicht automatisch.

- ElevenLabs: `elevenlabs_api_key` im Secret-Speicher; alternativ `ELEVENLABS_API_KEY` in der Service-Umgebung.
- MiniMax: `minimax_api_key` im Secret-Speicher; alternativ `MINIMAX_API_KEY`.
- OpenRouter: `openrouter_api_key` im Secret-Speicher; alternativ `OPENROUTER_API_KEY`.
- Das aktualisierte Gateway verwendet vorhandene native Schlüssel dieser Namen. Explizite **Custom Secrets** gleichen Namens haben Vorrang; leere Werte und `CHANGE_ME` überdecken keine vorhandenen nativen Schlüssel. Im Dashboard können die Schlüssel auch als Custom Secret angelegt werden. Änderungen ohne Master-Passwort bleiben nur im Speicher; zum dauerhaften Speichern die vorhandene verschlüsselte Secret-Verwaltung verwenden.
- Das neue Gateway reicht über `PLUGIN_SECRETS` nur die im Manifest des ausgewählten Plugins deklarierten Schlüssel weiter. Ältere Binaries unterstützen diese native Schlüsselauflösung noch nicht: dort sind Custom Secrets oder die genannten Umgebungsvariablen erforderlich.
- Niemals API-Keys in Tool-Argumente, `plugin.json`, POML oder Benutzer-Kontext schreiben. Plugin-Prozesse sind vertrauenswürdiger lokaler Code, **keine Sandbox**; sie erben weiterhin die normale Prozessumgebung.

Diese Entwicklung installiert nichts in `/workspace/release` und verändert keine laufenden Dienste oder Zugangsdaten.

## ElevenLabs

Eine nutzbare Voice-ID aus dem eigenen ElevenLabs-Konto ist erforderlich. Sie kann pro Aufruf, als `custom_data.elevenlabs_tts_voice_id` oder über `ELEVENLABS_VOICE_ID` gesetzt werden. Für das Modell gilt: Tool-Argument `model_id` → nichtleeres `custom_data.elevenlabs_tts_model_id` → `ELEVENLABS_TTS_MODEL` → `eleven_multilingual_v2`.

Beispielargumente für `elevenlabs_tts` (**ein echter Aufruf kostet Credits**):

```json
{
  "text": "Guten Morgen! Willkommen zur nächsten Lektion.",
  "voice_id": "VOICE_ID_FROM_YOUR_ACCOUNT",
  "model_id": "eleven_multilingual_v2",
  "speed": 1.0
}
```

Der Text wird unverändert übermittelt; keine Übersetzung, kein Voice-Cloning und keine automatische Wiedergabe. Maximal 5000 Zeichen pro Aufruf. Optional: `stability`, `similarity_boost`, `style` (je 0–1), `speed` (0,7–1,2) und `use_speaker_boost` (Boolean). Nicht angegebene Voice-Settings werden nicht überschrieben.

`language_code` ist ein optionaler ISO-639-1-Hinweis für unterstützende Modelle (z. B. `eleven_flash_v2_5` mit `de`). `eleven_multilingual_v2` unterstützt diesen Parameter laut API-Dokumentation nicht: dort weglassen; Deutsch wird aus dem deutschen Text erkannt. Modellspezifische Einschränkungen gelten zusätzlich.

Die Antwort enthält `path`, `download_url`, `mime_type`, `bytes`, Modell, Voice-ID und Zeichenzahl. Es werden nur MP3-Dateien zurückgegeben.

## OpenRouter Image API

Das Plugin nutzt die aktuelle dedizierte **`POST /api/v1/images`**-API, nicht `/chat/completions` und nicht `/images/generations`. Modellwahl: Tool-Argument `model` → nichtleeres `custom_data.openrouter_image_model` → `OPENROUTER_IMAGE_MODEL` → `openai/gpt-image-2`. Ein anderes verfügbares Bildmodell kann ausdrücklich gewählt werden, z. B. `bytedance-seed/seedream-4.5`.

Beispielargumente für `openrouter_image_generate` (**ein echter Aufruf ist kostenpflichtig**):

```json
{
  "prompt": "Ein kleiner roter Panda als Astronaut, klare Illustration, ohne Schrift",
  "model": "openai/gpt-image-2",
  "n": 1,
  "aspect_ratio": "16:9",
  "output_format": "png"
}
```

Optional: `resolution` (`512`, `1K`, `2K`, `4K`), `aspect_ratio`, `quality`, `output_format`, `background` und 1–4 HTTPS-URLs in `reference_images`. Referenzen werden als `input_references` an OpenRouter übermittelt; lokale Dateien und Data-URLs sind als Eingabe nicht unterstützt. Private Praxis-Download-Links sind für OpenRouter nicht öffentlich abrufbar. Die unterstützten Parameter hängen vom gewählten Modell/Endpoint ab. Das Plugin ruft die Discovery-API nicht automatisch auf; verfügbare Modelle/Preise/Capabilities stehen unter `GET /api/v1/images/models` und den zugehörigen `/endpoints`-Routen.

`n` ist standardmäßig 1 und auf maximal 4 begrenzt; nicht alle Modelle erlauben mehrere Bilder. Transparenz und JPEG schließen sich aus. Provider-Fallback ist ausdrücklich deaktiviert (`provider.allow_fallbacks: false`). Die Toolbeschreibung ist keine Preisgarantie: Kosten hängen von Modell, Größe und Anzahl ab.

Die Antwort enthält `images` mit lokalen Pfaden/Download-Links und, falls vorhanden, numerische `usage`-Metadaten inklusive `cost` in USD. Base64 wird lokal dekodiert und nicht in Tool-Output/LLM-History zurückgegeben. Die gesamte Bildliste wird vor dem Schreiben geprüft; Dateiendungen folgen der erkannten PNG/JPEG/WebP-Signatur und müssen zu `media_type` passen, falls vorhanden. SVG/HTML, falsche Base64-Daten, leere Ergebnisse und URL-only-Antworten werden abgelehnt. Zurückgegebene URLs werden nicht automatisch heruntergeladen.

## Speicherung, Fehler und Grenzen

- Dateien landen mit eindeutigen Namen in `$DATA_DIR/uploads` (Standard `./data/uploads`). `path` ist absolut; `download_url` zeigt auf `/api/files/<name>` im authentifizierten Praxis-Dashboard. Für Discord kann der Agent den lokalen Pfad anschließend mit dem normalen Upload-Tool versenden. Aufbewahrung/Löschen bleibt Aufgabe des Betreibers.
- Vor dem API-Aufruf wird ein temporärer Ausgabebereich angelegt. Veröffentlichte Dateien überschreiben nichts, auch keine Symlinks. Bei Schreibfehlern werden nur die in diesem Aufruf neu veröffentlichten Dateien entfernt; temporäre Dateien werden bereinigt. Ein Prozess-/Rechnerabsturz ist keine dateiübergreifende Transaktion.
- Ein Tool-Aufruf macht **genau einen POST**, keine automatischen Retries, keine Redirects. HTTP-Status und ein auswertbares `Retry-After` erscheinen im Fehlerobjekt; rohe Provider-Fehlertexte, vollständige URLs und Schlüssel nicht. Fehler enden mit Exitcode 1 und `retry_safe: false`. Nach einem Transport-/Speicherfehler kann bereits eine Generierung erfolgt sein; nicht blind erneut ausführen.
- Die `LLM_*`-Chat-Limits gelten **nicht** für diese Python-Media-Requests. Die Gateway-Retries wiederholen keine bereits ausgeführten Tools. Eine neue, ausdrücklich ausgelöste Tool-Ausführung ist jedoch eine neue, möglicherweise kostenpflichtige Generierung.
- Gesamtes HTTP-Zeitbudget inklusive DNS/Antwortkörper: ElevenLabs 120 s (`ELEVENLABS_TTS_TIMEOUT_SECONDS`), OpenRouter 300 s (`OPENROUTER_IMAGE_TIMEOUT_SECONDS`). Konfigurierbar von 0,1 bis 600 s. Kein endloses Verlängern durch langsame Teilantworten. Bereits laufende Tools werden bei einem Task-Stop wie andere Tools bis zum Ergebnis/Fehler abgewartet.
- Byte-Grenzen: 16 MiB MP3, 16 MiB pro Bild, insgesamt höchstens 64 MiB Image-JSON. Keine Garantie für Kontingente oder Medienqualität.
- Nur Betreiber-Umgebung, niemals Modellargumente: `ELEVENLABS_API_BASE` (Standard `https://api.elevenlabs.io`) und `OPENROUTER_IMAGE_API_BASE` (Standard `https://openrouter.ai/api/v1`). HTTPS ist erforderlich; HTTP ist ausschließlich für Loopback-Mocks zugelassen. Keine URL-Credentials, Query oder Fragment.

## Offline-Tests

```bash
python3 scripts/test_media_plugins.py
cargo test --locked --lib media_plugin -- --test-threads=1
cargo test --locked --lib skill_command -- --test-threads=1
```

Die Python-Tests verwenden echte Plugin-Subprozesse und lokale HTTP-Mocks mit synthetischen Credentials. Sie prüfen Request-Formate, Standalone-Installation, Unicode, Ausgabeformate, Schlüssel/Defaults, Byte-Limits, Fehlerredaktion, Redirect-Verweigerung, Zeitbudgets (auch tröpfelnde Antworten) und Speicherfehler. Es gibt **keine echten TTS-/Bildgenerierungsaufrufe**. Die Rust-Tests prüfen Registrierung, Secret-Weitergabe und Discord-`/skill`-Zugriffsschutz; reale Medienqualität und Live-Deployment sind nicht getestet.

API-Verträge: [ElevenLabs Create speech](https://elevenlabs.io/docs/api-reference/text-to-speech/convert), [OpenRouter Image Generation](https://openrouter.ai/docs/guides/overview/multimodal/image-generation).
