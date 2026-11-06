#!/usr/bin/env python3
"""Prepare an opt-in, single-root VRG auto-project installation for review."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import stat
import sys

SCRIPT_NAMES = ('vrg_autoproject.py', 'vrg_global.py', 'vrg_global_hook.py',
                'vrg_sources.py', 'vrg_shared.py', 'vrg_hook_observer.py')


def sha(data):
    return hashlib.sha256(data).hexdigest()


def owned_directory(path):
    path = Path(path)
    info = path.lstat()
    if (not path.is_absolute() or path.resolve(strict=True) != path
            or not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()):
        raise ValueError('directory must be owned, absolute and canonical: ' + str(path))
    return path


def mkdir_private(path):
    path.mkdir(parents=True, mode=0o700, exist_ok=True)
    info = path.lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
            or info.st_mode & 0o077 or path.resolve(strict=True) != path):
        raise ValueError('private installation directory changed: ' + str(path))


def create(path, data, mode=0o600):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    with os.fdopen(fd, 'wb') as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())


def encoded(data):
    return (json.dumps(data, ensure_ascii=False, indent=2) + '\n').encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--discovery-root', type=Path, required=True,
                        help='one explicitly authorized parent of new projects')
    parser.add_argument('--model-dir', type=Path, required=True,
                        help='existing local multilingual-e5-small files')
    parser.add_argument('--binary', type=Path, required=True,
                        help='installed vrg 0.7.0-rc.1 executable')
    parser.add_argument('--port', type=int, required=True,
                        help='unused loopback port for the single shared runtime')
    args = parser.parse_args()
    home = owned_directory(Path.home())
    root = owned_directory(args.discovery_root)
    model = owned_directory(args.model_dir)
    binary = args.binary.resolve(strict=True)
    if (binary != args.binary or not binary.is_file() or not os.access(binary, os.X_OK)
            or binary.stat().st_uid != os.getuid()):
        raise ValueError('binary must be owned, executable, absolute and canonical')
    if not 1024 <= args.port <= 65535:
        raise ValueError('port must be an unprivileged TCP port')
    for name in ('model.onnx', 'tokenizer.json', 'config.json',
                 'special_tokens_map.json', 'tokenizer_config.json'):
        if not (model / name).is_file():
            raise ValueError('local model is incomplete: ' + name)
    config_dir = home / '.config/vrg'
    profiles = config_dir / 'auto-profiles'
    state = home / '.local/state/vrg-auto'
    scripts = home / '.local/lib/vrg-auto'
    data_base = home / '.local/share/vrg-auto'
    registry = config_dir / 'auto-projects.json'
    if registry.exists() or (state / 'manifest.json').exists():
        raise ValueError('auto-project installation already exists; no overwrite performed')
    for directory in (config_dir, profiles, state, state / 'sessions',
                      state / 'hook-observations', state / 'hook-observations/project-discovery',
                      scripts, data_base):
        mkdir_private(directory)
    source_dir = Path(__file__).resolve().parent
    for name in SCRIPT_NAMES:
        raw = (source_dir / name).read_bytes()
        create(scripts / name, raw, 0o700)
    schema = source_dir.parent / 'schemas/MEMORY_PROPOSAL.schema.json'
    create(config_dir / schema.name, schema.read_bytes())
    create(state / 'memory-migration.lock', b'')
    static_bindings = []
    for channel in ('memory', 'sources'):
        store = data_base / ('empty-' + channel)
        mkdir_private(store)
        config = profiles / ('empty-' + channel + '.toml')
        raw = ('roots=[]\ndata_dir=' + json.dumps(str(store))
               + '\ncapture_session_events=false\n[adaptive_memory]\nenabled=false\n').encode()
        create(config, raw)
        static_bindings.append({'channel': channel, 'project': str(home),
                                'config': str(config), 'config_sha256': sha(raw),
                                'token': secrets.token_hex(32), 'managed_note_writes': False})
    details = {'schema_version': 1, 'binary': str(binary),
               'binary_sha256': sha(binary.read_bytes()), 'data_base': str(data_base),
               'profiles_dir': str(profiles), 'state_dir': str(state),
               'model_dir': str(model), 'discovery_roots': [str(root)],
               'sources_launcher_sha256': sha((scripts / 'vrg_sources.py').read_bytes()),
               'projects': []}
    manifest = {'port': args.port, 'identity_key': secrets.token_hex(32),
                'auto_registry_path': str(registry), 'auto_discovery_root': str(root),
                'bindings': static_bindings}
    create(registry, encoded(details))
    create(state / 'manifest.json', encoded(manifest))
    python = sys.executable
    snippet = (f'[mcp_servers.vrg]\nurl = "http://127.0.0.1:{args.port}/memory"\n'
               f'http_headers_helper = {json.dumps(python + " " + str(scripts / "vrg_shared.py") + " headers memory")}\n'
               f'startup_timeout_sec = 60.0\n\n[mcp_servers.vrg_sources]\n'
               f'url = "http://127.0.0.1:{args.port}/sources"\n'
               f'http_headers_helper = {json.dumps(python + " " + str(scripts / "vrg_shared.py") + " headers sources")}\n'
               'startup_timeout_sec = 60.0\n')
    create(state / 'codex-mcp.fragment.toml', snippet.encode())
    hooks = {'hooks': {event: [{'hooks': [{'type': 'command',
             'command': f'{python} {scripts / "vrg_global_hook.py"} {event}', 'timeout': 600}]}]
             for event in ('SessionStart', 'UserPromptSubmit', 'Stop')}}
    create(state / 'codex-hooks.fragment.json', encoded(hooks))
    print(json.dumps({'registry': str(registry), 'manifest': str(state / 'manifest.json'),
                      'server_command': [str(binary), 'mcp', 'shared', str(state / 'manifest.json')],
                      'codex_mcp_fragment': str(state / 'codex-mcp.fragment.toml'),
                      'codex_hooks_fragment': str(state / 'codex-hooks.fragment.json'),
                      'discovery_root': str(root)}, indent=2))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError) as error:
        print('VRG auto-project setup: ' + str(error), file=sys.stderr)
        raise SystemExit(78)
