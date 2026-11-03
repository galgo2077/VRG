#!/usr/bin/env python3
"""Bounded GV4 HTTP/security/lifecycle checks; session secrets remain in memory."""
import argparse, http.client, json, os, pathlib, re, signal, subprocess, time, urllib.parse
p=argparse.ArgumentParser()
p.add_argument('binary',type=pathlib.Path)
p.add_argument('profile',type=pathlib.Path)
p.add_argument('scope',type=pathlib.Path)
p.add_argument('--output',type=pathlib.Path,required=True)
a=p.parse_args()
results=[]
proc=subprocess.Popen([str(a.binary.resolve()),'--config',str(a.profile.resolve()),'view',str(a.scope.resolve()),'--no-open'],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
try:
    url=None
    for _ in range(20):
        line=proc.stdout.readline()
        if not line: break
        match=re.search(r'http://127\.0\.0\.1:\d+/[^\s]*',line)
        if match: url=match.group(0);break
    assert url,'server did not provide bootstrap URL'
    parsed=urllib.parse.urlsplit(url)
    token=urllib.parse.unquote(parsed.fragment)
    if token.startswith('token='): token=token[6:]
    authority=parsed.netloc
    def req(path='/api/v1/projects',token_value=token,extra=None,method='GET'):
        connection=http.client.HTTPConnection('127.0.0.1',parsed.port,timeout=10)
        headers={'Host':authority}
        if token_value is not None: headers['Authorization']='Bearer '+token_value
        headers.update(extra or {})
        connection.request(method,path,headers=headers)
        response=connection.getresponse()
        status=response.status; headers=dict(response.getheaders());body=response.read()
        connection.close()
        return status,headers,body
    def check(name,predicate):
        assert predicate,name
        results.append({'case':name,'status':'PASS'})
    check('no token denied',req(token_value=None)[0] in (401,403))
    check('wrong token denied',req(token_value='invalid')[0] in (401,403))
    check('valid token',req()[0]==200)
    for value in ('null','https://evil.invalid','http://127.0.0.1:1'):
        check('origin rejected '+value,req(extra={'Origin':value})[0] in (401,403))
    check('cross-site rejected',req(extra={'Sec-Fetch-Site':'cross-site'})[0] in (401,403))
    check('host rejected',req(extra={'Host':'evil.invalid'})[0] in (400,403))
    check('local origin accepted',req(extra={'Origin':'http://'+authority})[0]==200)
    check('write method denied',req(method='POST')[0] in (400,403,405))
    status,headers,body=req('/',token_value=None)
    check('landing accessible no notes',status==200)
    headers={k.lower():v for k,v in headers.items()}
    check('CSP restrictive',"default-src 'none'" in headers.get('content-security-policy','') and 'unsafe-' not in headers.get('content-security-policy',''))
    check('no persistent cache',headers.get('cache-control')=='no-store')
    check('no referrer',headers.get('referrer-policy')=='no-referrer')
    check('no MIME sniff',headers.get('x-content-type-options')=='nosniff')
    check('no CORS wildcard','access-control-allow-origin' not in headers)
    check('bootstrap contains no secret',token.encode() not in body)
    check('traversal rejected',req('/assets/../profile.toml')[0] in (400,403,404))
    check('encoded traversal rejected',req('/api/v1/projects/%2e%2e/graph')[0] in (400,403,404))
    # A second bind to the same explicit port must fail, never kill first server.
    busy=subprocess.run([str(a.binary.resolve()),'--config',str(a.profile.resolve()),'view',str(a.scope.resolve()),'--no-open','--port',str(parsed.port)],capture_output=True,text=True,timeout=10)
    check('occupied port explicit failure',busy.returncode!=0 and req()[0]==200)
    proc.send_signal(signal.SIGINT)
    proc.wait(timeout=8)
    check('Ctrl+C clean exit',proc.returncode==0)
    try: req(); alive=True
    except OSError: alive=False
    check('listener closed',not alive)
    a.output.write_text(json.dumps({'checks':results,'count':len(results),'publication':'NOT_AUTHORIZED'},indent=2)+'\n')
    print(json.dumps({'checks':len(results),'status':'PASS','evidence':str(a.output)}))
finally:
    if proc.poll() is None:
        proc.terminate()
        try:proc.wait(timeout=4)
        except subprocess.TimeoutExpired:proc.kill()
