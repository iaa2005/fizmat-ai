"""Прогон PII-Tracer на русских примерах: что находит, что пропускает, сколько времени.

Все имена, номера и адреса выдуманы.
    .venv/Scripts/python run.py
"""
import re
import sys
import time

import torch
from transformers import AutoModel

sys.stdout.reconfigure(encoding="utf-8")
torch.set_num_threads(8)

# Каждый пример — текст и то, что ДОЛЖНО быть найдено (подстроки).
SAMPLES = [
    ("простое", "Привет, я Анна Сергеевна Корнилова, пишите на korn.anna@mail.ru или звоните +7 916 555-12-34.",
     ["Анна Сергеевна Корнилова", "korn.anna@mail.ru", "+7 916 555-12-34"]),
    ("документы", "Заполни заявление: паспорт 4512 345678, выдан 12.03.2015, СНИЛС 112-233-445 95, ИНН 772812345678.",
     ["4512 345678", "12.03.2015", "112-233-445 95", "772812345678"]),
    ("карта и адрес", "Переведи на карту 4276 3801 2345 6789, получатель Дмитрий Олегович Федотов, "
     "живёт по адресу г. Казань, ул. Баумана, д. 17, кв. 42.",
     ["4276 3801 2345 6789", "Дмитрий Олегович Федотов", "г. Казань, ул. Баумана, д. 17, кв. 42"]),
    ("ловушки", "Сравни закон Ньютона с подходом Эйлера. Пушкин написал «Онегина» в 1830 году. "
     "В формуле F = ma масса m = 5 кг, а скорость 12.03 м/с. Задача по курсу СМ2, вариант 7.",
     []),
    ("код", "def connect():\n    host = 'db.internal'\n    password = 'Qx7!pass_2026'\n    api_key = 'sk-live-9f8e7d6c5b4a'\n    return host",
     ["Qx7!pass_2026", "sk-live-9f8e7d6c5b4a"]),
    ("разговор", "User: Меня зовут Игорь Валентинович Ступин, телефон 8 (903) 111-22-33.\n"
     "Assistant: Здравствуйте, Игорь Валентинович! Записал номер 8 (903) 111-22-33.\n"
     "User: Ещё запиши почту stupin.igor@yandex.ru и что я из Новосибирска, ул. Ленина 5.\n"
     "Assistant: Готово: stupin.igor@yandex.ru. Игорь, письмо отправлю сегодня.\n"
     "User: Спасибо. Моя дочь Вера Ступина тоже хочет записаться, её номер 8 903 444 55 66.",
     ["Игорь Валентинович Ступин", "8 (903) 111-22-33", "stupin.igor@yandex.ru", "Вера Ступина", "8 903 444 55 66"]),
    ("здоровье", "У меня диабет второго типа и я принимаю метформин, врач Ольга Петровна Лисицына из поликлиники №3.",
     ["Ольга Петровна Лисицына"]),
]


def show(model, name, text, want):
    t0 = time.perf_counter()
    spans, sens = model.predict(text)
    ms = (time.perf_counter() - t0) * 1000
    found = [text[s.start:s.end] for s in spans]
    print(f"\n=== {name}  ({len(text)} симв., {ms:.0f} мс, чувствительность {sens:.2f})")
    for s in spans:
        print(f"   {s.label:16} {s.score:.2f}  «{text[s.start:s.end]}»")
    # Полное совпадение: искомое целиком внутри одной найденной полосы.
    covered = lambda w: any(w in f or f in w and len(f) > len(w) * 0.6 for f in found)
    missed = [w for w in want if not covered(w)]
    extra = [f for f in found if not any(f in w or w in f for w in want)]
    if missed:
        print("   ПРОПУЩЕНО:", missed)
    if extra:
        print("   ЛИШНЕЕ:  ", extra)
    print("   скрыто:", model.mask(text).replace("\n", " / ")[:300])
    return ms, len(want) - len(missed), len(want), len(extra)


def main():
    t0 = time.perf_counter()
    model = AutoModel.from_pretrained("model", trust_remote_code=True, dtype=torch.bfloat16).eval()
    print(f"загрузка: {time.perf_counter() - t0:.1f} с")

    model.predict("прогрев")
    total = [0, 0, 0, 0]
    for name, text, want in SAMPLES:
        for i, v in enumerate(show(model, name, text, want)):
            total[i] += v
    print(f"\nИТОГО: найдено {total[1]} из {total[2]}, лишних {total[3]}, время {total[0]:.0f} мс")

    # Длинный разговор: ~3000 токенов, как средний чат.
    long = "\n".join(SAMPLES[5][1] for _ in range(25))
    n = len(model.tokenizer(long)["input_ids"])
    t0 = time.perf_counter()
    spans, _ = model.predict(long)
    print(f"длинный: {n} токенов, {(time.perf_counter() - t0):.1f} с, полос {len(spans)}")


if __name__ == "__main__":
    main()
