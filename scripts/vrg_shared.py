#!/usr/bin/env python3
"""Authenticated headers and native CAS notes for the opt-in shared VRG runtime."""
import hashlib
import hmac
import json
import os
from pathlib import Path
import secrets
import stat
import sys
import urllib.error
import urllib.request

from vrg_global import REGISTRY, checked_profile, load_registry, resolve

MANIFEST = Path.home() / '.local/state/vrg-auto/manifest.json'


def sha(data):
    return hashlib.sha256(data).hexdigest()


def private(path):
    path = Path(path)
    info = path.lstat()
    if (not path.is_absolute() or path.resolve(strict=True) != path
            or not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
            or info.st_mode & 0o077 or info.st_nlink != 1):
        raise ValueError('private canonical metadata required')
    return path.read_bytes()


def manifest():
    data = json.loads(private(MANIFEST))
    registry = load_registry()
    if (data.get('auto_registry_path') != str(REGISTRY)
            or data.get('auto_discovery_root') != registry['discovery_roots'][0]
            or len(registry['discovery_roots']) != 1
            or not isinstance(data.get('port'), int)):
        raise ValueError('shared runtime scope changed')
    return data


def identity(data, registry):
    nonce = secrets.token_hex(32)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(f'http://127.0.0.1:{data["port"]}/identity?nonce={nonce}', timeout=5) as response:
        result = json.load(response)
    expected = hmac.new(data['identity_key'].encode(), nonce.encode(), hashlib.sha256).hexdigest()
    pid = result.get('pid')
    if (not isinstance(pid, int) or not hmac.compare_digest(result.get('proof', ''), expected)
            or (Path('/proc') / str(pid) / 'exe').resolve(strict=True) != Path(registry['binary'])
            or sha(Path(registry['binary']).read_bytes()) != registry['binary_sha256']):
        raise ValueError('shared runtime identity changed')


def binding(data, channel, entry):
    if channel not in ('memory', 'sources'):
        raise ValueError('unknown channel')
    if not entry.get('auto_enrolled'):
        candidates = [item for item in data['bindings'] if item['channel'] == channel]
        if len(candidates) != 1 or sha(private(candidates[0]['config'])) != candidates[0]['config_sha256']:
            raise ValueError('unconfigured binding changed')
        return candidates[0]
    checked_profile(entry)
    profile = entry['profile'] if channel == 'memory' else entry['source']['config_path']
    digest = sha(private(profile))
    message = f'vrg-auto-binding\0{channel}\0{entry["project"]}\0{digest}'.encode()
    token = hmac.new(data['identity_key'].encode(), message, hashlib.sha256).hexdigest()
    return {'token': token, 'managed_note_writes': channel == 'memory' and entry['managed_note_writes']}


def note_bytes(entry, name):
    if (name not in ('STATUS.md', 'DECISIONS.md', 'SOLUTIONS.md')
            or not entry.get('memory') or name not in entry['allowed_files']):
        raise ValueError('note outside authorized memory')
    root = Path(entry['memory'])
    if root.resolve(strict=True) != root:
        raise ValueError('memory root changed')
    fd = os.open(root / name, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, 'rb') as handle:
        info = os.fstat(handle.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_nlink != 1 or info.st_size > 1_000_000):
            raise ValueError('note is not a bounded owned file')
        return handle.read(1_000_001)


def managed_note(action, name, args):
    entry = resolve()
    data = manifest()
    registry = load_registry()
    identity(data, registry)
    selected = binding(data, 'memory', entry)
    if not selected.get('managed_note_writes'):
        raise ValueError('managed note writes are not enrolled')
    before = note_bytes(entry, name)
    if action == 'append' and not args:
        snippet = sys.stdin.buffer.read(1_000_001)
        if not snippet or len(snippet) > 1_000_000:
            raise ValueError('append text empty or too large')
        snippet.decode('utf-8')
    elif action == 'apply' and len(args) == 3:
        expected, operation_id, input_path = args
        if (len(expected) != 64 or len(operation_id) != 64
                or not all(char in '0123456789abcdef' for char in expected + operation_id)):
            raise ValueError('expected revision or operation ID is invalid')
        proposed = private(input_path)
        if len(proposed) > 1_000_000:
            raise ValueError('proposed note too large')
        proposed.decode('utf-8')
    else:
        raise ValueError('usage: note append NAME or note apply NAME EXPECTED_SHA OP_ID PRIVATE_FILE')
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    for attempt in range(3 if action == 'append' else 1):
        if action == 'append':
            if attempt:
                before = note_bytes(entry, name)
            expected = sha(before)
            proposed = before + snippet
            operation_id = secrets.token_hex(32)
        payload = json.dumps({'relative_path': name, 'expected_revision': expected,
                              'proposed_markdown': proposed.decode('utf-8'),
                              'operation_id': operation_id}, ensure_ascii=False).encode()
        request = urllib.request.Request(f'http://127.0.0.1:{data["port"]}/note/apply',
                                         data=payload, method='POST',
                                         headers={'Authorization': 'Bearer ' + selected['token'],
                                                  'Content-Type': 'application/json'})
        try:
            with opener.open(request, timeout=30) as response:
                print(response.read(4096).decode())
                return
        except urllib.error.HTTPError as error:
            if error.code != 409:
                raise ValueError('managed note write rejected') from None
            conflict = json.loads(error.read(4096))
            if conflict.get('error') != 'REVISION_CONFLICT':
                raise ValueError('invalid CAS conflict response')
            if action == 'apply' or attempt == 2:
                print(json.dumps({'error': 'REVISION_CONFLICT',
                                  'current_revision': conflict.get('current_revision')}))
                raise SystemExit(75)


def main():
    args = sys.argv[1:]
    if len(args) == 2 and args[0] == 'headers':
        entry = resolve()
        data = manifest()
        identity(data, load_registry())
        selected = binding(data, args[1], entry)
        print(json.dumps({'Authorization': 'Bearer ' + selected['token']}))
    elif len(args) >= 3 and args[:2] == ['note', 'append'] and len(args) == 3:
        managed_note('append', args[2], [])
    elif len(args) == 6 and args[:2] == ['note', 'apply']:
        managed_note('apply', args[2], args[3:])
    else:
        raise ValueError('unknown operation')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, urllib.error.URLError) as error:
        print('VRG shared runtime: ' + str(error), file=sys.stderr)
        raise SystemExit(78)
