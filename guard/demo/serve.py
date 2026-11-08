"""Тестовый «сайт» для inject.js: POST /c/x/messages отвечает эхом того, что дошло."""
import json, sys
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
class H(SimpleHTTPRequestHandler):
    def do_POST(self):
        q = json.loads(self.rfile.read(int(self.headers["Content-Length"])))["q"]
        print("ДОШЛО ДО САЙТА:", q, flush=True)
        body = json.dumps({"got": q}, ensure_ascii=False).encode()
        self.send_response(200); self.send_header("Content-Type", "application/json; charset=utf-8"); self.end_headers(); self.wfile.write(body)
sys.stdout.reconfigure(encoding="utf-8")
ThreadingHTTPServer(("127.0.0.1", 8000), H).serve_forever()
