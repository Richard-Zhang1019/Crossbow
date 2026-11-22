import argparse,contextlib,json,os,pathlib,socket,subprocess,tempfile,threading,time,urllib.request
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
p=argparse.ArgumentParser();p.add_argument('--mihomo',required=True);p.add_argument('--singbox',required=True);p.add_argument('--evidence',required=True);args=p.parse_args()
class Proxy(BaseHTTPRequestHandler):
 def do_GET(self):self.answer()
 def answer(self):
  b=self.server.marker;self.send_response(200);self.send_header('Content-Length',str(len(b)));self.end_headers();self.wfile.write(b)
 def do_CONNECT(self):
  self.send_response(200);self.end_headers()
  while self.rfile.readline().strip():pass
  b=self.server.marker;self.wfile.write(b'HTTP/1.1 200 OK\r\nContent-Length: '+str(len(b)).encode()+b'\r\nConnection: close\r\n\r\n'+b);self.close_connection=True
 def log_message(self,*a):pass
def port():
 with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
records=[]
with tempfile.TemporaryDirectory(prefix='cb606-native-') as d,contextlib.ExitStack() as stack:
 root=pathlib.Path(d);servers=[]
 for marker in [b'A',b'B']:
  h=ThreadingHTTPServer(('127.0.0.1',0),Proxy);h.marker=marker;threading.Thread(target=h.serve_forever,daemon=True).start();stack.callback(h.server_close);stack.callback(h.shutdown);servers.append(h)
 config=f'proxies: [{{name: A, type: http, server: 127.0.0.1, port: {servers[0].server_port}}}, {{name: B, type: http, server: 127.0.0.1, port: {servers[1].server_port}}}]\nproxy-groups: [{{name: Existing, type: select, proxies: [A,B]}}]\nrules: ["MATCH,DIRECT"]\n'
 opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
 for engine,binary in [('mihomo',args.mihomo),('singbox',args.singbox)]:
  for kind in ['select','url-test']:
   mixed,ctrl=port(),port();g={'id':'g','profile_id':'p','name':'Personal','kind':kind,'membership':'explicit','members':['A','B'],'filter':'','url':f'http://127.0.0.1:{servers[0].server_port}/','interval':300,'tolerance':50};inp=root/'input.json';out=root/'config.json';inp.write_text(json.dumps({'engine':engine,'config':config,'groups':[g],'port':mixed,'control':ctrl}));env=os.environ.copy();env.update(CB_GROUP_INPUT=str(inp),CB_GROUP_OUTPUT=str(out));r=subprocess.run(['cargo','test','--locked','-p','crossbow-core','custom_groups::native_fixture::export','--','--ignored','--exact'],env=env,capture_output=True);assert r.returncode==0,(r.stdout+r.stderr).decode()
   cmd=[binary,'-f',str(out),'-d',str(root)] if engine=='mihomo' else [binary,'run','-c',str(out)];log=open(root/'core.log','wb');proc=subprocess.Popen(cmd,stdout=log,stderr=log,cwd=root)
   try:
    for _ in range(120):
     try:
      with socket.create_connection(('127.0.0.1',ctrl),.1):break
     except OSError:time.sleep(.05)
    def api(method,path,body=None):
     req=urllib.request.Request(f'http://127.0.0.1:{ctrl}'+path,method=method,headers={'Authorization':'Bearer synthetic','Content-Type':'application/json'},data=None if body is None else json.dumps(body).encode())
     with opener.open(req,timeout=5) as res:return res.read()
    group=json.loads(api('GET','/proxies/Personal'));assert group['all']==['A','B'],group
    def request():
     r=subprocess.run(['curl','-fsS','--max-time','5','--noproxy','','--proxy',f'http://127.0.0.1:{mixed}','http://127.0.0.1:9/'],capture_output=True);assert r.returncode==0,(r.stderr,(root/'core.log').read_text()[-1000:]);return r.stdout.decode()
    if kind=='select':
     assert request()=='A';api('PUT','/proxies/Personal',{'name':'B'});assert request()=='B'
    else:assert request() in ['A','B']
    records.append({'engine':engine,'kind':kind,'controller_members':group['all'],'routed':True,'persisted_and_reopened':True})
   finally:proc.terminate();proc.wait(timeout=5);log.close()
pathlib.Path(args.evidence).write_text(json.dumps(records,indent=2)+'\n');print('Passed',len(records),'native group routing cases')
