#!/usr/bin/env python3
"""Exercise a distribution binary using disposable synthetic data only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary')
parser.add_argument('--without-memory', action='store_true',
                    help='Do not authorize or create operational memory; skip hook-resource repair')
args = parser.parse_args()
binary = Path(args.binary).resolve(strict=True)


def snapshot(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.rglob('*') if p.is_file() and not p.is_symlink()}


with tempfile.TemporaryDirectory(prefix='vrg-distribution-') as tmp:
    base = Path(tmp).resolve()
    home, project = base / 'home', base / 'project'
    home.mkdir(mode=0o700)
    project.mkdir(mode=0o700)
    codex_dir = project / '.codex'
    codex_dir.mkdir()
    unrelated = '# preserved comment\n[mcp_servers.unrelated]\ncommand = "/usr/bin/true"\n'
    (codex_dir / 'config.toml').write_text(unrelated)
    other_handler = {'type': 'command', 'command': '/usr/bin/true'}
    (codex_dir / 'hooks.json').write_text(json.dumps({'hooks': {'Stop': [{'hooks': [other_handler]}]}}))
    (project / 'AGENTS.md').write_text('# Existing project instructions\n')
    config = home / '.config' / 'vrg' / 'config.toml'
    data = base / 'derived'
    env = os.environ.copy()
    env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / '.config'),
               XDG_DATA_HOME=str(home / '.local' / 'share'),
               CODEX_HOME=str(home / '.codex'))
    for key in ('VRG_CONFIG', 'VRG_DATA_DIR'):
        env.pop(key, None)

    def run(*args, allowed=(0,)):
        result = subprocess.run([str(binary), '--config', str(config), *args],
                                cwd=project, env=env, capture_output=True,
                                text=True, timeout=180)
        if result.returncode not in allowed:
            raise RuntimeError(f'{args[0]} failed ({result.returncode}): {result.stderr[-2000:]}')
        return result.stdout

    run('setup', '--help')
    run('doctor', '--help')
    setup = ('setup', '--project', str(project), '--synthetic', '--data-dir', str(data),
             '--text-only', '--codex', '--allow-remote-context')
    if not args.without_memory:
        setup += ('--memory-dir', str(project / 'memory'))
    setup += ('--non-interactive', '--yes', '--offline')
    before = snapshot(base)
    run(*setup, '--dry-run')
    assert snapshot(base) == before, 'setup dry-run modified files'
    run(*setup)
    assert unrelated in (codex_dir / 'config.toml').read_text()
    assert '# Existing project instructions' in (project / 'AGENTS.md').read_text()
    hooks_json = json.loads((codex_dir / 'hooks.json').read_text())
    assert any(other_handler in group['hooks'] for group in hooks_json['hooks']['Stop'])
    configured = snapshot(base)
    run(*setup)
    assert snapshot(base) == configured, 'setup is not idempotent'
    run('doctor', '--fix', '--dry-run')
    assert snapshot(base) == configured, 'doctor fix dry-run modified files'
    run('doctor', '--fix')
    assert snapshot(base) == configured, 'healthy doctor fix modified files'
    observer = data / 'resources' / 'vrg-hook-observer-v1.py'
    if not args.without_memory:
        hooks = project / '.codex' / 'hooks.json'
        codex_config = project / '.codex' / 'config.toml'
        assert observer.is_file() and hooks.is_file() and codex_config.is_file()
        retained = {f: hashlib.sha256(f.read_bytes()).hexdigest() for f in (hooks, codex_config)}
        observer_hash = hashlib.sha256(observer.read_bytes()).hexdigest()
        observer.unlink()
        run('doctor', '--fix', '--dry-run', allowed=(2,))
        assert not observer.exists(), 'repair dry-run restored a resource'
        run('doctor', '--fix')
        assert hashlib.sha256(observer.read_bytes()).hexdigest() == observer_hash
        assert all(hashlib.sha256(f.read_bytes()).hexdigest() == h for f, h in retained.items())
        healed = snapshot(base)
        run('doctor', '--fix')
        assert snapshot(base) == healed, 'second repair was not a no-op'
    else:
        assert not observer.exists(), 'unexpected supervisor without operational memory'
        assert not (project / 'memory').exists(), 'unexpected operational memory directory'
        assert not any(p.name in {'STATUS.md', 'DECISIONS.md', 'SOLUTIONS.md'}
                       for p in base.rglob('*')), 'unexpected operational memory file'
    report = json.loads(run('doctor', '--json', '--check-runtime'))
    assert report['schema_version'] == 1
    assert any(c['id'] == 'mcp_runtime' and c['status'] == 'PASS' for c in report['checks']), 'direct MCP probe did not pass'
    assert any(c['id'] == 'codex_mcp' and c['status'] == 'PASS' for c in report['checks']), 'MCP profile mismatch'
    if not args.without_memory:
        assert any(c['id'] == 'supervisor_resource' and c['status'] == 'PASS' for c in report['checks']), 'missing packaged hook resource'
    assert any(c['id'] == 'codex_trust' and c['status'] == 'UNKNOWN' for c in report['checks']), 'trust must not be fabricated'
    sample = project / '.vrg-synthetic-notes' / 'sample.md'
    sample_before = hashlib.sha256(sample.read_bytes()).hexdigest()
    run('index')
    result = json.loads(run('search', 'isolated', '--text-only'))
    assert 'sample.md' in json.dumps(result), 'synthetic source was not retrieved'
    assert hashlib.sha256(sample.read_bytes()).hexdigest() == sample_before
    print('PASS: synthetic setup, idempotence, dry-run, doctor, direct MCP, index and retrieval')
    if args.without_memory:
        assert not any(p.name in {'STATUS.md', 'DECISIONS.md', 'SOLUTIONS.md'}
                       for p in base.rglob('*')), 'unexpected operational memory file'
        print('Scope: no operational memory was created; hook setup and resource repair were not tested')
    print('Scope: no real Codex event or hook trust was tested')
