"""onnx/fp32 → onnx/pii-tracer-int8.onnx: int8 с масштабом на канал; головы (37 меток и чувствительность) — fp32.

    .venv/Scripts/python quantize.py
"""
import sys
from pathlib import Path

import onnx
from onnx import numpy_helper
from onnxruntime.quantization import QuantType, quantize_dynamic

sys.stdout.reconfigure(encoding="utf-8")
FP32 = Path("onnx/fp32/pii-tracer.onnx")
OUT = Path("onnx/pii-tracer-int8.onnx")

model = onnx.load(str(FP32), load_external_data=False)
shapes = {i.name: list(i.dims) for i in model.graph.initializer}
heads = [n.name for n in model.graph.node if n.op_type in ("MatMul", "Gemm")
         and any(shapes.get(x, [0])[-1] in (37, 1) or shapes.get(x, [0])[0] in (37, 1) for x in n.input[1:2])]
print("в fp32 остаются:", heads)
quantize_dynamic(str(FP32), str(OUT), weight_type=QuantType.QInt8, per_channel=True,
                 op_types_to_quantize=["MatMul", "Gather"], nodes_to_exclude=heads)
print("int8:", round(OUT.stat().st_size / 2**20), "МБ")
