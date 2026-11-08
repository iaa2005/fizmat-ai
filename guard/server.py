"""Прослойка на 127.0.0.1: страница fizmat.ai спрашивает её перед отправкой и при показе ответа.

    python server.py [порт]

Ключ лежит в %APPDATA%/FizmatGuard/token и передаётся оболочкой в страницу. Без ключа
и не с fizmat.ai — отказ: иначе любой сайт мог бы выспросить таблицу меток.

Сервер поднимается сразу; модели нет — скачивает её (download.py), потом грузит. Пока не готово,
/health отвечает фазой и долей скачанного, а /scan и /mask — 503.
"""
import json
import os
import secrets
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import download
from detector import Detector
from vault import Vault

sys.stdout.reconfigure(encoding="utf-8")
HOME = Path(os.environ.get("APPDATA", Path.home())) / "FizmatGuard"
ORIGINS = {"https://fizmat.ai", "http://127.0.0.1:8000", "http://localhost:8000"}
VAULT = "all"   # одна таблица на человека: метки сходятся между чатами, каналами и Главной

HOME.mkdir(parents=True, exist_ok=True)
TOKEN_FILE = HOME / "token"
if not TOKEN_FILE.exists():
    TOKEN_FILE.write_text(secrets.token_urlsafe(32), "utf-8")
TOKEN = TOKEN_FILE.read_text("utf-8").strip()

model_lock = threading.Lock()   # модель одна, запросы — по очереди
fetch = download.Download()
state = {"phase": "download", "error": "", "load_s": 0.0}
detector: Detector | None = None


def prepare():
    """Скачать (повтор раз в 30 с, пока нет сети) и загрузить модель."""
    global detector
    while not fetch.present():
        state["phase"] = "download"
        fetch.run()
        if fetch.error:
            state["error"] = fetch.error
            time.sleep(30)
    state.update(phase="loading", error="")
    try:
        detector = Detector(download.DIR)
        state.update(phase="ready", load_s=round(detector.load_s, 1))
    except Exception as e:   # битый файл — стереть и скачать заново при следующем запуске
        state.update(phase="error", error=str(e))
        (download.DIR / download.FILES[-1]).unlink(missing_ok=True)


def health() -> dict:
    return {"ok": state["phase"] == "ready", "phase": state["phase"], "progress": fetch.progress(),
            "total_mb": round(fetch.total / 2**20), "error": state["error"], "load_s": state["load_s"]}


class Handler(BaseHTTPRequestHandler):
    def _cors(self):
        origin = self.headers.get("Origin", "")
        if origin in ORIGINS:
            self.send_header("Access-Control-Allow-Origin", origin)
            self.send_header("Vary", "Origin")
            self.send_header("Access-Control-Allow-Headers", "Content-Type, X-Guard-Token")
            self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
            # Chrome/WebView2: публичная страница ходит в локальную сеть только с этим разрешением.
            self.send_header("Access-Control-Allow-Private-Network", "true")

    def _send(self, code: int, body: dict):
        data = json.dumps(body, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self._cors()
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _allowed(self) -> bool:
        origin = self.headers.get("Origin")
        return (origin is None or origin in ORIGINS) and secrets.compare_digest(self.headers.get("X-Guard-Token", ""), TOKEN)

    def do_OPTIONS(self):
        self.send_response(204)
        self._cors()
        self.end_headers()

    def do_GET(self):
        if self.path == "/health":
            return self._send(200, health())
        self._send(404, {"error": "нет такого"})

    def do_POST(self):
        if not self._allowed():
            return self._send(403, {"error": "нет доступа"})
        try:
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"{}")
        except ValueError:
            return self._send(400, {"error": "не JSON"})
        if not isinstance(body, dict):
            body = {}
        text = str(body.get("text") or "")[:200_000]
        if self.path in ("/scan", "/mask") and detector is None:
            return self._send(503, health())
        if self.path == "/scan":
            with model_lock:
                return self._send(200, detector.scan(text, VAULT))
        if self.path == "/mask":
            with model_lock:
                return self._send(200, detector.mask(text, VAULT, [str(v) for v in body.get("keep") or []]))
        if self.path == "/vault":
            return self._send(200, {"count": len(Vault(VAULT).data["tags"])})
        if self.path == "/vault/clear":
            Vault(VAULT).path.unlink(missing_ok=True)
            return self._send(200, {"count": 0})
        if self.path == "/restore":
            vault = Vault(VAULT)
            texts = body.get("texts")
            if isinstance(texts, list):
                return self._send(200, {"texts": [vault.restore(str(t)) for t in texts[:500]]})
            return self._send(200, {"text": vault.restore(text)})
        self._send(404, {"error": "нет такого"})

    def log_message(self, fmt, *args):
        pass


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 47821
    threading.Thread(target=prepare, daemon=True).start()
    print(f"FizmatGuard: 127.0.0.1:{port}, модель — {download.DIR}", flush=True)
    ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
