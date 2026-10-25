"""Resume a verified public FireRed checkpoint via bounded HTTP ranges."""
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import time
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
DEST = ROOT / 'dist/.live-archive-models/FireRedASR2-AED'
REVISION = '2304afed56eacfee6256dee5937ed22ffa0b64ec'
SIZE = 4731558506
SHA256 = '4677cbd30988d63ed3e777f6a42a1e5260a3865317f6e15e488bef40954f7054'
BLOCK = 4 * 1024 * 1024


def main():
    DEST.mkdir(parents=True, exist_ok=True)
    filename = globals().get('FILENAME', 'model.pth.tar')
    target = DEST / (filename + '.partial')
    marks = target.with_suffix('.ranges.json')
    complete = set(json.loads(marks.read_text()) if marks.exists() and target.exists() else [])
    with target.open('r+b' if target.exists() else 'w+b') as stream:
        stream.truncate(SIZE)
    url = globals().get('URL', f'https://huggingface.co/FireRedTeam/FireRedASR2-AED/resolve/{REVISION}/model.pth.tar')

    def download(index):
        start, end = index * BLOCK, min((index + 1) * BLOCK, SIZE) - 1
        for attempt in range(8):
            try:
                request = urllib.request.Request(url, headers={'Range': f'bytes={start}-{end}'})
                with urllib.request.urlopen(request, timeout=60) as response:
                    if response.status != 206 or response.headers.get('Content-Range') != f'bytes {start}-{end}/{SIZE}':
                        raise ValueError('invalid HTTP range')
                    data = response.read()
                if len(data) != end - start + 1:
                    raise ValueError('truncated HTTP range')
                with target.open('r+b') as stream:
                    stream.seek(start)
                    stream.write(data)
                return index
            except Exception:
                if attempt == 7:
                    raise
                time.sleep(min(2 ** attempt, 10))

    total = (SIZE + BLOCK - 1) // BLOCK
    with concurrent.futures.ThreadPoolExecutor(max_workers=12) as pool:
        futures = [pool.submit(download, i) for i in range(total) if i not in complete]
        for future in concurrent.futures.as_completed(futures):
            complete.add(future.result())
            temporary = marks.with_suffix('.tmp')
            temporary.write_text(json.dumps(sorted(complete)))
            os.replace(temporary, marks)
            if len(complete) % 50 == 0:
                print(f'{len(complete)}/{total} ranges', flush=True)
    digest = hashlib.file_digest(target.open('rb'), 'sha256').hexdigest()
    if digest != SHA256:
        raise ValueError('FireRed model SHA-256 mismatch')
    target.replace(DEST / filename)
    print(f'Verified {filename} {SIZE} bytes SHA256 {digest}', flush=True)


if __name__ == '__main__':
    main()
