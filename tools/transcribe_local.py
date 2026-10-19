#!/usr/bin/env python3
"""Offline development transcription; never sends audio to a remote service."""
import argparse
import hashlib
import json
import math
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def load_config(path):
    config = json.loads(path.read_text())
    if config.get('provider') != 'whisper_cpp':
        raise ValueError('this development runner supports only whisper_cpp')
    model = Path(config['model'])
    if not model.is_absolute():
        model = path.parent / model
    if not model.is_file():
        raise ValueError(f'model not installed: {model}')
    expected = config['model_sha256']
    digest = hashlib.sha256()
    with model.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    if digest.hexdigest() != expected:
        raise ValueError('model SHA-256 mismatch; refusing to load it')
    config['model'] = str(model.resolve())
    if not isinstance(config['threads'], int) or not 1 <= config['threads'] <= 32:
        raise ValueError('threads must be 1..32')
    for key in ('executable', 'ffmpeg'):
        if not shutil.which(config[key]):
            raise ValueError(f'executable not found: {config[key]}')
    return config


def normalize_segments(result, start_seconds):
    """Keep model segment boundaries and source-relative times for later analysis."""
    segments = []
    for item in result.get('transcription', []):
        text = item.get('text', '').strip()
        offsets = item.get('offsets', {})
        begin, end = offsets.get('from'), offsets.get('to')
        if not text:
            continue
        if not isinstance(begin, (int, float)) or not isinstance(end, (int, float)):
            raise ValueError('recognizer returned a segment without numeric offsets')
        if not math.isfinite(begin) or not math.isfinite(end) or begin < 0 or end < begin:
            raise ValueError('recognizer returned invalid timestamps')
        segments.append({
            'id': len(segments) + 1,
            'start_ms': round(start_seconds * 1000 + begin),
            'end_ms': round(start_seconds * 1000 + end),
            'text': text,
            'is_final': True,
            'boundary': 'asr_segment',
            'provider': 'whisper_cpp',
        })
    return segments


def transcribe(config, source, output, start, duration):
    if not source.is_file():
        raise ValueError(f'input file not found: {source}')
    output.mkdir(parents=True, exist_ok=False)
    began = time.monotonic()
    with tempfile.TemporaryDirectory(prefix='live-archive-asr-') as temporary:
        wav = Path(temporary) / 'audio.wav'
        subprocess.run([
            config['ffmpeg'], '-nostdin', '-v', 'error', '-ss', str(start),
            '-i', str(source), '-t', str(duration), '-vn', '-ac', '1', '-ar', '16000',
            '-c:a', 'pcm_s16le', str(wav),
        ], check=True)
        command = [
            config['executable'], '-m', config['model'], '-f', str(wav),
            '-l', config['language'], '-t', str(config['threads']),
            '-oj', '-osrt', '-otxt', '-of', str(output / 'transcript'),
        ]
        if not config.get('use_gpu', True):
            command.append('-ng')
        inference_started = time.monotonic()
        with (output / 'engine.log').open('w') as log:
            subprocess.run(command, stdin=subprocess.DEVNULL, stdout=log, stderr=log, check=True)
        inference_seconds = time.monotonic() - inference_started
    raw = json.loads((output / 'transcript.json').read_text())
    segments = normalize_segments(raw, start)
    with (output / 'segments.jsonl').open('w') as stream:
        for segment in segments:
            stream.write(json.dumps(segment, ensure_ascii=False) + '\n')
    report = {
        'provider': config['provider'], 'model': config['model'],
        'language': config['language'], 'threads': config['threads'],
        'use_gpu': config.get('use_gpu', True), 'source': str(source),
        'source_start_seconds': start, 'requested_duration_seconds': duration,
        'inference_seconds': round(inference_seconds, 3),
        'total_seconds': round(time.monotonic() - began, 3),
        'segments': len(segments),
        'timestamps': 'segments.jsonl uses source-relative time; SRT uses clip-relative time',
    }
    (output / 'run.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({'output': str(output), **report}, ensure_ascii=False, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input', type=Path)
    parser.add_argument('--config', type=Path, default=ROOT / 'asr.dev.json')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--start', type=float, default=0)
    parser.add_argument('--duration', type=float, default=30)
    parser.add_argument('--cpu', action='store_true', help='disable Metal for CPU-only experiments')
    args = parser.parse_args()
    if not math.isfinite(args.start) or args.start < 0 or not math.isfinite(args.duration) or args.duration <= 0:
        parser.error('start must be nonnegative and duration must be positive')
    config = load_config(args.config.resolve())
    if args.cpu:
        config['use_gpu'] = False
    stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')
    output = (args.output or ROOT / 'data' / 'asr-dev' / stamp).resolve()
    transcribe(config, args.input.resolve(), output, args.start, args.duration)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f'local ASR: {error}', file=sys.stderr)
        sys.exit(1)
