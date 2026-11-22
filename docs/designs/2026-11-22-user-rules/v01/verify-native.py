import argparse,contextlib,json,os,pathlib,socket,subprocess,tempfile,threading,time
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
class Echo(BaseHTTPRequestHandler):
 def do_GET(self):
  b=self.server.marker;self.send_response(200);self.send_header('Content-Length',str(len(b)));self.end_headers();self.wfile.write(b)
 def do_CONNECT(self):
  self.send_response(200);self.end_headers()
  while self.rfile.readline().strip():pass
  b=self.server.marker;self.wfile.write(b"HTTP/1.1 200 OK\r\nContent-Length: "+str(len(b)).encode()+b"\r\nConnection: close\r\n\r\n"+b);self.close_connection=True
 def log_message(self,*a):pass
def port():
 with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
parser=argparse.ArgumentParser();parser.add_argument('--mihomo',required=True);parser.add_argument('--singbox',required=True);parser.add_argument('--evidence',required=True);args=parser.parse_args()
records=[]
with tempfile.TemporaryDirectory(prefix='cb605-native-') as d,contextlib.ExitStack() as stack:
 root=pathlib.Path(d);servers=[]
 for marker in [b'direct',b'policy']:
  h=ThreadingHTTPServer(('127.0.0.1',0),Echo);h.marker=marker;threading.Thread(target=h.serve_forever,daemon=True).start();stack.callback(h.server_close);stack.callback(h.shutdown);servers.append(h)
 config=f'proxies: [{{name: node, type: http, server: 127.0.0.1, port: {servers[1].server_port}}}]\nproxy-groups: [{{name: policy, type: select, proxies: [node]}}]\nrules: ["MATCH,DIRECT"]\n'
 for engine,binary in [('mihomo',args.mihomo),('singbox',args.singbox)]:
  for step,enabled,value,kind,host,expected in [('suffix',True,'example.test','DOMAIN_SUFFIX','sub.example.test','policy'),('root',True,'example.test','DOMAIN_SUFFIX','example.test','policy'),('exact',True,'example.test','DOMAIN','example.test','policy'),('ip',True,'127.0.0.1/32','IP_CIDR','127.0.0.1','policy'),('ipv6',True,'::1/128','IP_CIDR','[::1]','policy'),('disabled',False,'127.0.0.1/32','IP_CIDR','127.0.0.1','direct'),('deleted',False,'127.0.0.1/32','IP_CIDR','127.0.0.1','direct'),('near',True,'example.test','DOMAIN_SUFFIX','notexample.test','policy'),('blocked-root',True,'example.test','DOMAIN_SUFFIX','example.test','blocked'),('broad-first',True,'127.0.0.1/32','IP_CIDR','127.0.0.1','blocked'),('narrow-first',True,'127.0.0.1/32','IP_CIDR','127.0.0.1','policy')]:
   p=port();rules=[] if step=='deleted' else [{'id':'one','kind':kind,'value':value,'target':'policy','enabled':enabled}];
   effective=config
   if step in ('near','blocked-root'):rules[0]['target']='REJECT';effective=config.replace('MATCH,DIRECT','MATCH,policy')
   if step in ('broad-first','narrow-first'):
    rules.append({'id':'two','kind':'IP_CIDR','value':'127.0.0.0/8','target':'REJECT','enabled':True})
    if step=='broad-first':rules.reverse()
   inp=root/'input.json';out=root/'config.json';inp.write_text(json.dumps({'engine':engine,'port':p,'rules':rules,'config':effective}));env=os.environ.copy();env.update(CB_USER_RULE_INPUT=str(inp),CB_USER_RULE_OUTPUT=str(out));r=subprocess.run(['cargo','test','-p','crossbow-core','user_rules::native_fixture::export','--','--ignored','--exact'],env=env,capture_output=True);assert r.returncode==0,(r.stdout+r.stderr).decode()
   cmd=[binary,'-f',str(out),'-d',str(root)] if engine=='mihomo' else [binary,'run','-c',str(out)];log=open(root/'core.log','wb');proc=subprocess.Popen(cmd,stdout=log,stderr=log,cwd=root)
   try:
    for _ in range(100):
     try:
      with socket.create_connection(('127.0.0.1',p),.1):break
     except OSError:time.sleep(.05)
    r=subprocess.run(['curl','-fsS','--max-time','5','--noproxy','','--proxy',f'http://127.0.0.1:{p}',f'http://{host}:{servers[0].server_port}/'],capture_output=True);assert (r.returncode!=0 if expected=='blocked' else r.returncode==0 and r.stdout.decode()==expected),(engine,step,r.stderr, (root/'core.log').read_text()[-1500:]);records.append({'engine':engine,'case':step,'response':expected})
   finally:proc.terminate();proc.wait(timeout=5);log.close()
pathlib.Path(args.evidence).write_text(json.dumps(records,indent=2))
print('Passed',len(records),'real routing cases')
