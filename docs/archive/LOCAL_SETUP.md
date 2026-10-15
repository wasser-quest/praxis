# Praxis auf diesem Rechner wieder verwenden

## Aktive Discord-Installation: `/workspace/release`

Die aktive Installation liegt inzwischen **nicht** unter `target/`, sondern in `/workspace/release`. Dort bleiben `.env`, `secrets.enc2`, `.secrets_salt` und `data/praxis.db` erhalten. Zum Starten:

```bash
cd /workspace/release
./praxis run
```

Den vorhandenen Master-Key nur im Terminal eingeben. Kein erneutes Onboarding und kein `--no-discord` für den Voice-Test.

- LLM-Laufzeitkorrekturen auf `testing` und noch nötige Deployment-Schritte: [docs/LLM_RESILIENCE.md](docs/LLM_RESILIENCE.md). Die laufende Release-Installation wird durch einen Git-Push nicht aktualisiert; `USE_PROVIDER` muss zum tatsächlich verwendeten Provider passen.
- Vollständige Discord-Anleitung: [docs/DISCORD_VOICE_SETUP.md](docs/DISCORD_VOICE_SETUP.md).
- Alle **92 Kontextfelder** mit Typ, erlaubten Werten, Default und tatsächlicher Wirkung: [docs/CONTEXT_VARIABLES.md](docs/CONTEXT_VARIABLES.md).
- Tutor-Template: `templates/language_learning.poml` — Französisch/Japanisch, deutsche Erklärungen bei Bedarf. Auswahl: `settings.system_template=language_learning`. Dein angepasstes `system.poml` bleibt unverändert.
- Korrekturen: früher Rustls-Provider-Start, Discord wartet auf die endgültige Gateway-Antwort, robuste ElevenLabs-Parameter/Audio-Anfragen und byte-sicheres Ollama-Streaming ohne Verlust des Antworttextes.
- **61 lokale Voice-/Streaming-Tests**, **12 CLI-/TLS-Tests** und **10 POML-Renders** erfolgreich. Die POML-Tests enthalten den gespeicherten Release-Kontext; die Dokumentationsprüfung deckt alle 92 Felder ab.
- Ein echter Discord-Test mit Mikrofon und hörbarer Antwort ist weiterhin nötig; lokale Mocks beweisen weder API-Key-/Quota-Gültigkeit noch hörbare Discord-Wiedergabe. Es wurden dafür keine echten LLM-/ElevenLabs-Anfragen ausgeführt.

Die folgenden Abschnitte beschreiben die **ursprüngliche Einrichtung** und deren Ausgangskonfiguration, nicht den heutigen Inhalt deiner Release-Secrets.

## Geprüft bei der ursprünglichen Einrichtung

- POML: `npm ci`, `npm run build-webview` und `npm run build-cli` erfolgreich.
- Praxis: erster Build ohne Voice erfolgreich (17 Compiler-Warnungen, keine Fehler; ca. 36 Minuten).
- Erster Songbird-Build: `cargo build --release --locked --features songbird --jobs 1` erfolgreich (13 Compiler-Warnungen, keine Fehler; ca. 27 Minuten). Discord-Voice ist damit einkompiliert, aber noch nicht mit einem Bot/TTS-Dienst getestet.
- `./target/release/praxis --help` und `onboard --help`: erfolgreich.
- Das Standard-POML-Template rendert im Chat- und Agent-Modus erfolgreich.
- Kein echter LLM-Aufruf getestet, da noch kein API-Key hinterlegt ist. Vollständige Test-Suites wurden nicht ausgeführt.

## Verzeichnisse und Voraussetzungen

- Praxis: `/workspace/praxis`
- POML-Quellcode: `/workspace/poml`
- Gebautes POML-CLI: `/workspace/poml/python/poml/js/cli.js`
- Rust/Cargo: über rustup in `~/.cargo/bin` (Shell: `source ~/.cargo/env`)
- Node.js 22 ist vorhanden. Für Praxis wird das JavaScript-CLI von POML benötigt, nicht das Python-SDK oder die VS-Code-Erweiterung.
- Installierte Build-Pakete: `build-essential`, `pkg-config`, `cmake`, `git`, `libssl-dev`.
- SQLite wird durch das Cargo-Feature `bundled` mitgebaut; ein separater SQLite-Server ist nicht erforderlich.

