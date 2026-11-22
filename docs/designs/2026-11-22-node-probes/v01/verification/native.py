"""#604 isolated native evidence; no host proxy/DNS/TUN modification.
Requires the built desktop guardian + Rust lib test executable (explicitly ignored tests).
Two public suites/core: <=5 MiB requested payload each, fixed Cloudflare/UDP DNS targets.
"""
import argparse,hashlib,json,os,secrets,socket,subprocess,sys,tempfile,time
from pathlib import Path
sys.dont_write_bytecode=True
REPO=Path(__file__).resolve().parents[5]
sys.path.insert(0,str(REPO/'scripts/benchmark'))
from bench_network import Core,fixture,free_port,upstream,measured_fetch,summarize
p=argparse.ArgumentParser(description=__doc__)
for key in ['test-bin','helper','mihomo','singbox','output']:p.add_argument('--'+key,type=Path,required=True)
a=p.parse_args();evidence={'platform':sys.platform,'scope':'isolated real cores + production measurement service/guardian; not native Tauri IPC/UI or system takeover acceptance','engines':[]}
def env(binary,engine):return dict(os.environ,CB_PROBE_CORE=str(binary),CB_PROBE_ENGINE=engine,CB_PROBE_HELPER=str(a.helper))
def compact(samples):
 summary=summarize(samples);summary.pop('samples');summary['samples']=[{'ms':r['elapsed_ms'],'ok':r['ok'],'error':r['error_class'],'route_evidence':bool(r.get('core_evidence'))} for r in samples];return summary
def crash_parent(binary,engine):
 folder=Path(tempfile.mkdtemp(prefix='crossbow-probe-'));os.chmod(folder,0o700);port=free_port()
 config,_=fixture(port,free_port(exclude=(port,)),secrets.token_hex(24),'mihomo' if engine=='mihomo' else 'sing-box')
 for name,data in [('probe.json',config),('launch.json',{'version':1,'binary':str(binary),'mihomo':engine=='mihomo'})]:
  f=folder/name;f.write_text(json.dumps(data));os.chmod(f,0o600)
 script="""
import subprocess,sys,socket,time
from pathlib import Path
helper,folder,port=sys.argv[1],sys.argv[2],int(sys.argv[3])
child=subprocess.Popen([helper,'--crossbow-node-probe-worker',folder],stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
for _ in range(250):
 if child.poll() is not None:raise SystemExit(2)
 try:
  with socket.create_connection(('127.0.0.1',port),timeout=.05):pass
  Path(folder,'ready').write_text('ready');break
 except OSError:time.sleep(.02)
else:raise SystemExit(3)
time.sleep(90)
"""
 parent=subprocess.Popen([sys.executable,'-c',script,str(a.helper),str(folder),str(port)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 try:
  deadline=time.monotonic()+7
  while not (folder/'ready').exists() and parent.poll() is None and time.monotonic()<deadline:time.sleep(.02)
  assert (folder/'ready').exists(),'fixture core never became ready'
  began=time.monotonic();parent.kill();parent.wait(timeout=2)
  while folder.exists() and time.monotonic()-began<3:time.sleep(.01)
  assert not folder.exists(),'guardian must remove its private directory after parent death'
  return {'ready_before_parent_death':True,'private_directory_removed':True,'cleanup_ms':round((time.monotonic()-began)*1000,2)}
 finally:
  if parent.poll() is None:parent.kill();parent.wait(timeout=2)
for engine,binary in [('mihomo',a.mihomo),('singbox',a.singbox)]:
 data={'engine':engine,'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'controlled_cases':[],'public':[]}
 for case in ['native_tcp_success_https_failure_and_cancel_cleanup','native_http_success_udp_blackhole_is_unverified']:
  result=subprocess.run([str(a.test_bin),case,'--ignored','--nocapture','--test-threads=1'],env=env(binary,engine),capture_output=True,text=True,timeout=35)
  data['controlled_cases'].append({'case':case,'passed':result.returncode==0,'stdout':result.stdout})
  if result.returncode:raise RuntimeError(result.stdout+'\n'+result.stderr)
 data['parent_death']=crash_parent(binary,engine)
 with tempfile.TemporaryDirectory(prefix='crossbow-business-') as raw,upstream() as origin:
  folder=Path(raw);mixed=free_port();controller=free_port(exclude=(mixed,));secret=secrets.token_hex(24)
  config,name=fixture(mixed,controller,secret,'mihomo' if engine=='mihomo' else 'sing-box');path=folder/name;path.write_text(json.dumps(config));os.chmod(path,0o600)
  with Core(binary,folder,path,controller,secret,'core.log','mihomo' if engine=='mihomo' else 'sing-box') as business:
   for kind in ['connectivity','throughput']:
    url=f'http://127.0.0.1:{origin}/204';baseline=[]
    for _ in range(30):baseline.append(measured_fetch(business,mixed,url,timeout=3));time.sleep(.05)
    output=folder/(kind+'.json');run_env=dict(env(binary,engine),CB_PROBE_KIND=kind,CB_PROBE_OUTPUT=str(output))
    task=subprocess.Popen([str(a.test_bin),'native_bounded_public_component_suite','--ignored','--nocapture','--test-threads=1'],env=run_env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
    deadline=time.monotonic()+10;snapshot={}
    while task.poll() is None and time.monotonic()<deadline:
     try:snapshot=json.loads(output.read_text())
     except (OSError,ValueError):pass
     if snapshot.get('phase') not in (None,'preparing'):break
     time.sleep(.02)
    measured=[];overlap=0
    for _ in range(30):
     try:snapshot=json.loads(output.read_text())
     except (OSError,ValueError):pass
     overlap+=int(snapshot.get('state')=='running' and snapshot.get('phase')!='preparing')
     measured.append(measured_fetch(business,mixed,url,timeout=3));time.sleep(.05)
    stdout,stderr=task.communicate(timeout=35)
    if task.returncode:raise RuntimeError(stdout+'\n'+stderr)
    final=json.loads(output.read_text())
    data['public'].append({'kind':kind,'business_before':compact(baseline),'business_during':compact(measured),'overlapping_business_requests':overlap,'measurement':final,'note':'4 MiB download/1 MiB upload are payload caps, not wire-billing caps; includes connection overhead and host scheduling; small sample only'})
 evidence['engines'].append(data)
evidence['source_sha256']={str(f.relative_to(REPO)):hashlib.sha256(f.read_bytes()).hexdigest() for f in (REPO/'apps/desktop/src-tauri/src/probe_worker').glob('*.rs')}
evidence['source_sha256']['apps/desktop/src-tauri/src/node_probes.rs']=hashlib.sha256((REPO/'apps/desktop/src-tauri/src/node_probes.rs').read_bytes()).hexdigest()
a.output.parent.mkdir(parents=True,exist_ok=True);a.output.write_text(json.dumps(evidence,ensure_ascii=False,indent=2)+'\n');print(a.output)
