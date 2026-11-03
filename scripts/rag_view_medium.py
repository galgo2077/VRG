#!/usr/bin/env python3
"""Generate exactly 2000 single-chunk synthetic documents and 10000 links."""
import argparse,json,pathlib,sqlite3,subprocess,urllib.parse
p=argparse.ArgumentParser();p.add_argument('destination',type=pathlib.Path);p.add_argument('--index-bin',type=pathlib.Path,required=True);a=p.parse_args()
base=a.destination.resolve();base.mkdir(parents=True,exist_ok=False,mode=0o700);notes=base/'notes';notes.mkdir()
for i in range(2000):
    links=' '.join(f'[enlace {j}](note-{(i+j+1)%2000:04}.md)' for j in range(5))
    (notes/f'note-{i:04}.md').write_text(f'# Sintético {i}\n\n{links}\n\nDato artificial Unicode café para medir límites, nunca datos de usuario.\n')
config=base/'profile.toml';config.write_text('data_dir='+json.dumps(str(base/'store'))+'\n[[roots]]\nid="medium"\npath='+json.dumps(str(notes))+'\n')
subprocess.run([str(a.index_bin.resolve()),'--config',str(config),'index'],check=True,capture_output=True)
db=sqlite3.connect('file:'+urllib.parse.quote(str(base/'store'/'vrg.sqlite3'))+'?mode=ro',uri=True)
g=db.execute('select generation_id from active_generation').fetchone()[0]
counts={'documents':db.execute('select count(*) from documents where generation_id=?',(g,)).fetchone()[0],'chunks':db.execute('select count(*) from chunks where generation_id=?',(g,)).fetchone()[0],'explicit_edges':db.execute('select count(*) from edges where generation_id=?',(g,)).fetchone()[0]};assert counts=={'documents':2000,'chunks':2000,'explicit_edges':10000},counts
print(json.dumps({'profile':str(config),'scope':str(notes),**counts}))