Die damalige Grundeinstellung wurde in `/workspace/praxis/.env` vorbereitet, zunächst ohne LLM-API-Keys und mit OpenAI als Platzhalter. **Deine aktuelle Konfiguration liegt in `/workspace/release/.env` und den dortigen Secrets und wird durch diese Anleitung nicht ersetzt.**

## Onboarding

```bash
cd /workspace/praxis
source ~/.cargo/env
cargo build --release --locked --features songbird --jobs 1
umask 077
./target/release/praxis onboard --interactive
```

Der erste Release-Build kann auf diesem ARM64-Rechner länger dauern. `--jobs 1` begrenzt den Speicherbedarf.

Für eine Grundeinrichtung ohne vorhandenen API-Key:

1. **POML CLI Path:** `/workspace/poml/python/poml/js/cli.js` (vorbelegt).
2. **Provider:** `1` / **Keep current provider and keys (skip)**. Die vorbereitete `.env` enthält `USE_PROVIDER=openai`, aber keinen Key. So verlangt der Assistent noch keinen Cloud-Key.
3. **Embedding provider:** `3` / **None**, wenn noch kein Ollama-/Embedding-Dienst vorhanden ist. Das ist keine eingerichtete semantische Suche; für RAG später einen echten Embedding-Dienst konfigurieren.
4. **Discord:** zunächst `n`, falls kein Bot-Token vorhanden ist.
5. **Gateway Port:** `3537`; Gateway-Key bei Bedarf mit Enter erzeugen lassen.
6. **Dashboard Port:** `1337`; ein eigenes Dashboard-Passwort setzen (mindestens 8 Zeichen, besser deutlich länger).
7. **MASTER_KEY:** ein eigenes, starkes Passwort setzen und sicher aufbewahren. Es wird zum Entschlüsseln von `secrets.enc2` benötigt. Nicht in den Chat oder als Kommandozeilenargument schreiben.
8. **VM:** zunächst deaktiviert lassen; **Data Directory:** `./data`; **Log Level:** `info`; **Voice:** zunächst `n`.

Nach erfolgreichem Onboarding liegen die Zugangsdaten verschlüsselt in `secrets.enc2`; der Salt liegt in `.secrets_salt`. Die `.env` enthält hauptsächlich die nicht geheimen Einstellungen. Onboarding ist kein verlustfreier Konfigurationseditor: Es schreibt Standardtemplates und den Secret-Speicher neu. Vor späterem erneutem Onboarding vorhandene Daten und Konfiguration sichern.

## Starten der ursprünglichen Quellkopie (nicht die aktive Discord-Installation)

Für die aktive Installation die Release-Anleitung oben verwenden. Die frühere Quellkopie konnte so ohne Discord gestartet werden:

```bash
cd /workspace/praxis
./target/release/praxis run --no-discord
```

Bei der Abfrage den beim Onboarding gewählten **MASTER_KEY** eingeben.

- Dashboard auf dem Praxis-Rechner: `http://localhost:1337`
- Gateway: Port `3537`
- Optionales Chat-TUI in einem zweiten Terminal: `cd /workspace/praxis && ./target/release/praxis chat`

**Netzwerk:** Der vorhandene Code bindet Dashboard und Gateway an `0.0.0.0`, nicht ausschließlich an localhost. Die Ports nicht ungeschützt ins Internet freigeben. Bei Zugriff von einem anderen Rechner ist dessen `localhost` nicht der Praxis-Rechner. Beispielsweise von deinem Rechner aus einen SSH-Tunnel verwenden:

```bash
ssh -L 1337:127.0.0.1:1337 benutzer@praxis-server
```

Dann im lokalen Browser `http://localhost:1337` öffnen. Container-Portfreigaben und Firewall hängen von deiner Umgebung ab und wurden hier nicht verändert.

## API-Key später eintragen

