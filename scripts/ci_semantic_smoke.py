#!/usr/bin/env python3
"""Exercise real local E5 indexing and semantic retrieval on isolated data.

Usage: python3 scripts/ci_semantic_smoke.py /path/to/installed/vrg /path/to/model
The caller supplies the installed binary and the already-downloaded model. On
macOS Intel the verified ONNX Runtime dylib must be beside the executable.
This probe neither downloads dependencies nor tests Codex hook trust.
"""

import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile

ARTIFACTS = {
    'model.onnx': (470268510, 'ca456c06b3a9505ddfd9131408916dd79290368331e7d76bb621f1cba6bc8665'),
    'tokenizer.json': (17082730, '0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39'),
    'config.json': (653, 'bbb7c1333fc4b3e27fbc9cd5d2070aabcc1d4dfb99917c3633e772f97545a6b6'),
    'special_tokens_map.json': (167, 'd05497f1da52c5e09554c0cd874037a083e1dc1b9cfd48034d1c717f1afc07a7'),
    'tokenizer_config.json': (443, 'a1d6bc8734a6f635dc158508bef000f8e2e5a759c7d92f984b2c86e5ff53425b'),
}


def digest(path, combined=None):
    result = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            result.update(chunk)
            if combined is not None:
                combined.update(chunk)
    return result.hexdigest()


if len(sys.argv) != 3:
    sys.exit(f'Usage: {sys.argv[0]} INSTALLED_VRG VERIFIED_MODEL_DIR')
binary = Path(sys.argv[1]).resolve(strict=True)
model = Path(sys.argv[2]).resolve(strict=True)
if not binary.is_file() or not model.is_dir():
    raise RuntimeError('binary and model directory must exist')
fingerprint = hashlib.sha256()
for name, (size, expected) in ARTIFACTS.items():
    artifact = model / name
    if artifact.is_symlink() or not artifact.is_file() or artifact.stat().st_size != size:
        raise RuntimeError(f'invalid model artifact: {name}')
    if digest(artifact, fingerprint) != expected:
        raise RuntimeError(f'model artifact checksum mismatch: {name}')
model_id = 'intfloat/multilingual-e5-small#' + fingerprint.hexdigest()

with tempfile.TemporaryDirectory(prefix='vrg-semantic-') as tmp:
    base = Path(tmp).resolve()
    home, project, notes = (base / part for part in ('home', 'project', 'notes'))
    for directory in (home, project, notes):
        directory.mkdir(mode=0o700)
    target = notes / 'animals.md'
    target.write_text('# Domestic animals\nDogs are friendly household pets. They enjoy play and walks alongside people.\n', encoding='utf-8')
    (notes / 'astronomy.md').write_text('# Astronomy\nDistant galaxies contain stars, planets and cosmic dust.\n', encoding='utf-8')
    revision = digest(target)
    config = home / '.config' / 'vrg' / 'config.toml'
    data = base / 'derived'
    env = os.environ.copy()
    env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / '.config'),
               XDG_DATA_HOME=str(home / '.local' / 'share'), CODEX_HOME=str(home / '.codex'))
    for key in ('VRG_CONFIG', 'VRG_DATA_DIR', 'ORT_DYLIB_PATH'):
        env.pop(key, None)

    def run(*args):
        result = subprocess.run([str(binary), '--config', str(config), *args],
                                cwd=project, env=env, capture_output=True,
                                text=True, timeout=600)
        if result.returncode:
            raise RuntimeError(f'{args[0]} failed ({result.returncode}): {result.stderr[-2000:]}')
        return result.stdout

    run('setup', '--project', str(project), '--root', str(notes), '--data-dir', str(data),
        '--model-dir', str(model), '--cli-only', '--non-interactive', '--yes', '--offline')
    report = json.loads(run('index'))
    if report['indexed'] < 2 or report['semantic_status']['state'] != 'ready' or not report['generation_id']:
        raise RuntimeError(f'index did not produce a ready semantic generation: {report}')
    generation = report['generation_id']
    # Inspect only derived vector metadata, using a read-only SQLite connection.
    uri = (data / 'vrg.sqlite3').as_uri() + '?mode=ro'
    with sqlite3.connect(uri, uri=True) as db:
        count = db.execute('SELECT count(*) FROM vectors WHERE generation_id = ? AND model_id = ? AND dimensions = 384 AND length(vector_data) = 1536',
                           (generation, model_id)).fetchone()[0]
    if count < 2:
        raise RuntimeError('the indexed generation is missing E5 vectors')
    query = 'canine companion'
    text = json.loads(run('search', query, '--text-only'))
    if text['hits']:
        raise RuntimeError('text-only control unexpectedly matched the paraphrase')
    found = json.loads(run('search', query))
    if (found['mode'] != 'hybrid' or found['degradation_reason'] is not None
            or found['sync_state'] != 'ready' or found['generation_id'] != generation
            or not found['hits']):
        raise RuntimeError('semantic search degraded or did not return the indexed generation')
    hit = found['hits'][0]
    if (hit['source']['relative_path'] != target.name or hit['revision'] != revision
            or 'semantic' not in hit['matches']):
        raise RuntimeError('the highest-ranked result is not the expected semantic source and revision')
    if digest(target) != revision:
        raise RuntimeError('synthetic source was modified')
    print(f'PASS: verified E5 artifacts, {count} vectors, semantic query and attributed retrieval')
    print('Scope: synthetic direct CLI probe; no Codex session or hook trust was tested')
