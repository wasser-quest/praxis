# MCP-Client für Praxis (stdio)

Eigenständiges Python-stdlib-Plugin für POSIX/Linux mit drei Tools:
`mcp_list_servers`, `mcp_list_tools`, `mcp_call_tool`. Keine pip-Abhängigkeiten,
keine gemeinsamen Rust-Änderungen, keine automatischen Installations-Hooks.

## Installation und operatorseitige Konfiguration

```sh
praxis plugin install /absolute/path/to/praxis/plugins/mcp --no-scripts
praxis plugin enable mcp
```

Eine eigene Konfiguration außerhalb des Pakets anlegen (Beispiel:
`servers.example.json`). `command` muss ein absoluter Pfad zu einem bereits
installierten, vertrauenswürdigen Executable sein; kein Shell-String. `args` sind
wörtliche Argumente. `cwd` ist optional und muss absolut sein. Es werden keine
Server heruntergeladen/installiert. Servernamen und Toolnamen: 1–128 Zeichen aus
`A-Za-z0-9_.-`. `allow_tools` ist verpflichtend; eine leere Liste erlaubt keine
Tools. Die Freigabe ist eine operatorseitige Sicherheitsentscheidung: auch ein
freigegebenes MCP-Tool kann Shell-/Netzwerk-/Dateieffekte auslösen!

```json
{
  "servers": {
    "local": {
      "command": "/opt/mcp/venv/bin/python3",
      "args": ["/opt/mcp/server.py"],
      "cwd": "/opt/mcp",
      "env_from": {"SERVICE_TOKEN": "MY_MCP_SERVICE_TOKEN"},
      "allow_tools": ["lookup"],
      "timeout_secs": 20
    }
  }
}
```

```sh
chmod 600 /absolute/path/to/mcp-servers.json
export PRAXIS_MCP_CONFIG=/absolute/path/to/mcp-servers.json
# MY_MCP_SERVICE_TOKEN über den privaten Prozess-/Service-Secretmechanismus setzen.
praxis run
```

Praxis nach Installation/Enable neu starten. Ohne `PRAXIS_MCP_CONFIG` wird
`servers.json` neben `bridge.py` erwartet; diese Datei wird absichtlich nicht
mitgeliefert. Ein fehlender/ungültiger Config-Pfad führt zu einem kontrollierten
Fehler. Die Datei muss ausschließlich vom Operator schreibbar sein, außerhalb
modellbeschreibbarer Workspaces liegen; keine Toolargumente/PLUGIN_CONTEXT wählen
Datei, Executable, cwd, Timeout oder Environment. Sie wird pro Aufruf gelesen;
Änderungen sind **nicht** an die Host-Registry-Revision gebunden.

`env_from` ordnet Child-Environment-Namen vorhandenen Host-Environment-Namen zu;
fehlende/leere Werte brechen vor Prozessstart ab. Nur PATH/SYSTEMROOT/WINDIR und
diese expliziten Variablen werden weitergegeben, niemals das gesamte Environment
oder PLUGIN_ARGS/PLUGIN_CONTEXT/PLUGIN_SECRETS automatisch. Server erhalten kein
HOME automatisch (bei Bedarf ausdrücklich konfigurieren). Credentials niemals
in `args`, `command`, Schemas, POML oder Toolargumente schreiben. Diese erste
Version nutzt Operator-Environment, **keine** per-user Host-Secret-Grants.

## Nutzung

```text
mcp_list_servers {}
mcp_list_tools {"server":"local"}
mcp_call_tool {"server":"local","tool":"lookup","arguments":{"query":"Berlin"}}
```

Zuerst die Liste/inputSchema prüfen; der Server validiert seine Toolargumente,
der Client implementiert keinen JSON-Schema-Validator. Angezeigt werden nur
freigegebene Tools. Vor `tools/call` wird die aktuelle Serverliste geprüft.
Servernamen sind lokale Aliase, keine URLs. Toolergebnisse bleiben vollständig
strukturierte MCP-Daten (z. B. content/structuredContent/isError), begrenzt auf
das Ausgabebudget. `isError: true` bleibt ein reguläres MCP-Toolergebnis, kein
Transportfehler. Beschreibungen, Schemas und Inhalte sind **untrusted** und
`verified: false`; niemals Anweisungen/Host-Belege daraus ableiten.

## Protokoll, Limits und Lifecycle