Für die vorbereitete OpenAI-Konfiguration:

1. Praxis starten und im Dashboard mit dem Dashboard-Passwort anmelden.
2. **Secrets** öffnen und **OpenAI API Key** / `openai_api_key` eintragen.
3. Zum dauerhaften Speichern auch den **Master Password** / `master_password` angeben. Ohne diesen speichert die aktuelle Implementierung Änderungen nur im Arbeitsspeicher.
4. In `.env` bei Bedarf `OPENAI_MODEL` und `OPENAI_API_BASE` auf ein für deinen Zugang verfügbares Modell bzw. den richtigen Endpunkt ändern.
5. Danach Praxis neu starten: Der Gateway-LLM-Router wird beim Start mit den gespeicherten Keys aufgebaut. Auch Änderungen an `.env` benötigen einen Neustart.

`OPENAI_MODEL=gpt-4o` ist lediglich der Projektstandard, kein geprüfter Zugriff auf dieses Modell. Ein funktionierender KI-Chat ist erst mit einem gültigen Key und zugänglichen Modell testbar.

Andere unterstützte Provider können über `USE_PROVIDER` und die passenden Modell-/Endpunktfelder gewählt werden, zum Beispiel `anthropic`, `ollama`, `llamacpp`, `minimax`, `mimo` oder `openrouter`. Der aktualisierte Wizard speichert `USE_PROVIDER` auch bei neuer Auswahl zuverlässig. Ältere Installationen auf eine fehlende/falsche Zeile prüfen: Der aktualisierte Service startet nicht mit einem unregistrierten ausgewählten Provider. Für Ollama/Llama.cpp muss zusätzlich ein erreichbarer lokaler Modellserver laufen; allein die Providerauswahl installiert oder startet keinen Modellserver.

## Ollama: ursprüngliche lokale Installation

Die offizielle ARM64-Version **0.33.3** wurde im rechten Herdr-Terminal `w7:p7` für Benutzer `marvin` installiert:

- Installation: `/home/marvin/.local/opt/ollama`
- CLI: `/home/marvin/.local/bin/ollama`
- `~/.bashrc` ergänzt `~/.local/bin` auch für neue interaktive Nicht-Login-Terminals zum PATH.
- Keine systemd-Unit eingerichtet: In diesem Container läuft kein systemd.
- Keine Modelle heruntergeladen, kein Cloud-Login und keine Modell-/Cloud-API-Anfragen ausgeführt. Die CLI-Hilfe funktioniert; ein laufender Modellserver wurde damit noch nicht getestet.

Server bei Bedarf im Terminal starten (nur lokal erreichbar):

```bash
OLLAMA_HOST=127.0.0.1:11434 ollama serve
```

Für Ollama Cloud danach in einem **zweiten** Terminal selbst anmelden:

```bash
OLLAMA_HOST=127.0.0.1:11434 ollama signin
```

Dieser Login wird erst bei eigener Ausführung durchgeführt und benötigt Internetzugang. Anschließend einen tatsächlich für deinen Zugang verfügbaren Cloud-Modelltag wählen. Praxis kann dann mit folgender Konfiguration an den lokalen Ollama-Server angebunden werden (Platzhalter ersetzen):

```env
USE_PROVIDER=ollama
OLLAMA_API_BASE=http://127.0.0.1:11434
OLLAMA_MODEL=<exakter-cloud-modelltag>
```

Keine `/api`- oder `/v1`-Endung an `OLLAMA_API_BASE` anhängen; Praxis ergänzt selbst `/api/chat`. Wenn Ollama in einem anderen Container/Rechner läuft, stattdessen dessen erreichbare private Adresse verwenden. Nicht ungeschützt Port 11434 ins Internet freigeben. Für dauerhaftes Hosting muss auch der Ollama-Zustand unter `~/.ollama` persistent sein.

Bei **Praxis → eigener Ollama-Server → Ollama Cloud** verwaltet der Ollama-Server den Cloud-Zugang. Ein Key in Praxis wird nur als Bearer-Header an den in `OLLAMA_API_BASE` eingestellten Server gesendet und meldet diesen nicht automatisch bei Ollama Cloud an. Die bestehende Praxis-Konfiguration wurde durch die Installation nicht umgestellt.

