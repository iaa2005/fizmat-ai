"""Поиск личных данных: PII-Tracer + правила + уже известные значения чата."""
import re
import time
from pathlib import Path

from engine import Engine
from rules import Hit, rule_hits, topics
from vault import TAGS, TAG_RX, Vault


PRONOUN = re.compile(r"(?:(?:я|мы|ты|вы|он|она|они|меня|мне|это|i|me|my)\s+)+", re.I)

# Форма, без которой находка модели — ошибка: «по номеру» телефоном не бывает.
SHAPE = {
    "private_phone": lambda v: sum(c.isdigit() for c in v) >= 7,
    "account_number": lambda v: sum(c.isdigit() for c in v) >= 4,
    "private_date": lambda v: any(c.isdigit() for c in v),
    "private_email": lambda v: "@" in v,
    "private_url": lambda v: "." in v or "/" in v,
    "private_person": lambda v: v[:1].isupper(),
    "private_address": lambda v: sum(c.isalpha() for c in v) >= 3,
}
WINDOW = 400   # токенов в окне для длинного текста
STEP = 300


class Detector:
    def __init__(self, folder: Path):
        t = time.perf_counter()
        self.model = Engine(folder)
        self.model.predict("прогрев")
        self.load_s = time.perf_counter() - t

    def _model_hits(self, text: str) -> list[Hit]:
        """Короткий текст — одним проходом, длинный — окнами с перекрытием."""
        offs = self.model.encode(text).offsets
        parts = [(0, len(text))] if len(offs) <= WINDOW else [
            (offs[a][0], offs[min(a + WINDOW, len(offs)) - 1][1]) for a in range(0, len(offs) - (WINDOW - STEP), STEP)]
        hits = []
        for lo, hi in parts:
            for s in self.model.predict(text[lo:hi])[0]:
                if s.end - s.start >= 2:   # буква «й» в «Эйлера» — сбой на стыке токенов
                    hits.append(Hit(s.start + lo, s.end + lo, s.label, "model"))
        return hits

    def scan(self, text: str, chat: str, *, commit: bool = False) -> dict:
        t = time.perf_counter()
        vault = Vault(chat)
        rules = rule_hits(text)
        # Правило точнее модели: где правило нашло номер, модельная полоса режется вокруг него.
        hits = list(rules)
        for h in self._model_hits(text):
            pieces = [(h.start, h.end)]
            for r in rules:
                pieces = [p for a, b in pieces for p in ((a, min(b, r.start)), (max(a, r.end), b)) if p[1] - p[0] >= 2]
            for a, b in pieces:
                seg = text[a:b]
                # Обрезки вроде «, ИНН » после выреза номера — без цифр и букв имени не берём.
                a += len(seg) - len(seg.lstrip(" ,;:—-"))
                b -= len(seg) - len(seg.rstrip(" ,;:—-"))
                # «Я Мария Котова» → «Мария Котова»: иначе та же Мария в другом сообщении — новая метка.
                if h.kind == "private_person" and (lead := PRONOUN.match(text, a, b)):
                    a = lead.end()
                if (b - a >= 2 and SHAPE.get(h.kind, lambda v: True)(text[a:b])
                        and not re.fullmatch(r"(?i)(снилс|инн|паспорт|карта|тел\.?|телефон)", text[a:b])):
                    hits.append(Hit(a, b, h.kind, h.source))
        # Уже скрытое в этом чате раньше — везде, даже если модель сейчас промолчала.
        for value, tag in vault.known().items():
            for m in re.finditer(re.escape(value), text):
                if not any(m.start() < x.end and x.start < m.end() for x in hits):
                    hits.append(Hit(m.start(), m.end(), "known:" + tag, "vault"))
        # Свои же метки ([ИМЯ_1] из прошлого ответа) — не данные.
        tags = [m.span() for m in TAG_RX.finditer(text)]
        hits = [h for h in hits if not any(a <= h.start and h.end <= b for a, b in tags)]
        hits.sort(key=lambda h: (h.start, -h.end))
        merged: list[Hit] = []
        for h in hits:
            if merged and h.start < merged[-1].end:
                continue
            merged.append(h)

        spans = []
        for h in merged:
            value = text[h.start:h.end]
            tag = h.kind[6:] if h.kind.startswith("known:") else vault.tag_for(value, h.kind)
            spans.append({"start": h.start, "end": h.end, "value": value, "tag": tag,
                          "kind": h.kind if not h.kind.startswith("known:") else "known", "source": h.source})
        masked = text
        for s in reversed(spans):
            masked = masked[:s["start"]] + s["tag"] + masked[s["end"]:]
        if commit:
            vault.save()
        return {"spans": spans, "masked": masked, "topics": topics(text),
                "ms": round((time.perf_counter() - t) * 1000)}

    def mask(self, text: str, chat: str, keep: list[str]) -> dict:
        """Окончательная замена при отправке: keep — значения, которые человек решил не скрывать."""
        found = self.scan(text, chat)
        vault = Vault(chat)
        out, kept = text, set(keep)
        for s in reversed(found["spans"]):
            if s["value"] in kept:
                continue
            tag = vault.tag_for(s["value"], s["kind"] if s["kind"] in TAGS else "other_pii") if s["kind"] != "known" else s["tag"]
            out = out[:s["start"]] + tag + out[s["end"]:]
        vault.save()
        return {"masked": out}
