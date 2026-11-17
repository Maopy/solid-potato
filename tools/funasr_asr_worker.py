"""Resident local FunASR Paraformer worker using the existing JSON-lines protocol."""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import site
import sys
import wave


def matches_speaker(model, reference, wav, torch, threshold=0.22):
    # Short clips have unstable embeddings; retain them rather than drop words.
    with wave.open(str(wav), 'rb') as audio:
        if audio.getnframes() < audio.getframerate() * 2:
            return True
    result = model.generate(input=str(wav), disable_pbar=True)
    embedding = result[0]['spk_embedding'].detach().cpu().flatten()
    similarities = [torch.nn.functional.cosine_similarity(embedding, ref, dim=0).item() for ref in reference]
    return max(similarities) >= threshold


def read_hotwords(path):
    if not path:
        return None
    file = Path(path)
    if not file.exists():
        return None
    if file.stat().st_size > 16384:
        raise ValueError("热词文件过大（最多 16 KB）")
    words = []
    for line in file.read_text(encoding="utf-8-sig").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if any(c.isspace() for c in line) or len(line) > 50:
            raise ValueError("每行填写一个热词，不能包含空格且最多 50 个字符")
        words.append(line)
        if len(words) > 100:
            raise ValueError("热词最多 100 个")
    return " ".join(words) if words else None


def main():
    sys.stdin.reconfigure(encoding="utf-8")
    sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--gpu", action="store_true")
    parser.add_argument("--vad", action="store_true")
    parser.add_argument("--punc-model")
    parser.add_argument("--hotwords-file")
    parser.add_argument("--speaker-model")
    parser.add_argument("--speaker-reference")
    args = parser.parse_args()
    runtime = Path(sys.executable).resolve().parent.parent
    site.addsitedir(str(runtime / "funasr-runtime" / "Lib" / "site-packages"))
    site.addsitedir(str(runtime / "funasr-packages"))
    transport = sys.stdout
    os.environ["HF_HUB_OFFLINE"] = "1"
    with contextlib.redirect_stdout(sys.stderr):
        import torch
        from funasr import AutoModel
        torch.set_num_threads(args.threads)
        if args.gpu and not torch.cuda.is_available():
            raise RuntimeError("FunASR CUDA 不可用")
        model = AutoModel(model=args.model, device="cuda" if args.gpu else "cpu",
                          disable_update=True, trust_remote_code=False)
        punc = (AutoModel(model=args.punc_model, device="cuda" if args.gpu else "cpu",
                          disable_update=True, trust_remote_code=False)
                if args.punc_model and not args.vad else None)
        speaker = None
        references = None
        if not args.vad and args.speaker_reference:
            model_dir = Path(args.speaker_model)
            if hashlib.sha256((model_dir / 'campplus_cn_common.bin').read_bytes()).hexdigest() != '3388cf5fd3493c9ac9c69851d8e7a8badcfb4f3dc631020c4961371646d5ada8':
                raise ValueError('FunASR 声纹模型校验失败')
            data = json.loads(Path(args.speaker_reference).read_text(encoding='utf-8'))
            references = [torch.tensor(row, dtype=torch.float32) for row in data['embeddings']]
            if not references or any(ref.shape != (192,) or not torch.isfinite(ref).all() for ref in references):
                raise ValueError('主播声纹参考资料无效')
            speaker = AutoModel(model=str(model_dir), device='cpu', disable_update=True, trust_remote_code=False)
    audio = bytearray()
    base = emitted = total = 0
    print(json.dumps({"ready": True, "lookahead_ms": 500}), file=transport, flush=True)
    for line in sys.stdin:
        try:
            request = json.loads(line)
            with contextlib.redirect_stdout(sys.stderr):
                if args.vad:
                    import numpy as np
                    pcm = bytes.fromhex(request.get("pcm_hex", ""))
                    if len(pcm) % 320 or len(pcm) > 16000:
                        raise ValueError("Invalid VAD audio")
                    audio.extend(pcm)
                    total += len(pcm) // 320
                    finish = request.get("finish", False)
                    stop = total if finish else max(emitted, total - 50)
                    decisions = []
                    if stop > emitted:
                        samples = np.frombuffer(audio, dtype="<i2").astype(np.float32) / 32768.0
                        result = model.generate(input=samples, disable_pbar=True,
                                                max_end_silence_time=250)
                        intervals = result[0].get("value", []) if result else []
                        for frame in range(emitted, stop):
                            ms = (frame - base) * 10
                            decisions.append(any(start < ms + 10 and end > ms for start, end in intervals))
                        emitted = stop
                        trim = max(0, emitted - 50 - base)
                        del audio[:trim * 320]
                        base += trim
                    response = {"speech": decisions}
                else:
                    keep = not speaker or matches_speaker(speaker, references, request['wav'], torch)
                    result = (model.generate(input=request["wav"], disable_pbar=True,
                                            hotword=read_hotwords(args.hotwords_file)) if keep else [])
                    text = result[0]["text"] if result else ""
                    if text and punc:
                        punctuated = punc.generate(input=text, disable_pbar=True)
                        text = punctuated[0]["text"] if punctuated else text
                    response = {"text": text}
        except Exception as error:
            response = {"error": str(error)}
        print(json.dumps(response, ensure_ascii=False), file=transport, flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(json.dumps({"error": str(error)}, ensure_ascii=True), flush=True)
        raise SystemExit(1)
