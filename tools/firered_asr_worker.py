"""Private JSON-lines transport for a resident local FireRedASR2 AED model."""
import argparse
import contextlib
import json
import sys
import traceback


def main():
    sys.stdin.reconfigure(encoding='utf-8')
    sys.stdout.reconfigure(encoding='utf-8')
    parser = argparse.ArgumentParser()
    parser.add_argument('--source', required=True)
    parser.add_argument('--model', required=True)
    parser.add_argument('--threads', type=int, default=4)
    parser.add_argument('--gpu', action='store_true')
    args = parser.parse_args()
    sys.path.insert(0, args.source)
    # Upstream dependencies sometimes print to stdout. Reserve it for framing.
    transport = sys.stdout
    with contextlib.redirect_stdout(sys.stderr):
        import torch
        from fireredasr2s.fireredasr2 import FireRedAsr2, FireRedAsr2Config
        torch.set_num_threads(args.threads)
        if args.gpu and not torch.cuda.is_available():
            raise RuntimeError('FireRed CUDA is unavailable')
        model = FireRedAsr2.from_pretrained('aed', args.model,
            FireRedAsr2Config(use_gpu=args.gpu, use_half=args.gpu, beam_size=3))
    print(json.dumps({'ready': True, 'gpu': args.gpu}), file=transport, flush=True)
    for line in sys.stdin:
        try:
            request = json.loads(line)
            with contextlib.redirect_stdout(sys.stderr):
                result = model.transcribe(['live'], [request['wav']])
            response = {'text': result[0]['text']}
        except Exception as error:
            response = {'error': str(error)}
        print(json.dumps(response, ensure_ascii=False), file=transport, flush=True)


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        # Startup failures must reach the panel instead of appearing as a broken pipe.
        print(json.dumps({'error': str(error)}, ensure_ascii=True), flush=True)
        traceback.print_exc(file=sys.stderr)
        raise SystemExit(1)
