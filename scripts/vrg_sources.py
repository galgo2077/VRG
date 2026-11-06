#!/usr/bin/env python3
"""Select a newly enrolled project's authorized source profile by cwd."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import sys
import tomllib

from vrg_global import load_registry


def regular(path):
    path = Path(path)
    info = path.lstat()
    if (not path.is_absolute() or path.resolve(strict=True) != path
            or not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
            or info.st_mode & 0o077 or info.st_nlink != 1):
        raise ValueError('private canonical metadata required')
    return path.read_bytes()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def load():
    registry = load_registry()
    if registry.get('sources_launcher_sha256') != sha(regular(Path(__file__).resolve())):
        raise ValueError('source launcher pin changed')
    if registry.get('binary_sha256') != sha(Path(registry['binary']).read_bytes()):
        raise ValueError('VRG executable pin changed')
    roots = {str(Path(path).resolve(strict=True)) for path in registry['discovery_roots']}
    profiles = []
    for entry in registry['projects']:
        if not entry.get('auto_enrolled'):
            continue
        project = Path(entry['project'])
        source = entry['source']
        if (not project.is_dir() or project.resolve(strict=True) != project
                or str(project.parent) not in roots
                or source['project_root'] != str(project)
                or source['config_sha256'] != sha(regular(source['config_path']))
                or source['policy_sha256'] != sha(regular(source['policy_path']))):
            raise ValueError('automatic source authorization changed')
        profiles.append(source)
    return registry, profiles


def select(profiles, cwd=None):
    here = Path(cwd or os.getcwd()).resolve(strict=True)
    matches = [profile for profile in profiles
               if here.is_relative_to(Path(profile['project_root']))]
    if len(matches) != 1:
        raise ValueError('cwd has no authorized canonical source profile')
    return matches[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['resolve', 'sync', 'index', 'status', 'search', 'graph', 'mcp'])
    parser.add_argument('args', nargs=argparse.REMAINDER)
    arguments = parser.parse_args()
    if any(value == '--config' or value.startswith('--config=') or value == '--data-dir'
           or value.startswith('--data-dir=') for value in arguments.args):
        raise ValueError('profile overrides prohibited')
    registry, profiles = load()
    profile = select(profiles)
    raw = regular(profile['config_path'])
    config = tomllib.loads(raw.decode())
    if (config['data_dir'] != profile['data_dir']
            or config['roots'] != [{'id': profile['root_id'], 'path': profile['corpus_path']}]
            or config['projection_invalidation']['manifest_sha256'] != profile['policy_sha256']):
        raise ValueError('source profile scope mismatch')
    if arguments.command == 'resolve':
        print(json.dumps(profile, ensure_ascii=False))
        return
    if arguments.command in ('sync', 'index', 'status') and arguments.args:
        raise ValueError('unexpected command parameters')
    if arguments.command == 'mcp' and arguments.args != ['serve']:
        raise ValueError('mcp serve required')
    environment = os.environ.copy()
    environment['VRG_CONFIG'] = profile['config_path']
    environment.pop('VRG_DATA_DIR', None)
    binary = registry['binary']
    os.execve(binary, [binary, '--config', profile['config_path'], arguments.command,
                       *arguments.args], environment)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print('VRG sources selection failed: ' + str(error), file=sys.stderr)
        raise SystemExit(78)
