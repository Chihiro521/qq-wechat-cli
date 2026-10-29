"""Development comparison against the same successful snapshots; prints aggregates only.

Uses legacy Python solely as an independent reference, never as a shipped dependency.
The optional QQ rows file contains local source material and must stay under .local.
"""
import argparse
import ctypes
import json
import re
import sqlite3
import sys
from pathlib import Path

ap = argparse.ArgumentParser()
ap.add_argument('platform', choices=['qq', 'wechat'])
ap.add_argument('state', type=Path)
ap.add_argument('--legacy', type=Path)
ap.add_argument('--rows', type=Path)
args = ap.parse_args()
index = sqlite3.connect(args.state / 'index.sqlite')
profile = json.loads(index.execute('SELECT body FROM profiles WHERE platform=?', [args.platform]).fetchone()[0])
source_id = profile['sourceId']
total = index.execute('SELECT count(*) FROM messages WHERE source_id=?', [source_id]).fetchone()[0]
result = {'platform': args.platform, 'indexedMessages': total, 'rawMessages': 0, 'sampled': 0, 'identityMismatches': 0, 'textMismatches': 0, 'mismatchesByRawType': {}}
if args.platform == 'qq':
    library = ctypes.CDLL(str(Path(sys.base_prefix) / 'DLLs/sqlite3.dll'))
    control = library.sqlite3_test_control
    control.restype = ctypes.c_int
    state = profile['files']['messages']
    prior = control(11, ctypes.c_uint(state.get('pendingByte') or 0x40000000))
    source = sqlite3.connect(f"file:{Path(state['cache']).as_posix()}?mode=ro&immutable=1", uri=True)
    tables = [v[0] for v in source.execute("SELECT name FROM sqlite_master WHERE type='table'") if re.fullmatch(r'(group|buddy|system)_\d+', v[0])]
    samples = []
    for table in tables:
        kind, peer = table.split('_', 1)
        result['rawMessages'] += source.execute(f'SELECT count(*) FROM "{table}"').fetchone()[0]
        for row in source.execute(f'SELECT Time,Rand,SenderUin,MsgContent FROM "{table}" ORDER BY (Time & 4294967295) DESC,(Rand & 4294967295) DESC LIMIT 25'):
            samples.append({'kind': kind, 'peer': peer, 'Time': row[0], 'Rand': row[1], 'SenderUin': row[2], 'MsgContent': (row[3] or b'').hex()})
    source.close()
    control(11, ctypes.c_uint(prior))
    if args.rows:
        args.rows.parent.mkdir(parents=True, exist_ok=True)
        args.rows.write_text(json.dumps(samples), encoding='utf-8')
    result['sampled'] = len(samples)
else:
    assert args.legacy
    sys.path.insert(0, str(args.legacy))
    from wechat_cli.core.messages import decompress_content, _format_message_text
    contact = profile['files'].get('contact/contact.db')
    names = {}
    if contact:
        with sqlite3.connect(contact['cache']) as db:
            names = {u: remark or nick or u for u, nick, remark in db.execute('SELECT username,nick_name,remark FROM contact')}
    display = lambda username, name_map: name_map.get(username, username)
    for shard, state in profile['files'].items():
        if not re.fullmatch(r'message/message_\d+\.db', shard):
            continue
        with sqlite3.connect(state['cache']) as source:
            tables = [v[0] for v in source.execute("SELECT name FROM sqlite_master WHERE type='table'") if re.fullmatch(r'Msg_[a-fA-F0-9]{32}', v[0])]
            for table in tables:
                result['rawMessages'] += source.execute(f'SELECT count(*) FROM "{table}"').fetchone()[0]
                for row in source.execute(f'SELECT local_id,local_type,create_time,message_content,WCDB_CT_message_content FROM "{table}" ORDER BY create_time DESC LIMIT 25'):
                    local, kind, stamp, raw, compressed = row
                    original = f'{shard}:{table}:{local}'
                    alias = profile['account'].replace('%', '%25').replace(':', '%3A')
                    found = index.execute("SELECT body FROM messages WHERE id=?", [f'wechat:{alias}:{original}']).fetchone()
                    result['sampled'] += 1
                    if not found:
                        result['identityMismatches'] += 1
                        continue
                    m = json.loads(found[0])
                    result['identityMismatches'] += int(m['time'] != stamp or m['metadata']['rawType'] != kind)
                    # Plain text and stickers retain the legacy renderer verbatim.
                    # Other types are normalized into the new structured model.
                    if kind & 0xffffffff in (1, 47, 10000, 10002):
                        raw = decompress_content(raw, compressed)
                        _, expected = _format_message_text(local, kind, raw, m['conversationKind']=='group', m['conversationId'], names.get(m['conversationId'],m['conversationId']), names, display)
                        result['textMismatches'] += int(expected != m['text'])
                        if expected != m['text']:
                            k = str(kind)
                            result['mismatchesByRawType'][k] = result['mismatchesByRawType'].get(k, 0) + 1
index.close()
print(json.dumps(result))
