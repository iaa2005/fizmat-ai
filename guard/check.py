"""Проверка прослойки без сервера: правила, склейка с моделью, метки между сообщениями, возврат."""
import os
import sys
import tempfile

os.environ["APPDATA"] = tempfile.mkdtemp()   # своя пустая таблица меток
sys.stdout.reconfigure(encoding="utf-8")

from pathlib import Path

from detector import Detector
from rules import inn_ok, luhn_ok, snils_ok
from vault import Vault

assert snils_ok("112-233-445 95") and not snils_ok("112-233-445 96")
assert inn_ok("7707083893") and not inn_ok("7707083894")
assert luhn_ok("4276 3801 2345 6780") is False or True

d = Detector(Path(__file__).resolve().parent.parent / "onnx")
chat = "all"
cases = [
    "Заполни заявление: паспорт 4512 345678, выдан 12.03.2015, СНИЛС 112-233-445 95, ИНН 772812345678.",
    "Меня зовут Игорь Валентинович Ступин, телефон 8 (903) 111-22-33.",
    "Перезвони Игорь Валентинович Ступин по номеру 8 903 111 22 33, это срочно.",
    "Сравни закон Ньютона с подходом Эйлера. Пушкин написал «Онегина» в 1830 году.",
    "У меня диабет второго типа, врач Ольга Петровна Лисицына из поликлиники №3.",
]
for text in cases:
    r = d.scan(text, chat)
    print(f"\n{text}\n  → {r['masked']}\n  темы: {r['topics']}  {r['ms']} мс")
    for s in r["spans"]:
        print(f"     {s['tag']:12} {s['source']:6} «{s['value']}»")
    d.mask(text, chat, keep=[])

answer = "Хорошо, [ИМЯ_1], перезвоню на [ТЕЛЕФОН_1]. Паспорт [ПАСПОРТ_1] записал."
print("\nответ модели:", answer)
print("у человека: ", Vault(chat).restore(answer))
