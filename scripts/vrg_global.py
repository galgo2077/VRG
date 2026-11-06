#!/usr/bin/env python3
"""Local integration selector for the configured native VRG binary.

Only registered canonical projects receive authorized memory roots. A Codex cwd
inside an explicitly approved discovery parent can enroll its direct child.
Other directories retain empty profiles and never gain source authorization.
No conversation or hook payload is persisted here.
"""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import stat
import subprocess
import sys
import tomllib

REGISTRY = Path.home() / '.config/vrg/auto-projects.json'


def private_file(path):
    path = Path(path)
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise ValueError('VRG integration requires owned private regular metadata')
    return path


def load_registry():
    data = json.loads(private_file(REGISTRY).read_text())
    if data['schema_version'] != 1:
        raise ValueError('unsupported VRG project registry')
    return data


def atomic_json(path, data):
    temporary = path.with_name(path.name + '.tmp-' + str(os.getpid()))
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(fd, 'w') as handle:
            json.dump(data, handle, indent=2)
            handle.write('\n')
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def git_common_dir(project):
    """Resolve worktree identity without inspecting or changing source contents."""
    result = subprocess.run(['git', '-C', str(project), 'rev-parse',
                             '--path-format=absolute', '--git-common-dir'],
                            capture_output=True, text=True, timeout=10)
    return Path(result.stdout.strip()).resolve(strict=True) if result.returncode == 0 else None


def canonical_project(cwd, registry):
    cwd = Path(cwd).resolve(strict=True)
    for parent in [cwd, *cwd.parents]:
        if (parent / '.git').exists():
            common = git_common_dir(parent)
            matches = [Path(p['project']) for p in registry['projects']
                       if common is not None and git_common_dir(p['project']) == common]
            if len(matches) > 1:
                raise ValueError('ambiguous registered Git common directory')
            return matches[0] if matches else parent
    matches = [Path(p['project']) for p in registry['projects']
               if cwd.is_relative_to(Path(p['project'])) or
               any(cwd.is_relative_to(Path(alias).resolve(strict=True))
                   for alias in p.get('aliases', []))]
    return max(matches, key=lambda p: len(p.parts)) if matches else cwd


def checked_profile(entry):
    config = tomllib.loads(private_file(entry['profile']).read_text())
    roots = config.get('roots', [])
    if config.get('project_memory'):
        raise ValueError('selected profiles must contain explicit scoped roots')
    if config['data_dir'] != entry['data_dir']:
        raise ValueError('profile derived location changed outside its registry')
    if entry.get('memory'):
        expected = {'id': 'auto-memory' if entry.get('auto_enrolled') else 'vrg-memory',
                    'path': entry['memory'],
                    'allowed_files': entry['allowed_files']}
        if len(roots) != 1 or any(roots[0].get(k) != v for k, v in expected.items()):
            raise ValueError('profile root authorization does not match registry')
        if Path(entry['memory']).resolve(strict=True) != Path(entry['memory']):
            raise ValueError('memory root is not canonical')
    elif roots:
        raise ValueError('unregistered project must keep an empty root set')
    if entry.get('managed_note_writes') and (not entry.get('memory') or not entry.get('allowed_files')):
        raise ValueError('managed note writes require explicit memory authorization')
    if entry.get('auto_enrolled'):
        import vrg_autoproject
        registry = load_registry()
        if (vrg_autoproject.candidate(entry['project'], registry) != Path(entry['project'])
                or entry.get('profile_sha256') != hashlib.sha256(private_file(entry['profile']).read_bytes()).hexdigest()):
            raise ValueError('automatic project authorization changed')
        source = entry.get('source')
        if not isinstance(source, dict) or source.get('project_root') != entry['project']:
            raise ValueError('automatic source profile is missing')
        raw = private_file(source['config_path']).read_bytes()
        source_config = tomllib.loads(raw.decode())
        if (hashlib.sha256(raw).hexdigest() != source['config_sha256']
                or source_config.get('data_dir') != source['data_dir']
                or source_config.get('roots') != [{'id': source['root_id'], 'path': source['corpus_path']}]
                or source_config.get('projection_invalidation', {}).get('manifest_sha256') != source['policy_sha256']
                or hashlib.sha256(private_file(source['policy_path']).read_bytes()).hexdigest() != source['policy_sha256']):
            raise ValueError('automatic source authorization changed')
    return entry


def maybe_enroll(cwd):
    """Publish one selected project; never scan siblings under the discovery root."""
    registry = load_registry()
    canonical = canonical_project(cwd, registry)
    if any(entry['project'] == str(canonical) for entry in registry['projects']):
        return
    import vrg_autoproject
    if vrg_autoproject.candidate(cwd, registry) is None:
        return
    lock_path = Path(registry['state_dir']) / 'memory-migration.lock'
    fd = os.open(lock_path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, 'rb') as lock:
        info = os.fstat(lock.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_mode & 0o077 or info.st_nlink != 1):
            raise ValueError('VRG migration lock must be private and owned')
        fcntl.flock(lock, fcntl.LOCK_EX)
        registry = load_registry()
        project = vrg_autoproject.candidate(cwd, registry)
        if project is None or any(entry['project'] == str(project) for entry in registry['projects']):
            return
        entry = vrg_autoproject.prepare(project, registry)
        registry['projects'].append(entry)
        atomic_json(REGISTRY, registry)


