#!/usr/bin/env python3
"""Minimal Praxis dashboard package (Host API v1, stdlib only).

The host launches this over the process protocol on stdin/stdout, passes the
listen address and a scoped Host API grant, and supervises it. Browser users
log in with the normal operator password; every /api/* call is checked with
the host and then forwarded to Host API v1 with the package token, which never
reaches the browser.
"""
import http.server
import json
import re
import socketserver
import ssl
import struct
import sys
import threading
import urllib.error
import urllib.request

SEGMENT = re.compile(r"^[A-Za-z0-9_.:@-]{1,200}$")


def read_frame():
    header = sys.stdin.buffer.read(4)
    if len(header) != 4:
        raise EOFError
    return json.loads(sys.stdin.buffer.read(struct.unpack(">I", header)[0]))


def write_frame(value):
    data = json.dumps(value).encode()
    sys.stdout.buffer.write(struct.pack(">I", len(data)) + data)
    sys.stdout.buffer.flush()


hello = read_frame()
init = hello["initialization"]
API = init["host_api"]


def host(method, path, body=None, stream=False):
    request = urllib.request.Request(
        API["url"] + "/host/v1" + path,
        method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Authorization": "Bearer " + API["token"], "Content-Type": "application/json"},
    )
    return urllib.request.urlopen(request, timeout=None if stream else 30)


def operator_ok(token):
    if not token:
        return False
    try:
        with host("POST", "/auth/verify", {"token": token}) as r:
            return json.load(r).get("valid") is True
    except urllib.error.URLError:
        return False


PAGE = """<!doctype html><meta charset=utf-8><title>Praxis (minimal dashboard)</title>
<style>body{font:14px system-ui;margin:0;display:flex;height:100vh}
nav{width:260px;border-right:1px solid #ccc;overflow:auto}main{flex:1;overflow:auto;padding:12px}
nav div{padding:6px 10px;cursor:pointer}nav div:hover{background:#eee}pre{white-space:pre-wrap}
.m{border-bottom:1px solid #eee;padding:6px 0}.t{color:#777;font-size:12px}</style>
<nav id=s></nav><main id=m><form id=f><input type=password id=p placeholder="Admin password">
<button>Login</button></form></main><script>
let tok=localStorage.tok||"",user="";const $=id=>document.getElementById(id);
const api=(p,o={})=>fetch("/api"+p,{...o,headers:{Authorization:"Bearer "+tok,"Content-Type":"application/json"}}).then(r=>r.json());
$("f").onsubmit=async e=>{e.preventDefault();const r=await fetch("/api/auth/login",{method:"POST",
 headers:{"Content-Type":"application/json"},body:JSON.stringify({password:$("p").value})}).then(r=>r.json());
 if(r.token){tok=localStorage.tok=r.token;load()}};
async function load(){const r=await api("/sessions");if(!r.sessions)return;
 $("s").innerHTML="";r.sessions.forEach(x=>{const d=document.createElement("div");
 d.textContent=(x.session_title||x.username||x.user_id)+" ("+x.message_count+")";d.onclick=()=>open(x.user_id);$("s").append(d)})}
async function open(u){user=u;const [msgs,g,use]=await Promise.all([api("/messages/"+encodeURIComponent(u)),
 api("/graphs/"+encodeURIComponent(u)),api("/usage/"+encodeURIComponent(u))]);
 $("m").innerHTML="<h3>"+u+"</h3><p class=t>Active state: <b>"+(g.active_state||"-")+"</b> · nodes: "+
 ((g.nodes||[]).map(n=>n.id||n.name).join(", "))+"</p><pre class=t>"+JSON.stringify(use)+"</pre><div id=l></div>"+
 "<form id=c><input id=q size=60><button>Send</button> <button type=button id=x>Stop</button></form><pre id=ev class=t></pre>";
 (msgs.messages||[]).forEach(m=>{const d=document.createElement("div");d.className="m";
 d.innerHTML="<b>"+m.role+"</b> <span class=t>"+[m.prompt_tokens&&("in "+m.prompt_tokens),m.completion_tokens&&("out "+m.completion_tokens),m.generation_ms&&(m.generation_ms+" ms")].filter(Boolean).join(" · ")+"</span>";
 const p=document.createElement("pre");p.textContent=m.content;d.append(p);$("l").append(d)});
 $("c").onsubmit=async e=>{e.preventDefault();const s=await api("/agent/"+encodeURIComponent(u));
 await api("/agent/"+encodeURIComponent(u)+(s.active?"/input":"/begin"),{method:"POST",body:JSON.stringify({message:$("q").value})});$("q").value=""};
 $("x").onclick=()=>api("/agent/"+encodeURIComponent(u)+"/stop",{method:"POST"});
 const r=await fetch("/api/events/"+encodeURIComponent(u),{headers:{Authorization:"Bearer "+tok}});
 const rd=r.body.getReader(),dec=new TextDecoder();for(;;){const {value,done}=await rd.read();if(done||user!==u)break;
 $("ev").textContent=(dec.decode(value)+$("ev").textContent).slice(0,4000)}}
if(tok)load();</script>"""


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):  # stdout carries protocol frames only
        pass

    def reply(self, status, body, kind="application/json"):
        data = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def forward(self, method):
        if self.path in ("/", "/index.html"):
            return self.reply(200, PAGE.encode(), "text/html; charset=utf-8")
        if not self.path.startswith("/api/"):
            return self.reply(404, {"error": "not found"})
        rest = self.path[4:]
        route = rest.split("?", 1)[0]
        if not all(SEGMENT.match(part) for part in route.strip("/").split("/")):
            return self.reply(400, {"error": "bad path"})
        length = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(length) or b"null") if length else None
        if route != "/auth/login":
            token = (self.headers.get("Authorization") or "").removeprefix("Bearer ")
            if not operator_ok(token):
                return self.reply(401, {"error": "login required"})
        if method == "POST" and body is None and route.startswith("/agent/"):
            body = {}
        try:
            upstream = host(method, rest, body if method == "POST" else None, stream=route.startswith("/events/"))
        except urllib.error.HTTPError as error:
            return self.reply(error.code, error.read() or b"{}")
        except urllib.error.URLError:
            return self.reply(503, {"error": "host unavailable"})
        if route.startswith("/events/"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            try:
                while chunk := upstream.read1(65536):
                    self.wfile.write(chunk)
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                pass
            return
        with upstream:
            self.reply(upstream.status, upstream.read())

    def do_GET(self):
        self.forward("GET")

    def do_POST(self):
        self.forward("POST")


class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


address, port = init["listen"].rsplit(":", 1)
server = Server((address, int(port)), Handler)
if init.get("tls"):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(init["tls"]["cert"], init["tls"]["key"])
    server.socket = context.wrap_socket(server.socket, server_side=True)
threading.Thread(target=server.serve_forever, daemon=True).start()

write_frame({"type": "ready", "version": 1, "owner": hello["owner"], "service": "dashboard",
             "nonce": hello["nonce"], "operations": ["status"], "controls": []})
while True:
    try:
        request = read_frame()
    except EOFError:
        break
    identity = {"id": request["id"], "nonce": request["nonce"]}
    if request["type"] == "shutdown":
        server.shutdown()
        write_frame({"type": "stopped", **identity})
        break
    if request["type"] == "health":
        write_frame({"type": "completed", **identity, "result": {"healthy": True, "listen": init["listen"]}})
    elif request["type"] == "invoke" and request["operation"] == "status":
        write_frame({"type": "completed", **identity, "result": {"listen": init["listen"]}})
    else:
        write_frame({"type": "failed", **identity, "code": "unsupported"})
