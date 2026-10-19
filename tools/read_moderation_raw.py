#!/usr/bin/env python3
"""Read local raw-message captures without modifying the journal."""
import argparse
import base64
import json
import sqlite3
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('journal', type=Path)
parser.add_argument('--run', help='run ID; defaults to the latest raw capture run')
parser.add_argument('--contains', help='match decoded JSON text (e.g. a public nickname)')
parser.add_argument('--limit', type=int, default=20, help='latest matching captures, maximum 500')
args = parser.parse_args()
if not 1 <= args.limit <= 500:
    parser.error('--limit must be between 1 and 500')
connection = sqlite3.connect(args.journal.resolve().as_uri() + '?mode=ro', uri=True)
connection.execute('BEGIN')  # Consistent DB/WAL snapshot while the writer runs.
run = args.run
if not run:
    row = connection.execute('SELECT run_id FROM raw_messages ORDER BY rowid DESC LIMIT 1').fetchone()
    run = row[0] if row else None
if run:
    captures = connection.execute('SELECT connection_id,capture_id FROM raw_messages WHERE run_id=? GROUP BY connection_id,capture_id ORDER BY max(rowid) DESC', (run,))
    emitted = 0
    for connection_id, capture_id in captures:
        rows = connection.execute('SELECT captured_at_ms,channel,category,encoding,part,parts,data FROM raw_messages WHERE run_id=? AND connection_id=? AND capture_id=? ORDER BY part', (run, connection_id, capture_id)).fetchall()
        at, channel, category, encoding, _, parts, _ = rows[0]
        complete = len(rows) == parts and [r[4] for r in rows] == list(range(parts))
        record = dict(run_id=run, connection_id=connection_id, capture_id=capture_id,
                      captured_at_ms=at, channel=channel, category=category, encoding=encoding, complete=complete)
        if not complete:
            record['error'] = 'incomplete_capture'
        else:
            data = ''.join(r[6] for r in rows)
            if encoding == 'json_utf8_base64':
                try:
                    record['message'] = json.loads(base64.b64decode(data, validate=True).decode('utf-8'))
                except (ValueError, UnicodeError):
                    record['error'] = 'invalid_encoding'
            elif encoding == 'gap':
                record['gap'] = data
            else:
                record['data_base64'] = data
        text = json.dumps(record, ensure_ascii=False)
        if args.contains and args.contains not in text:
            continue
        print(text)
        emitted += 1
        if emitted >= args.limit:
            break
connection.close()