def _resolve(cwd=None):
    registry = load_registry()
    project = canonical_project(cwd or os.getcwd(), registry)
    entries = [p for p in registry['projects'] if p['project'] == str(project)]
    if len(entries) > 1:
        raise ValueError('ambiguous canonical project authorization')
    if entries:
        entry = dict(entries[0])
    else:
        key = re.sub(r'[^A-Za-z0-9_.-]', '_', project.name)[:48] or 'project'
        key += '-' + hashlib.sha256(str(project).encode()).hexdigest()[:12]
        data = Path(registry['data_base']) / ('unconfigured-' + key) / 'v0.7'
        profile = Path(registry['profiles_dir']) / ('unconfigured-' + key + '.toml')
        lock_path = Path(registry['state_dir']) / 'profile-selection.lock'
        fd = os.open(lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, 'w') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            if not profile.exists():
                data.mkdir(mode=0o700, parents=True, exist_ok=True)
                if data.resolve() != data:
                    raise ValueError('unconfigured derived location is not canonical')
                text = ('roots=[]\ndata_dir=' + json.dumps(str(data)) +
                        '\ncapture_session_events=false\n[adaptive_memory]\nenabled=false\n')
                output = os.open(profile, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                with os.fdopen(output, 'w') as handle:
                    handle.write(text)
                    handle.flush()
                    os.fsync(handle.fileno())
        entry = {'project': str(project), 'key': key, 'profile': str(profile),
                 'data_dir': str(data), 'memory': None, 'allowed_files': []}
    checked_profile(entry)
    entry.update(executable=registry['binary'], legacy=False)
    return entry


def resolve(cwd=None):
    """Read a coherent registry/profile pair during an authorized root migration."""
    maybe_enroll(cwd or os.getcwd())
    registry = load_registry()
    lock_path = Path(registry['state_dir']) / 'memory-migration.lock'
    try:
        # A shared flock requires no write access to an existing lock. This lets
        # a sandboxed reader resolve an authorized profile without granting it
        # write access to the global integration state.
        fd = os.open(lock_path, os.O_RDONLY | os.O_NOFOLLOW)
    except FileNotFoundError:
        # First initialization still requires authorization to create the lock.
        fd = os.open(lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'rb') as lock:
        info = os.fstat(lock.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_mode & 0o077 or info.st_nlink != 1):
            raise ValueError('VRG migration lock must be an owned private regular file')
        fcntl.flock(lock, fcntl.LOCK_SH)
        return _resolve(cwd)


def has_turn_receipt(selected, session, turn):
    """Inspect existing receipt identity only; never initialize or manufacture proof."""
    database = Path(selected['data_dir']) / 'vrg-closure.sqlite3'
    if not database.exists():
        return False
    private_file(database)
    token = hashlib.sha256(session.encode() + b'\0' + turn.encode()).hexdigest()
    try:
        with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True) as connection:
            row = connection.execute('SELECT turn_key FROM turn_state WHERE session_key=?',
                                     (hashlib.sha256(session.encode()).hexdigest(),)).fetchone()
    except sqlite3.Error as error:
        raise ValueError('cannot verify existing turn identity') from error
    return row is not None and row[0] == token


def select_hook(event, cwd, session, turn=None):
    registry = load_registry()
    state = Path(registry['state_dir']) / 'sessions'
    name = hashlib.sha256(session.encode()).hexdigest() + '.json'
    binding = state / name
    if event == 'Stop':
        if binding.exists():
            selected = json.loads(private_file(binding).read_text())
            current = resolve(cwd)
            if any(current.get(key) != selected.get(key) for key in
                   ('project', 'profile', 'data_dir', 'memory', 'allowed_files')):
                raise ValueError('Codex project authorization changed within the turn; start a fresh session')
            checked_profile(selected)
            if not turn:
                raise ValueError('Stop requires its turn identity')
            if selected.get('turn_sha256'):
                if selected['turn_sha256'] != hashlib.sha256(turn.encode()).hexdigest():
                    raise ValueError('Stop does not match the initialized turn')
                return selected
            # Old session-only bindings could be written by SessionStart during
            # an in-flight legacy turn. Only an exact existing receipt proves
            # that native UserPromptSubmit actually initialized this turn.
            if has_turn_receipt(selected, session, turn):
                return selected
        # An already-running turn was initialized by the previous installation.
        # Keep its original proof/scope; a new prompt/session selects a fresh turn.
        return {'executable': registry['legacy_binary'], 'profile': registry['legacy_config'],
                'project': str(canonical_project(cwd, registry)), 'legacy': True}
    selected = resolve(cwd)
    if event == 'UserPromptSubmit':
        if not turn:
            raise ValueError('UserPromptSubmit requires its turn identity')
        selected['turn_sha256'] = hashlib.sha256(turn.encode()).hexdigest()
        atomic_json(binding, selected)
    # SessionStart may arrive while an older turn is still running. It must
    # never change the route selected by that turn's UserPromptSubmit.
    if not (Path(selected['data_dir']) / 'vrg.sqlite3').exists():
        result = subprocess.run([selected['executable'], '--config', selected['profile'], 'status'],
                                cwd=cwd, capture_output=True, timeout=60)
        if result.returncode:
            raise ValueError('VRG profile initialization failed; sources were not scanned')
    return selected


def main():
    arguments = sys.argv[1:]
    selected = resolve()
    if arguments == ['resolve']:
        print(json.dumps(selected))
        return
    if arguments == ['proposal-schema']:
        print(private_file(REGISTRY.parent / 'MEMORY_PROPOSAL.schema.json').read_text())
        return
    if arguments[:1] == ['note']:
        helper = str(Path(__file__).with_name('vrg_shared.py'))
        os.execv('/usr/bin/python3', ['/usr/bin/python3', helper, *arguments])
    if not arguments:
        raise ValueError('provide a VRG command or resolve')
    environment = os.environ.copy()
    environment['VRG_CONFIG'] = selected['profile']
    os.execve(selected['executable'], [selected['executable'], '--config', selected['profile'], *arguments], environment)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        print('VRG project selection failed: ' + str(error), file=sys.stderr)
        raise SystemExit(78)
