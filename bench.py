"""Скорость на CPU: bf16 против fp32, целиком против окон по 512 токенов с перекрытием."""
import sys
import time

import torch
from transformers import AutoModel

from run import SAMPLES

sys.stdout.reconfigure(encoding="utf-8")
torch.set_num_threads(8)
LONG = "\n".join(SAMPLES[5][1] for _ in range(25))


def windows(model, text, size=512, step=256):
    """Окна по size токенов с шагом step; полосы склеиваются по символам."""
    tok = model.tokenizer(text, return_offsets_mapping=True)
    offs = tok["offset_mapping"]
    spans = {}
    for a in range(0, max(1, len(offs) - step), step):
        b = min(a + size, len(offs))
        lo, hi = offs[a][0], offs[b - 1][1]
        for s in model.predict(text[lo:hi])[0]:
            spans[(s.start + lo, s.end + lo)] = s.label
        if b == len(offs):
            break
    return spans


for dtype in (torch.bfloat16, torch.float32):
    m = AutoModel.from_pretrained("model", trust_remote_code=True, dtype=dtype).eval()
    m.predict("прогрев")
    t = time.perf_counter(); m.predict(SAMPLES[5][1]); short = time.perf_counter() - t
    t = time.perf_counter(); whole = m.predict(LONG)[0]; full = time.perf_counter() - t
    t = time.perf_counter(); win = windows(m, LONG); chunk = time.perf_counter() - t
    print(f"{str(dtype):15} разговор 392 симв.: {short*1000:.0f} мс | 4700 ток. целиком: {full:.1f} с, полос {len(whole)} "
          f"| окнами 512/256: {chunk:.1f} с, полос {len(win)}")
