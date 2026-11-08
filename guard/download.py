"""Модель скачивается при первом запуске с выпуска на GitHub и кладётся в %LOCALAPPDATA%/FizmatGuard/model.

Файлы — в одном выпуске (тег model-…); новая модель после дообучения — новый тег в MODEL_TAG
или FIZMAT_GUARD_MODEL_URL. Скачивается во .part и переименовывается — оборванное не примется.
"""
import os
import threading
import urllib.request
from pathlib import Path

MODEL_TAG = "model-v1"
BASE = os.environ.get("FIZMAT_GUARD_MODEL_URL",
                      f"https://github.com/iaa2005/fizmat-ai/releases/download/{MODEL_TAG}/")
FILES = ["viterbi.json", "tokenizer.json", "LICENSE", "pii-tracer-int8.onnx"]
DIR = Path(os.environ.get("FIZMAT_GUARD_MODEL_DIR")
           or Path(os.environ.get("LOCALAPPDATA", Path.home())) / "FizmatGuard" / "model" / MODEL_TAG)


class Download:
    def __init__(self):
        self.done = 0
        self.total = 0
        self.error = ""
        self.ready = threading.Event()

    def present(self) -> bool:
        return all((DIR / f).exists() for f in FILES)

    def progress(self) -> float:
        return round(self.done / self.total, 3) if self.total else 0.0

    def run(self) -> None:
        """Докачивает, чего нет; при ошибке — пишет её и пробует снова позже (зовущий решает)."""
        try:
            DIR.mkdir(parents=True, exist_ok=True)
            need = [f for f in FILES if not (DIR / f).exists()]
            sizes = {}
            for f in need:
                req = urllib.request.Request(BASE + f, method="HEAD", headers={"User-Agent": "FizmatGuard"})
                with urllib.request.urlopen(req, timeout=30) as r:
                    sizes[f] = int(r.headers.get("Content-Length") or 0)
            self.total, self.done, self.error = sum(sizes.values()), 0, ""
            for f in need:
                part = DIR / (f + ".part")
                req = urllib.request.Request(BASE + f, headers={"User-Agent": "FizmatGuard"})
                with urllib.request.urlopen(req, timeout=60) as r, open(part, "wb") as out:
                    while chunk := r.read(1 << 20):
                        out.write(chunk)
                        self.done += len(chunk)
                if sizes[f] and part.stat().st_size != sizes[f]:
                    raise IOError(f"{f}: скачано не целиком")
                part.replace(DIR / f)
            self.ready.set()
        except Exception as e:   # сеть, GitHub, диск — показать человеку и не падать
            self.error = str(e)
