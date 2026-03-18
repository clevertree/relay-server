#!/usr/bin/env python3
"""Minimal HTTP gateway: POST /tts JSON {"text":"..."} -> audio/wav via Piper CLI."""
import json
import os
import subprocess
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

PIPER = os.environ.get("PIPER_BIN", "/opt/relay/lib/piper/piper")
MODEL = os.environ.get("PIPER_MODEL", "")
PORT = int(os.environ.get("PIPER_HTTP_PORT", "5590"))


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        sys.stderr.write("%s\n" % (args[0],))

    def do_GET(self):
        if self.path == "/health":
            ok = os.path.isfile(PIPER) and os.path.isfile(MODEL)
            self.send_response(200 if ok else 503)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(
                json.dumps(
                    {"ok": ok, "piper": PIPER, "model": MODEL}
                ).encode()
            )
            return
        self.send_response(404)
        self.end_headers()

    def do_POST(self):
        if self.path != "/tts":
            self.send_response(404)
            self.end_headers()
            return
        try:
            n = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(n) or b"{}")
            text = (body.get("text") or "").strip()
            if not text or len(text) > 8000:
                self.send_response(400)
                self.end_headers()
                return
        except Exception:
            self.send_response(400)
            self.end_headers()
            return
        try:
            proc = subprocess.run(
                [PIPER, "--model", MODEL, "--output_file", "-"],
                input=text.encode(),
                capture_output=True,
                timeout=120,
            )
            if proc.returncode != 0:
                self.send_response(502)
                self.end_headers()
                return
            wav = proc.stdout
            self.send_response(200)
            self.send_header("Content-Type", "audio/wav")
            self.send_header("Content-Length", str(len(wav)))
            self.end_headers()
            self.wfile.write(wav)
        except Exception as e:
            sys.stderr.write("piper error: %s\n" % e)
            self.send_response(500)
            self.end_headers()


if __name__ == "__main__":
    if not MODEL or not os.path.isfile(MODEL):
        sys.stderr.write("PIPER_MODEL must point to an .onnx file\n")
        sys.exit(1)
    HTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