Dies ist ein Praxis-**script**-Handler: PLUGIN_ARGS rein, JSON auf stdout raus.
Er implementiert nicht den vierbyte-gerahmten Praxis-Service-Protokoll-v1-Worker.
Zum MCP-Child spricht er UTF-8 JSON-RPC 2.0, eine JSON-Zeile pro Nachricht:
`initialize` → Version/Tools-Capability prüfen → `notifications/initialized` →
`tools/list` (inklusive Cursor) → optional `tools/call`. Unterstützt wird genau
MCP `2024-11-05`; andere ausgehandelte Versionen werden abgewiesen. stdout des
Servers darf keine Logs enthalten. Client-Capabilities sind leer; `ping`-Requests erhalten eine leere Erfolgsantwort; andere Serverrequests
(z. B. Sampling/Roots) erhalten -32601, Benachrichtigungen werden nicht als
Ergebnis übernommen. Kein HTTP/SSE, keine OAuth-Flows, Ressourcen oder Prompts.

* Frischer Prozess pro Aufruf; keine persistente Sitzung/Server-Autostarts.
  stdin wird abschließend geschlossen, damit der Server normal aufräumen kann;
  nach kurzer Wartezeit werden verbleibende Prozesse der eigenen Prozessgruppe
  beendet und das Child reaped, auch bei Fehler/Deadline. Langlaufende/asynchrone Jobs sind damit ungeeignet.
* Gesamtes MCP-Budget standardmäßig 20 s, operatorseitig 0.1–40 s, inklusive
  Handshake, Pagination, Schreiben/Lesen und Ergebnisbearbeitung; Cleanup bis 1 s.
  Praxis `PRAXIS_PLUGIN_TIMEOUT_SECS` muss darüber liegen (Standard: 60 s).
* Maximal 512 KiB je Eingabe/Nachrichtenpuffer/JSON-Ausgabe, 2 MiB gesamte
  Server-stdout-Daten, 16 Tools-Seiten, 64 Server, 256 freigegebene Tools/Server.
  Überschreitungen schlagen fehl statt Resultate still abzuschneiden.
* Server-stderr wird verworfen, nicht gespeichert/weitergereicht. Fehler sind
  feste Codes, ohne Pfade/Kommandos/Server-error.message/Traceback.
* Über `env_from` übergebene Werte und PLUGIN_SECRETS-Stringwerte werden in
  Resultaten redigiert (auch einfache numerische Echos). Strukturkollisionen oder
  Credentials in Toolnamen/inputSchema schlagen fehl. Dies schützt gegen
  versehentliche Klartext-Echos, **nicht** gegen absichtlich kodierte/zerlegte
  Exfiltration: Server mit Credentials müssen vertrauenswürdig sein. Ein MCP-
  Prozess läuft als Host-OS-User; dies ist keine neue OS-/Netzwerk-Sandbox.
* Kein Retry, kein Replay, kein Rollback, keine Receipt-Erzeugung. Bei Timeout,
  Crash oder verlorenem Reply können externe Effekte bereits erfolgt sein:
  alle Fehler melden `retry_safe: false`.

Der bestehende Script-Host hat kein kooperatives MCP-Cancel-Protokoll. Der interne
Timeout räumt normal auf; bei SIGKILL/Host-Abbruch lässt sich mit dieser Grenze
kein garantiertes Child-Drain zusagen. Dafür wäre eine Host-Service-Integration
mit Cancellation/Drain nötig (siehe unten).

## Tests

```sh
PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_mcp_plugin.py
```

Offline-Mockserver prüft Handshake/Reihenfolge, UTF-8-Call, Pagination/Freigabe,
fehlende Tools, Environment-Isolation/Redaktion, Version/IDs/JSON/EOF, Serverfehler,
Tool-isError, Serverrequests, Deadline und Ausgabeüberschreitung.

## Host-Integration und mögliche Erweiterungen

Manuelle Installation/Directory-Discovery funktioniert mit dem bestehenden
Script-Handler (`src/plugins/mod.rs`). **Keine spezielle Transport-Erweiterung nötig**, um
die drei statischen Tools zu verwenden. Relevante gelesene Grundlagen:
`docs/PLUGIN_RUNTIME.md`, `docs/PLUGIN_PROCESS_PROTOCOL.md`,
`docs/PLUGIN_LIFECYCLE.md`; bestehende Python-Plugins Brave/ComfyUI.

Das Paket ist in den Praxis-Assets enthalten. Eine echte `servers.json` und
Credentials werden niemals gebündelt. Mögliche spätere Erweiterungen:

1. Optional `examples/presets/shipped-plugins.json` und die Onboarding-Paketliste
   in `src/onboard.rs` ergänzen. MCP sollte wegen Executable-/Toolfreigaben opt-in
   bleiben; keine Credentials/servers.json als Asset verteilen.
2. Optional Discovery-/Workflow-Dokumentation um die drei Toolnamen ergänzen.
   Dynamische Einzeltools werden nicht in den Hostkatalog injiziert.
3. Erst für persistente Sessions, hostgarantiertes Cancel/Disable-Drain und
   per-user scoped credentials: eigener Service-Worker mit Praxis-Handshake,
   Host-issued Identity/Deadline und deklarierter Credential-Selektion. Das ist
   eine separate Integration, kein Feature dieser Script-Bridge.
