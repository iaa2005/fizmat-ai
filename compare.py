"""Сравнение onnx-движка с исходной моделью на примерах run.py: совпадение полос и время."""
import sys, time
from pathlib import Path
sys.stdout.reconfigure(encoding="utf-8")
sys.path.insert(0, "guard")
import torch
from transformers import AutoModel
from engine import Engine
from run import SAMPLES

file = sys.argv[1] if len(sys.argv) > 1 else "pii-tracer-int8.onnx"
ref = AutoModel.from_pretrained("model", trust_remote_code=True, dtype=torch.float32).eval()
eng = Engine(Path("onnx"), file=file)
same = total = 0; ta = tb = 0
for name, text, _ in SAMPLES:
    t = time.perf_counter(); a = {(s.start, s.end, s.label) for s in ref.predict(text)[0]}; ta += time.perf_counter() - t
    t = time.perf_counter(); b = {(s.start, s.end, s.label) for s in eng.predict(text)[0]}; tb += time.perf_counter() - t
    same += len(a & b); total += len(a | b)
    for x in a ^ b: print("  расхождение", name, "torch" if x in a else "onnx", text[x[0]:x[1]], x[2])
print(f"совпало {same} из {total}; torch {ta*1000:.0f} мс, onnx {tb*1000:.0f} мс")
