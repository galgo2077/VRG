#!/usr/bin/env python3
"""GV4 actual-binary lifecycle checks. Tokens/URLs remain only in process memory."""
import argparse
import http.client
import json
import os
import pathlib
import re
import selectors
import signal
import subprocess
import tempfile
import time
import urllib.parse

p = argparse.ArgumentParser()
p.add_argument('binary', type=pathlib.Path)
p.add_argument('profile', type=pathlib.Path)
p.add_argument('scope', type=pathlib.Path)
p.add_argument('--profile-b', type=pathlib.Path, required=True)
p.add_argument('--scope-b', type=pathlib.Path, required=True)
p.add_argument('--output', type=pathlib.Path, required=True)
a = p.parse_args()
binary = str(a.binary.resolve())
checks = []
processes = []


def check(name, condition):
    if not condition:
        raise AssertionError(name)
    checks.append({'case': name, 'status': 'PASS'})


def start(profile, scope, path, no_open=False, extra_env=None):
    env = dict(os.environ, PATH=str(path))
    env.update(extra_env or {})
    args = [binary, '--config', str(profile.resolve()), str(scope.resolve())]
    if no_open:
        args.append('--no-open')
    proc = subprocess.Popen(args, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    processes.append(proc)
    selector = selectors.DefaultSelector()
    selector.register(proc.stdout, selectors.EVENT_READ)
    output = b''
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        for _, _ in selector.select(0.1):
            chunk = os.read(proc.stdout.fileno(), 8192)
            if not chunk:
                break
            output += chunk
            match = re.search(rb'http://127\.0\.0\.1:\d+/#token=([a-f0-9]{64})', output)
            if match:
                parsed = urllib.parse.urlsplit(match.group().decode())
                selector.close()
                return proc, parsed.port, match.group(1).decode()
        if proc.poll() is not None:
            break
    selector.close()
    # Startup errors must not dump the captured bootstrap URL into reports.
    raise AssertionError('actual binary failed to start local session')


def request(instance, path='/api/v1/session', token=None):
    _, port, own_token = instance
    conn = http.client.HTTPConnection('127.0.0.1', port, timeout=4)
    conn.request('GET', path, headers={'Host': f'127.0.0.1:{port}', 'Authorization': 'Bearer ' + (own_token if token is None else token)})
    response = conn.getresponse()
    status, payload = response.status, response.read()
    conn.close()
    return status, json.loads(payload)


def stop(instance):
    proc, port, _ = instance
    started = time.monotonic()
    proc.send_signal(signal.SIGINT)
    proc.wait(timeout=5)
    check('Ctrl+C clean exit', proc.returncode == 0)
    check('Ctrl+C bounded shutdown', time.monotonic() - started < 3)
    try:
        conn = http.client.HTTPConnection('127.0.0.1', port, timeout=1)
        conn.request('GET', '/health')
        conn.getresponse()
        alive = True
        conn.close()
    except OSError:
        alive = False
    check('listener revoked after shutdown', not alive)


try:
    with tempfile.TemporaryDirectory(prefix='vrg lifecycle spaces ') as directory:
        root = pathlib.Path(directory)
        bindir = root / 'fake tools'
        bindir.mkdir()
        opener_log = root / 'opener.jsonl'
        opener = bindir / 'xdg-open'
        opener.write_text('''#!/usr/bin/python3
import http.client,json,os,sys,urllib.parse
u=urllib.parse.urlsplit(sys.argv[1]) if len(sys.argv)==2 else None
ok=False
if u and u.hostname=='127.0.0.1' and u.scheme=='http':
 c=http.client.HTTPConnection(u.hostname,u.port,timeout=3)
 c.request('GET','/health',headers={'Host':u.netloc})
 r=c.getresponse();ok=r.status==200;r.read();c.close()
record={'argc':len(sys.argv)-1,'health_ready':ok,'loopback':bool(u and u.hostname=='127.0.0.1'),'secret_fragment':bool(u and u.fragment.startswith('token=')),'private_path_in_url':bool(u and u.path!='/')}
with open(os.environ['VRG_TEST_OPENER_LOG'],'a') as f:f.write(json.dumps(record)+'\\n')
sys.exit(int(os.environ.get('VRG_TEST_OPENER_EXIT','0')))
''')
        opener.chmod(0o700)
        instance = start(a.profile, a.scope, bindir, extra_env={'VRG_TEST_OPENER_LOG': str(opener_log)})
        deadline = time.monotonic() + 4
        while not opener_log.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        lines = opener_log.read_text().splitlines() if opener_log.exists() else []
        check('automatic opener exactly once', len(lines) == 1)
        record = json.loads(lines[0])
        check('opener gets one safe argv and ready health', record['argc'] == 1 and record['health_ready'] and record['loopback'] and record['secret_fragment'] and not record['private_path_in_url'])
        check('opened session remains available', request(instance)[0] == 200)
        stop(instance)

        opener_log.unlink()
        instance = start(a.profile, a.scope, bindir, no_open=True, extra_env={'VRG_TEST_OPENER_LOG': str(opener_log)})
        time.sleep(0.2)
        check('--no-open does not spawn opener', not opener_log.exists())
        stop(instance)

        empty_path = root / 'no browser tools'
        empty_path.mkdir()
        instance = start(a.profile, a.scope, empty_path)
        check('missing browser keeps authenticated server alive', request(instance)[0] == 200)
        stop(instance)

        one = start(a.profile, a.scope, bindir, no_open=True)
        two = start(a.profile_b, a.scope_b, bindir, no_open=True)
        status_one, body_one = request(one)
        status_two, body_two = request(two)
        check('two concurrent instances have separate listeners', one[1] != two[1] and status_one == status_two == 200)
        check('two scopes stay distinct', body_one['selected_project_id'] != body_two['selected_project_id'])
        check('session tokens cannot cross instances', request(one, token=two[2])[0] == 401 and request(two, token=one[2])[0] == 401)
        stop(one)
        check('closing one instance preserves the other', request(two)[0] == 200)
        stop(two)

    a.output.write_text(json.dumps({'checks': checks, 'count': len(checks), 'publication': 'NOT_AUTHORIZED'}, indent=2) + '\n')
    print(json.dumps({'status': 'PASS', 'checks': len(checks), 'evidence': str(a.output)}))
finally:
    for proc in processes:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
