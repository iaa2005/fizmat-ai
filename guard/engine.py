"""PII-Tracer на onnxruntime: токенизатор, модель, ограниченный BIOES-Витерби. Без torch.

Повторяет modeling_pii_masking.py от Perplexity: токены без BOS/EOS, 37 меток на токен,
Витерби с разрешёнными переходами и сдвигами b_bias/e_bias, полосы по символам.
На fp32-модели совпадает с исходной 24 из 24 (compare в README).
"""
import json
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import onnxruntime as ort
from tokenizers import Tokenizer

MAX_TOKENS = 4096
MODEL_FILE = "pii-tracer-int8.onnx"


@dataclass
class Span:
    start: int
    end: int
    label: str
    score: float


class Engine:
    def __init__(self, folder: Path, threads: int = 0, file: str = MODEL_FILE):
        opts = ort.SessionOptions()
        if threads:
            opts.intra_op_num_threads = threads
        self.session = ort.InferenceSession(str(folder / file), opts, providers=["CPUExecutionProvider"])
        self.tokenizer = Tokenizer.from_file(str(folder / "tokenizer.json"))
        self.tokenizer.no_padding()
        self.tokenizer.no_truncation()
        conf = json.loads((folder / "viterbi.json").read_text("utf-8"))
        self.labels = conf["labels"]
        self.trans, self.start_ok, self.end_ok = self._transitions(conf["labels"], conf["b_bias"], conf["e_bias"])

    @staticmethod
    def _transitions(labels, b_bias, e_bias):
        n, idx = len(labels), {l: i for i, l in enumerate(labels)}
        types = [l[2:] for l in labels if l.startswith("S-")]
        ok = np.zeros((n, n), dtype=bool)
        ends, begins = {idx["O"]}, {idx["O"]}
        for t in types:
            b, i, e, s = idx[f"B-{t}"], idx[f"I-{t}"], idx[f"E-{t}"], idx[f"S-{t}"]
            ends |= {e, s}
            begins |= {b, s}
            ok[b, i] = ok[b, e] = ok[i, i] = ok[i, e] = True
        for f in ends:
            for t in begins:
                ok[f, t] = True
        trans = np.where(ok, 0.0, -np.inf)
        for t in types:
            b, e = idx[f"B-{t}"], idx[f"E-{t}"]
            trans[:, b] = np.where(ok[:, b], trans[:, b] + b_bias, -np.inf)
            trans[e, :] = np.where(ok[e, :], trans[e, :] + e_bias, -np.inf)
        start_ok = np.array([0.0 if (l == "O" or l[:2] in ("B-", "S-")) else -np.inf for l in labels])
        end_ok = np.array([0.0 if (l == "O" or l[:2] in ("E-", "S-")) else -np.inf for l in labels])
        return trans, start_ok, end_ok

    def encode(self, text: str):
        return self.tokenizer.encode(text, add_special_tokens=False)

    def predict(self, text: str) -> tuple[list[Span], float]:
        enc = self.encode(text)
        ids, offs = enc.ids[:MAX_TOKENS], enc.offsets[:MAX_TOKENS]
        if not ids:
            return [], 0.0
        logits, sens = self.session.run(None, {
            "input_ids": np.array([ids], dtype=np.int64),
            "attention_mask": np.ones((1, len(ids)), dtype=np.int64),
        })
        return self._decode(logits[0].astype(np.float64), offs, text), float(1 / (1 + np.exp(-sens[0])))

    def _decode(self, logits: np.ndarray, offs, text: str) -> list[Span]:
        T, C = logits.shape
        score = logits[0] + self.start_ok
        back = np.zeros((T, C), dtype=np.int64)
        for t in range(1, T):
            cand = score[:, None] + self.trans          # [prev, next]
            back[t] = cand.argmax(0)
            score = logits[t] + cand.max(0)
        path = [int((score + self.end_ok).argmax())]
        for t in range(T - 1, 0, -1):
            path.append(int(back[t, path[-1]]))
        path.reverse()

        sig = lambda x: float(1 / (1 + np.exp(-x)))
        spans, t = [], 0
        while t < T:
            lab = self.labels[path[t]]
            a, b = offs[t]
            if lab.startswith("S-"):
                if a < b:
                    spans.append(Span(a, b, lab[2:], sig(logits[t, path[t]])))
                t += 1
            elif lab.startswith("B-"):
                kind, end, scores = lab[2:], b, [logits[t, path[t]]]
                t += 1
                while t < T:
                    inner = self.labels[path[t]]
                    if inner not in (f"I-{kind}", f"E-{kind}"):
                        break
                    end = offs[t][1]
                    scores.append(logits[t, path[t]])
                    t += 1
                    if inner.startswith("E-"):
                        break
                if a < end:
                    spans.append(Span(a, end, kind, sig(float(np.mean(scores)))))
            else:
                t += 1
        # Пробел перед словом токенизатор приклеивает к токену — обрезаем, как в оригинале.
        out = []
        for s in spans:
            a, b = s.start, s.end
            while a < b and text[a] in " \t\n":
                a += 1
            while b > a and text[b - 1] in " \t\n":
                b -= 1
            if a < b:
                out.append(Span(a, b, s.label, s.score))
        return out
