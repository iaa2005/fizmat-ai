"""Таблица «значение ↔ метка» на чат. Лежит только на этом компьютере.

Одно и то же значение в чате всегда получает одну метку: «Игорь Ступин» во втором
сообщении — снова [ИМЯ_1], и модель в облаке понимает, что это тот же человек.
"""
import json
import os
import re
import threading
from pathlib import Path

TAGS = {
    "private_person": "ИМЯ", "private_email": "ПОЧТА", "private_phone": "ТЕЛЕФОН",
    "private_address": "АДРЕС", "private_url": "ССЫЛКА", "private_date": "ДАТА",
    "account_number": "НОМЕР", "secret": "СЕКРЕТ", "other_pii": "ДАННЫЕ",
    "snils": "СНИЛС", "inn": "ИНН", "card": "КАРТА", "passport": "ПАСПОРТ",
    "email": "ПОЧТА", "phone": "ТЕЛЕФОН",
}
TAG_RX = re.compile(r"\[(" + "|".join(sorted(set(TAGS.values()))) + r")_(\d+)\]")

ROOT = Path(os.environ.get("APPDATA", Path.home())) / "FizmatGuard" / "vault"
_lock = threading.Lock()


def _norm(value: str) -> str:
    """Телефон «8 (903) 111-22-33» и «8 903 111 22 33» — одно значение."""
    v = value.strip()
    digits = re.sub(r"\D", "", v)
    return digits if len(digits) >= 7 and len(digits) >= len(re.sub(r"\s", "", v)) * 0.6 else v.lower()


class Vault:
    def __init__(self, chat: str):
        safe = re.sub(r"[^\w-]", "_", chat or "new")[:64]
        self.path = ROOT / f"{safe}.json"
        self.data = json.loads(self.path.read_text("utf-8")) if self.path.exists() else {"tags": {}, "values": {}}

    def tag_for(self, value: str, kind: str) -> str:
        key = _norm(value)
        if key in self.data["values"]:
            return self.data["values"][key]
        name = TAGS.get(kind, "ДАННЫЕ")
        n = 1 + sum(1 for t in self.data["tags"] if t.startswith(f"[{name}_"))
        tag = f"[{name}_{n}]"
        self.data["values"][key] = tag
        self.data["tags"][tag] = value.strip()
        return tag

    def known(self) -> dict[str, str]:
        """Значение (как было написано) → метка: для замены уже известного в новом тексте."""
        return {v: t for t, v in self.data["tags"].items()}

    def restore(self, text: str) -> str:
        return TAG_RX.sub(lambda m: self.data["tags"].get(m.group(0), m.group(0)), text)

    def save(self) -> None:
        with _lock:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            tmp = self.path.with_suffix(".tmp")
            tmp.write_text(json.dumps(self.data, ensure_ascii=False, indent=1), "utf-8")
            tmp.replace(self.path)
