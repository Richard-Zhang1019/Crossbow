#!/usr/bin/env python3
"""Isolated real Hy2/TUIC/REALITY connections using configs from public import.
Requires explicitly selected installed mihomo, sing-box and openssl. No downloads,
no system proxy/TUN, no real subscriptions. Certificates/keys die with temp dir.
"""
import argparse, contextlib, json, os, pathlib, socket, ssl, subprocess, tempfile, threading, time, urllib.parse, urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

def free_port():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0)); return s.getsockname()[1]

def wait_port(port, process):
    deadline=time.monotonic()+8
    while time.monotonic()<deadline:
        if process.poll() is not None: raise RuntimeError('fixture core exited; inspect private fixture log')
        try:
            with socket.create_connection(('127.0.0.1', port), .2): return
        except OSError: time.sleep(.05)
    raise RuntimeError('fixture core listener did not become ready')

class Echo(BaseHTTPRequestHandler):
    def do_GET(self):
        body=b'crossbow-controlled-response'
        self.send_response(200); self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    def log_message(self,*args): pass

def exact(s,n):
    data=b''
    while len(data)<n:
        chunk=s.recv(n-len(data))
        if not chunk: raise RuntimeError('closed SOCKS connection')
        data+=chunk
    return data

def udp_roundtrip(mixed, target):
    with socket.create_connection(('127.0.0.1',mixed),2) as tcp, socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as udp:
        tcp.settimeout(6);tcp.sendall(b'\x05\x01\x00');assert exact(tcp,2)==b'\x05\x00'
        tcp.sendall(b'\x05\x03\x00\x01'+bytes(6));header=exact(tcp,4);assert header[1]==0
        if header[3]==1: host=socket.inet_ntoa(exact(tcp,4))
        elif header[3]==4: host=socket.inet_ntop(socket.AF_INET6,exact(tcp,16))
        else: host=exact(tcp,exact(tcp,1)[0]).decode()
        port=int.from_bytes(exact(tcp,2),'big');host='127.0.0.1' if host in ('0.0.0.0','::') else host
        body=b'crossbow-udp-payload';packet=b'\0\0\0\x01'+socket.inet_aton('127.0.0.1')+target.to_bytes(2,'big')+body
        udp.settimeout(6);udp.sendto(packet,(host,port));reply,_=udp.recvfrom(4096);assert reply.endswith(body)

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--mihomo',required=True);parser.add_argument('--singbox',required=True);parser.add_argument('--evidence',required=True);args=parser.parse_args()
    repo=pathlib.Path(__file__).resolve().parents[2];records=[]
    with tempfile.TemporaryDirectory(prefix='cb-share-native-') as directory, contextlib.ExitStack() as stack:
        root=pathlib.Path(directory)
        def run(cmd,**kw):
            try:return subprocess.run(cmd,check=True,capture_output=True,timeout=90,**kw)
            except subprocess.CalledProcessError as error:
                # All inputs here are generated synthetic fixtures, never user data.
                raise RuntimeError((error.stdout+error.stderr).decode(errors='replace')[-3000:]) from error
        run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-keyout',str(root/'key.pem'),'-out',str(root/'cert.pem'),'-days','1','-subj','/CN=localhost','-addext','subjectAltName=DNS:localhost','-addext','basicConstraints=critical,CA:TRUE'])
        http=ThreadingHTTPServer(('127.0.0.1',0),Echo);stack.callback(http.server_close);stack.callback(http.shutdown)
        threading.Thread(target=http.serve_forever,daemon=True).start()
        tls=ThreadingHTTPServer(('127.0.0.1',0),Echo);context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.load_cert_chain(root/'cert.pem',root/'key.pem');context.set_ecdh_curve('X25519');tls.socket=context.wrap_socket(tls.socket,server_side=True)
        threading.Thread(target=tls.serve_forever,daemon=True).start();stack.callback(tls.server_close);stack.callback(tls.shutdown)
        udp=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);udp.bind(('127.0.0.1',0));stack.callback(udp.close)
        def echo_udp():
            while True:
                try: data,addr=udp.recvfrom(4096);udp.sendto(data,addr)
                except OSError:return
        threading.Thread(target=echo_udp,daemon=True).start()
        keys=run([args.singbox,'generate','reality-keypair']).stdout.decode().splitlines();keys={line.split(':',1)[0].strip():line.split(':',1)[1].strip() for line in keys if ':' in line}
        uid='00000000-0000-4000-8000-000000000001';password='test:p@ss';q=urllib.parse.quote(password,safe='');hy,tu,re,hyplain=[free_port() for _ in range(4)];ca={'enabled':True,'certificate_path':str(root/'cert.pem'),'key_path':str(root/'key.pem'),'alpn':['h3']}
        server={'log':{'level':'warn'},'inbounds':[
            {'type':'hysteria2','listen':'127.0.0.1','listen_port':hy,'users':[{'password':password}],'obfs':{'type':'salamander','password':'test-obfs'},'tls':ca},
            {'type':'hysteria2','listen':'127.0.0.1','listen_port':hyplain,'users':[{'password':password}],'tls':ca},
            {'type':'tuic','listen':'127.0.0.1','listen_port':tu,'users':[{'uuid':uid,'password':password}],'congestion_control':'bbr','tls':ca},
            {'type':'vless','listen':'127.0.0.1','listen_port':re,'users':[{'uuid':uid,'flow':'xtls-rprx-vision'}],'tls':{'enabled':True,'server_name':'localhost','reality':{'enabled':True,'handshake':{'server':'127.0.0.1','server_port':tls.server_port},'private_key':keys['PrivateKey'],'short_id':['abcd']}}}], 'outbounds':[{'type':'direct','tag':'direct'}]}
        (root/'server.json').write_text(json.dumps(server));run([args.singbox,'check','-c',str(root/'server.json')])
        processes=[]
        def start(cmd,env=None):
            log=open(root/f'core-{len(processes)}.log','wb');stack.callback(log.close);p=subprocess.Popen(cmd,stdout=log,stderr=log,env=env);processes.append(p)
            def stop():
                if p.poll() is None:
                    p.terminate()
                    try:p.wait(timeout=4)
                    except subprocess.TimeoutExpired:p.kill();p.wait()
            stack.callback(stop);return p
        serv=start([args.singbox,'run','-c',str(root/'server.json')]);wait_port(re,serv)
        cases=[{'name':'hy2','uri':f'hy2://{q}@127.0.0.1:{hy}?sni=localhost&obfs=salamander&obfs-password=test-obfs'}, {'name':'tuic','uri':f'tuic://{uid}:{q}@127.0.0.1:{tu}?sni=localhost&alpn=h3&congestion_control=bbr&udp_relay_mode=native'}, {'name':'reality','uri':f'vless://{uid}@127.0.0.1:{re}?security=reality&sni=localhost&pbk={keys["PublicKey"]}&sid=abcd&fp=chrome&flow=xtls-rprx-vision'}]
        cases.extend([
            {'name':'hy2-userpass','uri':f'hysteria2://test:p%40ss@127.0.0.1:{hy}?sni=localhost&obfs=salamander&obfs-password=test-obfs'},
            {'name':'tuic-v2rayN','uri':f'tuic://{urllib.parse.quote(uid+":"+password,safe="")}@127.0.0.1:{tu}?sni=localhost&alpn=h3&congestion_control=bbr&udp_relay_mode=native'},
            {'name':'hy2-singbox','engine':'singbox','uri':f'hy2://{q}@127.0.0.1:{hyplain}?sni=localhost'},
            {'name':'reject-auth','uri':cases[0]['uri'].replace(q,'incorrect-auth'),'reject':True},
            {'name':'reject-sni','uri':cases[0]['uri'].replace('sni=localhost','sni=wrong.invalid'),'reject':True},
            {'name':'reject-ca','uri':cases[0]['uri'],'reject':True,'untrusted':True},
            {'name':'reject-reality','uri':cases[2]['uri'].replace('sid=abcd','sid=eeee'),'reject':True},
        ])
        for case in cases:case['mixed_port']=free_port()
        (root/'inputs.json').write_text(json.dumps(cases));env=os.environ.copy();env.update(CB_SHARE_INPUT=str(root/'inputs.json'),CB_SHARE_OUTPUT=str(root))
        run(['cargo','test','--locked','-p','crossbow-core','--test','share_links_native','export_loopback_clients','--','--ignored','--exact'],cwd=repo,env=env)
        for case in cases:
            path=root/(case['name']+'.json');config=json.loads(path.read_text())
            if case.get('engine')=='singbox':
                for outbound in config['outbounds']:
                    if outbound['type']=='hysteria2':outbound['tls']['certificate_path']=str(root/'cert.pem')
                config['experimental']['cache_file']['enabled']=False
                path.write_text(json.dumps(config));run([args.singbox,'check','-c',str(path)])
                client=start([args.singbox,'run','-c',str(path)]);wait_port(case['mixed_port'],client)
                result=run(['curl','--silent','--show-error','--fail','--max-time','8','--noproxy','','--proxy',f'http://127.0.0.1:{case["mixed_port"]}',f'http://127.0.0.1:{http.server_port}/'])
                assert result.stdout==b'crossbow-controlled-response';udp_roundtrip(case['mixed_port'],udp.getsockname()[1])
                records.append({'protocol':case['name'],'tcp':True,'udp':True,'tls_verification':'temporary process-local CA; no insecure flag'});continue
            if not case.get('untrusted'):config['tls']={'custom-certifactes':[(root/'cert.pem').read_text()]}
            control=free_port();config['external-controller']=f'127.0.0.1:{control}';config['secret']='local-fixture'
            path.write_text(json.dumps(config))
            # Fixed mihomo constructs outbound TLS configs before applying its
            # global CA pool. Bootstrap the temporary trust pool, then load the
            # imported config through its authenticated loopback controller.
            bootstrap=dict(config);bootstrap.update(proxies=[],**{'proxy-groups':[],'rules':['MATCH,DIRECT']})
            initial=root/(case['name']+'-bootstrap.json');initial.write_text(json.dumps(bootstrap))
            env=os.environ.copy();env['SSL_CERT_FILE']=str(root/'cert.pem')
            client=start([args.mihomo,'-f',str(initial),'-d',str(root)],env);wait_port(control,client)
            request=urllib.request.Request(f'http://127.0.0.1:{control}/configs?force=true',data=json.dumps({'path':str(path)}).encode(),headers={'Authorization':'Bearer local-fixture','Content-Type':'application/json'},method='PUT')
            with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(request,timeout=6) as response:assert response.status==204
            wait_port(case['mixed_port'],client)
            try:
                result=run(['curl','--silent','--show-error','--fail','--max-time','8','--noproxy','','--proxy',f'http://127.0.0.1:{case["mixed_port"]}',f'http://127.0.0.1:{http.server_port}/'])
            except RuntimeError as error:
                if case.get('reject'):
                    records.append({'case':case['name'],'rejected':True});continue
                logs='\n'.join(p.read_text(errors='replace')[-1800:] for p in root.glob('core-*.log'))
                raise RuntimeError(case['name']+' loopback request failed: '+logs) from error
            assert not case.get('reject'), case['name']+' unexpectedly bypassed verification'
            assert result.stdout==b'crossbow-controlled-response',case['name']
            udp_roundtrip(case['mixed_port'],udp.getsockname()[1]);records.append({'protocol':case['name'],'tcp':True,'udp':True,'tls_verification':'local CA for Hy2/TUIC; REALITY public-key authentication'})
        evidence={'platform':os.uname().sysname+' '+os.uname().machine,'mihomo':run([args.mihomo,'-v']).stdout.decode().splitlines()[0],'singbox':run([args.singbox,'version']).stdout.decode().splitlines()[0],'records':records,'boundary':'loopback controlled peers, public importer generated configs; no system proxy/TUN or public server'}
        pathlib.Path(args.evidence).write_text(json.dumps(evidence,indent=2)+'\n');print(json.dumps(evidence,indent=2))
if __name__=='__main__':main()
