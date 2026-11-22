"""#604 bounded dual-core controller smoke + independent explicit-proxy probe.
Not a Tauri IPC/native UI or system-proxy/TUN acceptance test. Uses #503 helpers.
python3 <this-file> --mihomo <binary> --singbox <binary> --output <json>
"""
import argparse
import concurrent.futures
import hashlib
import http.client
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import urlencode
sys.dont_write_bytecode = True
REPO=Path(__file__).resolve().parents[5]
sys.path.insert(0,str(REPO/'scripts/benchmark'))
from bench_network import Core, fixture, free_port, upstream, measured_fetch, summarize

def measure(core, name, url, observations):
    started=time.monotonic()
    connection=http.client.HTTPConnection('127.0.0.1',core.controller,timeout=7)
    try:
        connection.request('GET',f'/proxies/{name}/delay?'+urlencode({'url':url,'timeout':5000}),headers={'Authorization':'Bearer '+core.secret})
        response=connection.getresponse();status=response.status;body=json.loads(response.read(65536))
        result={'target':url,'status':status,'delay_ms':body.get('delay'),'class':'ok' if status==200 and body.get('delay',0)>0 else 'timeout' if status==504 else 'failed'}
    except Exception as error:
        result={'target':url,'class':'controller','error':type(error).__name__}
    finally:connection.close()
    result['elapsed_ms']=round((time.monotonic()-started)*1000,2)
    observations.append(result)

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--mihomo',type=Path,required=True);parser.add_argument('--singbox',type=Path,required=True);parser.add_argument('--output',type=Path,required=True)
a=parser.parse_args();report={'scope':'native cores/controller + explicit proxy; no Tauri/system proxy/TUN/DNS modifications','engines':[]}
with upstream() as origin:
 for engine,binary in [('mihomo',a.mihomo),('sing-box',a.singbox)]:
  with tempfile.TemporaryDirectory(prefix='crossbow-quality-native-') as raw:
   folder=Path(raw);os.chmod(folder,0o700)
   mixed=free_port();controller=free_port(exclude=(mixed,));secret=secrets.token_hex(24)
   config,name=fixture(mixed,controller,secret,engine);config_path=folder/name;config_path.write_text(json.dumps(config));os.chmod(config_path,0o600)
   version=subprocess.run([str(binary),'version' if engine=='sing-box' else '-v'],capture_output=True,text=True,check=True).stdout.splitlines()[0]
   with Core(binary,folder,config_path,controller,secret,'core.log',engine) as core:
    url=f'http://127.0.0.1:{origin}/204'
    before=[]
    for _ in range(30):
     before.append(measured_fetch(core,mixed,url,timeout=3));time.sleep(.05)
    observations=[];active=threading.Event()
    def tests():
     active.set()
     targets=['http://www.gstatic.com/generate_204','https://www.gstatic.com/generate_204','https://cp.cloudflare.com/generate_204'] if engine=='mihomo' else ['https://www.gstatic.com/generate_204']*3
     for target in targets:measure(core,'DIRECT' if engine=='mihomo' else 'direct',target,observations)
    with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
     task=pool.submit(tests);active.wait(1);during=[];overlap=0
     for _ in range(30):
      overlap+=int(not task.done());during.append(measured_fetch(core,mixed,url,timeout=3));time.sleep(.05)
     task.result(timeout=25)
    report['engines'].append({'engine':engine,'version':version,'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'business_before':summarize(before),'business_during_quality':summarize(during),'business_requests_overlapping_quality':overlap,'quality_controller_requests':len(observations),'additional_requests_for_history':0,'note':'history observer/persistence tested separately in Rust; smoke replays equivalent fixed-target controller requests','quality':observations})
   assert core.process.poll() is not None,'owned core not reaped'
report['cleanup']='owned cores reaped; temporary configs removed'
a.output.parent.mkdir(parents=True,exist_ok=True);a.output.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
print(json.dumps(report,ensure_ascii=False,indent=2))
