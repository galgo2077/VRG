#!/usr/bin/env python3
"""Assemble an explicit, private-data-free release package; no network/publish."""
import argparse,gzip,hashlib,json,os,pathlib,re,shutil,subprocess,tarfile,tomllib
p=argparse.ArgumentParser();p.add_argument('--binary',type=pathlib.Path,required=True);p.add_argument('--destination',type=pathlib.Path,required=True);p.add_argument('--licenses-list',type=pathlib.Path,required=True);p.add_argument('--notices-dir',type=pathlib.Path,required=True);a=p.parse_args()
repo=pathlib.Path(__file__).resolve().parent.parent
commit=subprocess.check_output(['git','-C',str(repo),'rev-parse','HEAD'],text=True).strip()
assert not subprocess.check_output(['git','-C',str(repo),'status','--porcelain'],text=True).strip(), 'Release checkout must be clean'
version=tomllib.loads((repo/'Cargo.toml').read_text())['package']['version']
name='vrg-v'+version+'-linux-x86_64'
stage=a.destination/name;stage.mkdir(parents=True,exist_ok=False)
files=[]
def put(source,target):
 assert source.is_file() and not source.is_symlink(),source
 target=stage/target;target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(source,target);files.append(target)
put(a.binary,'bin/vrg');(stage/'bin/vrg').chmod(0o755)
put(repo/'README.md','README.md')
for document in sorted((repo/'docs/public').glob('*.md')):
 put(document,pathlib.Path('docs/public')/document.name)
put(repo/'docs/assets/vrg-banner.svg','docs/assets/vrg-banner.svg')
put(repo/'CHANGELOG.md','CHANGELOG.md')
put(repo/'docs/RELEASE_PROTOCOL.md','docs/RELEASE_PROTOCOL.md')
for name_license in ['LICENSE-MIT','LICENSE-APACHE']:put(repo/name_license,name_license)
put(repo/'Cargo.lock','Cargo.lock')
for asset in ['index.html','app.js','style.css','nuclei.js']:put(repo/'src/view_assets'/asset,'assets/'+asset)
registry=pathlib.Path(os.environ.get('CARGO_HOME',pathlib.Path.home()/'.cargo'))/'registry/src'
packages=[];missing=[]
for line in a.licenses_list.read_text().splitlines():
 match=re.match(r'([A-Za-z0-9_-]+) v([^ |]+).*?\|(.*)',line)
 if not match or match[1]=='vrg':continue
 package,version,license_spec=match.groups()
 if any(x['name']==package and x['version']==version for x in packages):continue
 dirs=list(registry.glob('*/'+package+'-'+version));assert len(dirs)==1,(package,version)
 root=dirs[0];metadata=tomllib.loads((root/'Cargo.toml').read_text())['package']
 candidates=[f for f in root.rglob('*') if f.is_file() and not f.is_symlink() and (f.name.upper().startswith(('LICENSE','LICENCE','COPYING','NOTICE','UNLICENSE')) or f.relative_to(root).as_posix()==metadata.get('license-file'))]
 candidates=[f for f in candidates if len(f.relative_to(root).parts)<=3 and not {'node_modules','target','.git','.cache'}.intersection(f.relative_to(root).parts) and f.stat().st_size<4*1024*1024]
 if not candidates:
  override=a.notices_dir/(package+'-'+version)
  extra=list(override.iterdir()) if override.is_dir() else []
  if not extra:missing.append(package+'-'+version)
  for f in extra:put(f,pathlib.Path('licenses')/(package+'-'+version)/f.name)
 for f in candidates:put(f,pathlib.Path('licenses')/(package+'-'+version)/f.relative_to(root))
 if 'MPL' in license_spec:
  for f in root.rglob('*'):
   if f.is_file() and not f.is_symlink() and '.git' not in f.parts:
    put(f,pathlib.Path('licenses')/(package+'-'+version)/'source'/f.relative_to(root))
 packages.append({'name':package,'version':version,'license':license_spec.replace(' (*)','').strip()})
assert not missing,('Missing dependency notices',missing)
for notice in ['ONNXRuntime-LICENSE','ONNXRuntime-ThirdPartyNotices.txt','provenance.json']:put(a.notices_dir/notice,'licenses/onnxruntime-1.31.0/'+notice)
symbols=subprocess.check_output(['readelf','--version-info',str(a.binary)],text=True)
requirements=sorted(set(re.findall(r'\b(?:GLIBC|GLIBCXX|CXXABI)_[0-9.]+',symbols)))
info={'format':'vrg-release-v1','runtime_source_commit':commit,'crate_version':tomllib.loads((repo/'Cargo.toml').read_text())['package']['version'],'platform':'linux-x86_64','frontend':'Embedded WebGL2/DOM; MIT OR Apache-2.0; no CDN','required_symbol_versions':requirements,'dependencies':packages,'source_availability':{'option-ext-0.2.0':'https://docs.rs/crate/option-ext/0.2.0/source/'}}
buildinfo=stage/'BUILD.json';buildinfo.write_text(json.dumps(info,indent=2)+'\n');files.append(buildinfo)
sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
manifest=stage/'MANIFEST.json';manifest.write_text(json.dumps({'files':[{'path':f.relative_to(stage).as_posix(),'bytes':f.stat().st_size,'sha256':sha(f)} for f in sorted(files)]},indent=2)+'\n');files.append(manifest)
checksums=stage/'SHA256SUMS';checksums.write_text(''.join(sha(f)+'  '+f.relative_to(stage).as_posix()+'\n' for f in sorted(files)));files.append(checksums)
for f in files:
 assert f.is_relative_to(stage) and not f.is_symlink()
 # Prebuilt upstream native libraries retain public GitHub runner paths.
 assert all(prefix == b'/home/runner/' for prefix in re.findall(rb'/home/[A-Za-z0-9_.-]+/',f.read_bytes())), ('Personal path',f)
# Source archives are generated separately from the exact clean Git commit.
archive=a.destination/(name+'.tar.gz')
with archive.open('wb') as output:
 with gzip.GzipFile(fileobj=output,mode='wb',filename='',mtime=0) as compressed:
  with tarfile.open(fileobj=compressed,mode='w|') as tar:
   for f in sorted(files):
    item=tar.gettarinfo(str(f),arcname=name+'/'+f.relative_to(stage).as_posix());item.uid=item.gid=0;item.uname=item.gname='';item.mtime=0
    with f.open('rb') as source:tar.addfile(item,source)
checksum=sha(archive);(a.destination/'SHA256SUMS.txt').write_text(checksum+'  '+archive.name+'\n')
print(json.dumps({'package':str(archive),'sha256':checksum,'files':len(files),'dependency_notices':len(packages),'source_commit':commit}))
