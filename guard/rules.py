"""Правила поверх модели: российские номера с контрольными суммами и чувствительные темы.

Модель склеивает «СНИЛС 112-233-445 95, ИНН 772812345678» в одну находку вместе
со словами и почти не замечает медицину по-русски — это дочищают правила.
"""
import re
from dataclasses import dataclass


@dataclass
class Hit:
    start: int
    end: int
    kind: str      # ключ из TAGS
    source: str    # "rule" | "model"


def _digits(s: str) -> list[int]:
    return [int(c) for c in s if c.isdigit()]


def snils_ok(s: str) -> bool:
    d = _digits(s)
    if len(d) != 11:
        return False
    total = sum(v * (9 - i) for i, v in enumerate(d[:9]))
    check = total % 101 % 100 if total > 101 else (0 if total in (100, 101) else total)
    return check == d[9] * 10 + d[10]


def inn_ok(s: str) -> bool:
    d = _digits(s)
    ctrl = lambda w: sum(a * b for a, b in zip(w, d)) % 11 % 10
    if len(d) == 10:
        return ctrl([2, 4, 10, 3, 5, 9, 4, 6, 8]) == d[9]
    if len(d) == 12:
        return (ctrl([7, 2, 4, 10, 3, 5, 9, 4, 6, 8]) == d[10]
                and ctrl([3, 7, 2, 4, 10, 3, 5, 9, 4, 6, 8]) == d[11])
    return False


def luhn_ok(s: str) -> bool:
    d = _digits(s)[::-1]
    if not 13 <= len(d) <= 19:
        return False
    return sum(v if i % 2 == 0 else (v * 2 - 9 if v > 4 else v * 2) for i, v in enumerate(d)) % 10 == 0


# Номер и условие, при котором он засчитывается. Слово перед номером («паспорт», «ИНН»)
# достаточно для паспорта и ИНН; СНИЛС и карта узнаются и без слов — по контрольной сумме.
PATTERNS = [
    ("snils", re.compile(r"(?<!\d)\d{3}[- ]?\d{3}[- ]?\d{3}[- ]?\d{2}(?!\d)"), lambda m, ctx: snils_ok(m)),
    ("card", re.compile(r"(?<!\d)(?:\d{4}[ -]?){3}\d{1,7}(?!\d)"), lambda m, ctx: luhn_ok(m)),
    ("inn", re.compile(r"(?<!\d)\d{10}(?:\d{2})?(?!\d)"), lambda m, ctx: inn_ok(m) or "инн" in ctx),
    ("passport", re.compile(r"(?<!\d)\d{2} ?\d{2}[ №]{1,3}\d{6}(?!\d)"), lambda m, ctx: "паспорт" in ctx),
    ("email", re.compile(r"[\w.+-]+@[\w-]+(?:\.[\w-]+)+"), lambda m, ctx: True),
    ("phone", re.compile(r"(?<![\w+])(?:\+7|8)[\s(-]*\d{3}[\s)-]*\d{3}[\s-]*\d{2}[\s-]*\d{2}(?!\d)"), lambda m, ctx: True),
]


def rule_hits(text: str) -> list[Hit]:
    hits: list[Hit] = []
    taken: list[tuple[int, int]] = []
    for kind, rx, ok in PATTERNS:
        for m in rx.finditer(text):
            a, b = m.span()
            if any(a < y and x < b for x, y in taken):
                continue
            ctx = text[max(0, a - 24):a].lower()
            if ok(m.group(), ctx):
                hits.append(Hit(a, b, kind, "rule"))
                taken.append((a, b))
    return hits


# Темы, о которых человек может не хотеть говорить облаку. Основы слов, а не слова целиком.
TOPICS = {
    "здоровье": ["диагноз", "диабет", "онколог", "рак ", "опухол", "вич", "гепатит", "депресс", "психиатр",
                 "психотерап", "беремен", "аборт", "инвалид", "анализ кров", "таблетк", "метформин",
                 "антидепресс", "больниц", "поликлиник", "лечащ", "симптом"],
    "вера": ["православ", "мусульман", "ислам", "иудаизм", "буддизм", "атеист", "крещ", "мечет", "церков"],
    "политика": ["митинг", "оппозиц", "иноагент", "экстремист"],
    "интим": ["ориентац", "гомосекс", "лгбт", "интим"],
    "деньги": ["долг", "кредит", "ипотек", "зарплат", "банкрот", "коллектор"],
    "суд": ["судим", "уголовн", "приговор", "арест", "полиц"],
}


_TOPIC_RX = {name: re.compile(r"(?<!\w)(?:" + "|".join(re.escape(s.strip()) for s in stems) + ")", re.I)
             for name, stems in TOPICS.items()}


def topics(text: str) -> list[str]:
    """Основа — с начала слова: «вич» не должен ловить «Валентинович»."""
    return [name for name, rx in _TOPIC_RX.items() if rx.search(text)]
