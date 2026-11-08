# Fizmat для компьютера

Программа для Windows: окно [fizmat.ai](https://fizmat.ai) и локальная защита личных данных.
Перед отправкой модель **на вашем компьютере** находит имена, телефоны, почту, адреса,
СНИЛС, ИНН, паспорт и карты. В Fizmat они уходят метками вида `[ИМЯ_1]`, а в чате
вы видите настоящие значения. Таблица «значение ↔ метка» хранится только у вас.

**Скачать:** [Fizmat-setup.exe](https://github.com/iaa2005/fizmat-ai/releases/latest/download/Fizmat-setup.exe)
(Windows 10/11, x64). При первом запуске программа сама скачает модель (~600 МБ)
из выпуска [`model-v1`](https://github.com/iaa2005/fizmat-ai/releases/tag/model-v1).

## Как устроено

- `shell/` — оболочка на Tauri 2 (Rust): окно с fizmat.ai, команда `guard`, через которую
  страница обращается к службе. Ключ службы хранится в программе, страница его не видит.
- `guard/` — локальная служба на 127.0.0.1:47821:
  - `engine.py` — PII-Tracer на onnxruntime и ограниченный BIOES-Витерби (без torch);
  - `rules.py` — СНИЛС, ИНН, карта (с контрольными суммами), паспорт, почта, телефон; чувствительные темы;
  - `detector.py` — модель вместе с правилами; `vault.py` — таблица меток;
  - `download.py` — скачивание модели при первом запуске; `server.py` — HTTP-служба;
  - `inject.js` — встраивается в страницу: окно «что скрыть» перед отправкой, обратная замена меток в ленте.
- `export_onnx.py`, `quantize.py` — сборка ONNX из исходной модели; `compare.py` — сверка с исходной моделью.

## Модель

[PII-Tracer](https://huggingface.co/perplexity-ai/pii-tracer) от Perplexity (0,6B, MIT),
ONNX int8 с масштабом на канал, головы в fp32: 572 МБ, около 50 мс на сообщение на CPU.
После дообучения новая модель выйдет новым выпуском `model-vN`.

## Сборка

```bash
python -m venv .venv && .venv/Scripts/pip install onnxruntime tokenizers numpy pyinstaller
# модель: export_onnx.py + quantize.py (нужны torch, transformers, onnx) или файлы из выпуска model-v1 в onnx/
.venv/Scripts/pyinstaller --noconfirm --onedir --name guard --distpath dist-guard --paths guard --collect-binaries onnxruntime guard/server.py
cd shell && npm install && npx tauri build
```

Ограничение: восстановленные значения видны коду страницы fizmat.ai — защита скрывает данные
от модели и сервера, но не от взломанного сайта.

Лицензия кода — MIT. Модель — MIT, © 2026 Perplexity AI, Inc.
