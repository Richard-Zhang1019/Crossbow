import argparse,json,os,pathlib,socket,subprocess,tempfile,time
parser=argparse.ArgumentParser(description="PR #630 controlled loopback review regression; no public traffic")
for name in ['test-bin','helper','mihomo','singbox','output']:
 parser.add_argument('--'+name,type=pathlib.Path,required=True)
args=parser.parse_args()
root=pathlib.Path(__file__).resolve().parents[5]
helper=args.helper.resolve()
tests=args.test_bin.resolve()
cases=[]
for engine,binary in [('mihomo',str(args.mihomo.resolve())),('singbox',str(args.singbox.resolve()))]:
 env=dict(os.environ,CB_PROBE_CORE=binary,CB_PROBE_ENGINE=engine,CB_PROBE_HELPER=str(helper))
 for name in ['probe_worker::tests::native_tcp_success_https_failure_and_cancel_cleanup','probe_worker::transport::native_tests::native_http_success_udp_blackhole_is_unverified']:
  r=subprocess.run([str(tests),name,'--exact','--ignored','--nocapture'],cwd=root,env=env,capture_output=True,text=True,timeout=40)
  assert r.returncode==0,r.stdout+r.stderr
  cases.append({'engine':engine,'case':name,'passed':True,'output':r.stdout})
 folder=pathlib.Path(tempfile.mkdtemp(prefix='crossbow-probe-'));os.chmod(folder,0o700)
 with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
 config=({'mixed-port':port,'bind-address':'127.0.0.1','allow-lan':False,'authentication':['probe:synthetic'],'skip-auth-prefixes':[],'log-level':'silent','dns':{'enable':False},'tun':{'enable':False},'proxies':[{'name':'fixture','type':'direct'}],'proxy-groups':[{'name':'only','type':'select','proxies':['fixture']}],'rules':['MATCH,only']} if engine=='mihomo' else {'log':{'disabled':True},'inbounds':[{'type':'mixed','listen':'127.0.0.1','listen_port':port,'users':[{'username':'probe','password':'synthetic'}]}],'outbounds':[{'type':'direct','tag':'fixture'}],'route':{'final':'fixture'}})
 for name,data in [('probe.json',config),('launch.json',{'version':1,'binary':binary,'mihomo':engine=='mihomo'})]:
  path=folder/name;path.write_text(json.dumps(data));os.chmod(path,0o600)
 code="import subprocess,sys,time; child=subprocess.Popen([sys.argv[1],'--crossbow-node-probe-worker',sys.argv[2]],stdin=subprocess.PIPE,start_new_session=True); print(child.pid,flush=True); time.sleep(60)"
 parent=subprocess.Popen(['python3','-c',code,str(helper),str(folder)],stdout=subprocess.PIPE,text=True)
 guardian=int(parent.stdout.readline());ready=False
 try:
  for _ in range(150):
   try:
    with socket.create_connection(('127.0.0.1',port),.05):ready=True;break
   except OSError:time.sleep(.02)
  assert ready,'guardian core did not start'
  start=time.monotonic();parent.kill();parent.wait(timeout=3)
  while folder.exists() and time.monotonic()-start<4:time.sleep(.02)
  assert not folder.exists(),'private directory not cleaned after parent death'
  try:
   socket.create_connection(('127.0.0.1',port),.1).close();raise AssertionError('owned core still listening')
  except OSError:pass
  cases.append({'engine':engine,'case':'parent-SIGKILL-owned-core-cleanup','passed':True,'elapsed_ms':round((time.monotonic()-start)*1000)})
 finally:
  if parent.poll() is None:parent.kill();parent.wait()
  if folder.exists():
   try:os.killpg(guardian,9)
   except ProcessLookupError:pass
args.output.write_text(json.dumps({'scope':'controlled loopback only; production guardian/Worker; no host networking changes','cases':cases},indent=2)+'\n')
print('Passed',len(cases),'controlled native cases')