**Nachtrag zur Voice-Korrektur, weiterhin keine Live-Modell-API-Prüfung:** Der Ollama-Client verwendet das native `/api/chat`-Format. Die gefundenen Streaming-Probleme wurden mit lokalen Regressionstests korrigiert: Ein leeres Schluss-Chunk löscht die Antwort nicht mehr, frühe Tool-Calls bleiben erhalten, und UTF-8 wird erst nach vollständigen NDJSON-Zeilen dekodiert. Fehler/abgebrochene Streams werden gemeldet. `health_check` prüft jetzt den HTTP-Status und sendet einen gegebenenfalls konfigurierten Bearer-Key. Das prüft weder Cloud-Anmeldung noch Modellzugriff oder Kreditstand; diese bleiben separat live zu verifizieren.

## Optionale Funktionen

- **Discord:** Bot im Discord Developer Portal erstellen/konfigurieren; Token unter **Secrets → Discord Bot Token**, `DISCORD_APPLICATION_ID` in `.env`. Danach ohne `--no-discord` starten.
- **RAG/Embeddings:** Erst einen tatsächlichen Embedding-Dienst und ein Modell bereitstellen. Der aktuelle Embedding-Code liest `EMBEDDING_PROVIDER`, `EMBEDDING_MODEL` sowie `OLLAMA_BASE_URL` bzw. `OPENAI_BASE_URL` (abweichend von den Chat-Einstellungen `*_API_BASE`). Die Onboarding-Auswahl „None“ richtet keinen Ollama-Server ein.
- **VM:** QEMU und ein passendes Betriebssystem-Image nötig. Für x86-Gäste: `qemu-system-x86` und `qemu-utils`; für ARM64-Gäste: `qemu-system-arm` und `qemu-utils`. Architektur und verfügbare Beschleunigung beachten. Nicht für normalen Chat erforderlich.
- **Voice:** `songbird` für Discord-Voice ist jetzt im aktuellen Build aktiviert. Ein TTS-Zugang (z. B. ElevenLabs oder ein Qwen-TTS-Server) muss zusätzlich eingerichtet werden. Für lokale Spracherkennung sind gesonderte Engines/Modelle und Features nötig; die Whisper-Transkription ist im vorhandenen Code noch ein Stub. Windows SAPI funktioniert nicht unter Linux. Es wurden keine TTS-Dienste eingerichtet.

## POML erneut bauen / lokale Reparatur

```bash
cd /workspace/poml
HUSKY=0 npm ci --no-audit --no-fund
npm run build-webview
npm run build-cli
```

`templates/system.poml` enthielt ein HTML-`style`-Attribut, das der POML/React-Renderer ablehnte. Dieses Attribut wurde entfernt; der eigentliche Text blieb erhalten. Das Standardtemplate wurde danach mit dem gebauten CLI und einer Beispielanfrage erfolgreich gerendert. Beim nächsten Praxis-Build wird das korrigierte Template auch in das Onboarding-Binary eingebettet.

## Frühere Daten übernehmen

In der übertragenen Praxis-Kopie waren vor dieser Einrichtung **keine `.env`, kein `secrets.enc2` und keine Datenbank** vorhanden. Falls alte Chats/Benutzer/Secrets erhalten bleiben sollen, müssen die ursprünglichen Laufzeitdaten vom alten Rechner übernommen werden, insbesondere:

- der bisherige `DATA_DIR` (meist `data/`),
- `secrets.enc2` **zusammen mit** `.secrets_salt`,
- die bisherige `.env` und der zugehörige MASTER_KEY,
- gegebenenfalls eigene Templates, Kontexte, Plugins, Skills und VM-Dateien.

Vor dem Ersetzen vorhandene neue Dateien sichern und Praxis stoppen. Den POML-Pfad in einer übernommenen `.env` an diesen Rechner anpassen. Ohne den passenden Salt und MASTER_KEY lassen sich alte verschlüsselte Secrets nicht wiederherstellen.
