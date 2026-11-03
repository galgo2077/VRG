#!/usr/bin/env python3
"""Create synthetic, isolated graph-view validation inputs; never user profiles.
Indexing requires an explicit --index-bin and occurs only under the given fixture.
"""
import argparse, json, pathlib, subprocess
p=argparse.ArgumentParser()
p.add_argument('destination',type=pathlib.Path)
p.add_argument('--index-bin',type=pathlib.Path)
p.add_argument('--documents',type=int,default=12)
a=p.parse_args()
base=a.destination.resolve()
base.mkdir(parents=True,exist_ok=False,mode=0o700)
for name in ('A','B'):
    scope=base/name
    notes=scope/'notes'
    notes.mkdir(parents=True)
    for i in range(a.documents):
        link=f'[Documento vecino](note-{(i+1)%a.documents:03}.md)' if i%3 else 'Documento aislado sin enlace saliente.'
        text=f'# Documento {i} — conocimiento sintético {name}\n\n{link}\n\n'
        if i==0:
            text+='Texto hostil literal: <script>window.VRG_XSS=true</script> ![remote](https://example.invalid/tracker.png)\n\n'
        text+='\n\n'.join(f'Párrafo {j}: revisión verificable, Unicode café, diseño local y lectura de chunks. '+('Información sintética para validar navegación y procedencia. '*8) for j in range(5))
        (notes/f'note-{i:03}.md').write_text(text)
    config=scope/'profile.toml'
    config.write_text('data_dir='+json.dumps(str(scope/'store'))+'\nchunk_bytes=700\nchunk_overlap_bytes=60\n[adaptive_memory]\nenabled=true\n[[roots]]\nid="synthetic"\npath='+json.dumps(str(notes))+'\n')
    if a.index_bin:
        subprocess.run([str(a.index_bin),'--config',str(config),'index'],check=True,capture_output=True)
print(json.dumps({'fixture':str(base),'projects':[str(base/x/'notes') for x in ('A','B')],'profiles':[str(base/x/'profile.toml') for x in ('A','B')]}))
