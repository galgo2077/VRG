#!/usr/bin/env python3
"""GV4 native-store oracle. Emits hashes/counts only, never note text or token."""
import argparse, hashlib, http.client, json, pathlib, re, signal, sqlite3, subprocess, time, tomllib, urllib.parse, zlib
p=argparse.ArgumentParser()
p.add_argument('binary',type=pathlib.Path);p.add_argument('profile',type=pathlib.Path);p.add_argument('scope',type=pathlib.Path);p.add_argument('--output',type=pathlib.Path,required=True)
p.add_argument('--registered',action='store_true',help='Resolve PATH through registered profiles, without --config')
a=p.parse_args();config=tomllib.loads(a.profile.read_text());roots={r['id']:pathlib.Path(r['path']) for r in config['roots']}
sha=lambda b:hashlib.sha256(b).hexdigest()
profile_before=sha(a.profile.read_bytes())
command=[str(a.binary.resolve())]+([] if a.registered else ['--config',str(a.profile.resolve())])+[str(a.scope.resolve()),'--no-open']
proc=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
try:
    url=None
    for _ in range(20):
        line=proc.stdout.readline();m=re.search(r'http://127\.0\.0\.1:\d+/[^\s]*',line)
        if m:url=m.group(0);break
        if not line:break
    assert url,'no session bootstrap'
    u=urllib.parse.urlsplit(url);token=u.fragment.removeprefix('token=')
    def get(path):
        c=http.client.HTTPConnection(u.hostname,u.port,timeout=15);c.request('GET',path,headers={'Host':u.netloc,'Authorization':'Bearer '+token});r=c.getresponse();body=json.loads(r.read());c.close();assert r.status==200,body.get('error',{});return body
    project=get('/api/v1/session')['selected_project_id'];assert project
    base='/api/v1/projects/'+project
    started=time.perf_counter();g=get(base+'/graph');first_ms=(time.perf_counter()-started)*1000
    required={'api_version','project_id','snapshot_id','generation_id','policy_fingerprint','sync_state','capabilities','coverage','projection','nodes','edges','next_cursor','warnings'}
    assert set(g)==required and g['api_version']=='rag-view-v1'
    assert len(g['nodes'])<=2000 and len(g['edges'])<=10000 and len(json.dumps(g).encode())<2*1024*1024
    ids={n['id'] for n in g['nodes']};assert len(ids)==len(g['nodes'])
    assert all(e['source'] in ids and e['target'] in ids for e in g['edges'])
    chunk=next(n for n in g['nodes'] if n['kind']=='chunk');s=chunk['source']
    rawpath=(roots[s['root_id']]/s['relative_path']).resolve();assert rawpath.is_relative_to(roots[s['root_id']].resolve())
    before=sha(rawpath.read_bytes());assert before==s['revision']
    t=time.perf_counter();d=get(base+'/nodes/'+chunk['id']+'?'+urllib.parse.urlencode({'snapshot_id':g['snapshot_id'],'limit':131072}));detail_ms=(time.perf_counter()-t)*1000
    dbpath=pathlib.Path(config['data_dir'])/'vrg.sqlite3'
    db=sqlite3.connect('file:'+urllib.parse.quote(str(dbpath))+'?mode=ro',uri=True)
    assert db.execute('SELECT generation_id FROM active_generation WHERE singleton=1').fetchone()[0]==g['generation_id']
    row=db.execute('SELECT body FROM chunks WHERE generation_id=? AND root_id=? AND relative_path=? AND revision=? AND start_byte=? AND end_byte=?',(g['generation_id'],s['root_id'],s['relative_path'],s['revision'],s['start_byte'],s['end_byte'])).fetchone();assert row
    payload=row[0];payload=payload.encode() if isinstance(payload,str) else payload
    if payload.startswith(b'VRGA'):
        assert payload[4]==1 and payload[5] in (0,1)
        raw=payload[46:] if payload[5]==0 else zlib.decompress(payload[46:])
        assert len(raw)==int.from_bytes(payload[6:14],'little') and hashlib.sha256(raw).digest()==payload[14:46]
    else:raw=payload
    assert d['text'].encode()==raw and d['offset_basis']=='body_text_utf8'
    original=get(base+'/nodes/'+chunk['id']+'?'+urllib.parse.urlencode({'snapshot_id':g['snapshot_id'],'limit':131072,'source':1}))
    assert original['text'].encode()==rawpath.read_bytes() and original['offset_basis']=='raw_markdown_utf8'
    assert before==sha(rawpath.read_bytes()) and profile_before==sha(a.profile.read_bytes())
    rss_status=pathlib.Path('/proc')/str(proc.pid)/'status'
    memory={line.split(':')[0]:int(line.split()[1])*1024 for line in rss_status.read_text().splitlines() if line.startswith(('VmRSS:','VmHWM:'))}
    oracle={'server_memory_bytes':memory,'status':'PASS','generation_id':g['generation_id'],'nodes':len(g['nodes']),'chunks':sum(n['kind']=='chunk' for n in g['nodes']),'edges':len(g['edges']),'coverage':g['coverage'],'semantic_vectors':g['capabilities']['semantic_vectors'],'chunk_exact_sha256':sha(raw),'source_revision':before,'offset_basis':d['offset_basis'],'source_unchanged':True,'profile_unchanged':True,'first_graph_ms':round(first_ms,2),'detail_ms':round(detail_ms,2)}
    a.output.write_text(json.dumps(oracle,indent=2)+'\n');print(json.dumps(oracle))
finally:
    proc.send_signal(signal.SIGINT) if proc.poll() is None else None
    try:proc.wait(timeout=5)
    except subprocess.TimeoutExpired:proc.kill();proc.wait()
