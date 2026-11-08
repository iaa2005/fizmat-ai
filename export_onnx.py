"""PII-Tracer → ONNX fp32. Программе хватает onnxruntime.

    .venv/Scripts/python export_onnx.py
Итог в onnx/: fp32/, tokenizer.json, viterbi.json, LICENSE; затем quantize.py → pii-tracer-int8.onnx.
"""
import json
import shutil
import sys
from pathlib import Path

import onnx
import torch
from transformers import AutoModel

sys.stdout.reconfigure(encoding="utf-8")
OUT = Path("onnx")
FP32 = OUT / "fp32" / "pii-tracer.onnx"
FP32.parent.mkdir(parents=True, exist_ok=True)

model = AutoModel.from_pretrained("model", trust_remote_code=True, dtype=torch.float32,
                                  attn_implementation="eager").eval()


class Wrap(torch.nn.Module):
    def __init__(self, m):
        super().__init__()
        self.m = m

    def forward(self, input_ids, attention_mask):
        out = self.m(input_ids=input_ids, attention_mask=attention_mask)
        return out.logits, out.sensitivity_logits


tok = model.tokenizer("Привет, я Анна Корнилова, +7 916 555-12-34", return_tensors="pt")
with torch.no_grad():
    torch.onnx.export(Wrap(model), (tok["input_ids"], tok["attention_mask"]), str(FP32),
                      input_names=["input_ids", "attention_mask"], output_names=["logits", "sensitivity"],
                      dynamic_axes={"input_ids": {0: "b", 1: "t"}, "attention_mask": {0: "b", 1: "t"},
                                    "logits": {0: "b", 1: "t"}, "sensitivity": {0: "b"}},
                      opset_version=17, dynamo=False, external_data=True)
print("fp32 готов")

shutil.copy("model/tokenizer.json", OUT / "tokenizer.json")
shutil.copy("model/LICENSE", OUT / "LICENSE")
v = model.viterbi
json.dump({"labels": v.labels, "b_bias": float(v.b_bias), "e_bias": float(v.e_bias)},
          open(OUT / "viterbi.json", "w", encoding="utf-8"), ensure_ascii=False)
print("готово")
