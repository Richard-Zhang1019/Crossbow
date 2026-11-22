// #604 production component + synthetic IPC. No actual network tests or host-network changes.
import assert from 'node:assert/strict';
import {mkdirSync,writeFileSync} from 'node:fs';
import {startVite,stubSource,launchBrowser,artifactsDir,revision} from './verify-lib.mjs';
const out=process.env.CB_PROBES_EVIDENCE||artifactsDir();mkdirSync(out,{recursive:true});
const before=process.env.CB_PROBES_BEFORE==='1';
const vite=await startVite();const browser=await launchBrowser(true);const errors=[];
try{
 const ctx=await browser.newContext({viewport:{width:1120,height:734}});
 await ctx.route('**/*',r=>new URL(r.request().url()).origin===new URL(vite.url).origin?r.continue():r.abort());
 await ctx.addInitScript(stubSource());await ctx.addInitScript(()=>{
  const nodes=[{name:'HK · 测试节点',kind:'Shadowsocks',alive:true,delay_ms:84},{name:'JP · 备用',kind:'Trojan',alive:true,delay_ms:110}];
  const f=window.__cb.finite={jobs:{},history:{},last:null,count:0,epoch:0,paused:false,failState:false,holdTicket:false,release:null};
  const empty=target=>({target,total:0,eligible:0,successes:0,timeouts:0,failures:0,controllerErrors:0,successRate:null,p50Ms:null,p95Ms:null,jitterMs:null,samples:[]});
  const config={context:{revision:1,pid:4242},groups:[{name:'节点选择',kind:'Selector',now:nodes[0].name,nodes}]};
  f.report=kind=>({kind,atMs:Date.now(),elapsedMs:4300,tcp:{class:'ok',elapsedMs:82,packets:0},https:{class:'https_unverified',elapsedMs:500,packets:1},udp:{class:'udp_unverified',elapsedMs:3600,packets:3},download:{class:'ok',bytes:4194304,elapsedMs:2000,limitBytes:4194304,mbps:16.78},upload:{class:'incomplete',bytes:0,elapsedMs:1400,limitBytes:1048576,mbps:null}});
  f.complete=()=>{const j=f.jobs[f.last];j.state='completed';j.phase='finished';j.report=f.report(j.kind);if(!f.paused&&j.epoch===f.epoch)(f.history[j.node]??=[]).unshift(j.report);};
  window.__cb.setCommands({
   proxies_test_snapshot:()=>config,
   node_quality_report:()=>({paused:f.paused,epoch:f.epoch,retentionDays:7,capacity:8192,storedSamples:0,storageError:null,pendingSave:false,testTargets:['google-http','google-https','cloudflare-https'],targets:['google-http','google-https','cloudflare-https'].map(empty)}),
   node_quality_set_paused:({paused})=>{f.paused=paused;f.epoch++;},node_quality_clear:()=>{f.epoch++;f.history={};},
   node_probe_history:({node})=>structuredClone(f.history[node]??[]),
   start_node_probe:async({node,kind})=>{const id='finite-'+(++f.count);f.last=id;f.jobs[id]={id,node,kind,state:'running',phase:'preparing',report:null,error:null,epoch:f.epoch};return f.holdTicket?new Promise(resolve=>{f.release=()=>resolve(id);}):id;},
   node_probe_snapshot:({id})=>{if(f.failState)throw Error('synthetic IPC read failure');return structuredClone(f.jobs[id]??null);},
   cancel_node_probe:({id})=>{const j=f.jobs[id];if(j?.state==='running'){j.state='cancelled';j.report=null;j.phase='finished';}},
  });
 });
 const p=await ctx.newPage();p.on('pageerror',e=>errors.push(e.message));await p.goto(vite.url);await p.locator('.cb-system-foot',{hasText:'Crossbow v'}).waitFor();
 await p.locator('.cb-primary-nav button',{hasText:'节点'}).click();await p.getByRole('button',{name:'质量历史',exact:true}).click();
 const dialog=p.getByRole('dialog');await dialog.getByText('0 次有效探测 · 0 次成功').waitFor();
 const shot=async name=>{await p.mouse.move(0,0);await p.screenshot({path:`${out}/${name}.png`,animations:'disabled'});};
 if(before){await shot('before');}else{
  assert.equal(await p.evaluate(()=>window.__cb.calls.start_node_probe||0),0);
  await dialog.getByRole('button',{name:'分项测试',exact:true}).click();const panel=p.locator('.cb-node-probes');await panel.waitFor();await panel.scrollIntoViewIfNeeded();await shot('empty-light');
  assert.equal(await p.evaluate(()=>window.__cb.calls.start_node_probe||0),0,'opening panel/history must never start probes');
  const begin=()=>panel.getByRole('button',{name:'测试 TCP / HTTPS / UDP',exact:true}).click();
  await begin();await panel.getByRole('button',{name:'取消测试',exact:true}).waitFor();
  assert.equal(await dialog.getByRole('button',{name:'测一次',exact:true}).count(),0,'latency controls must not imply a target for fixed component tests');
  await p.evaluate(()=>{const f=window.__cb.finite;f.jobs[f.last].phase='https';});await panel.getByText('正在验证 HTTPS…',{exact:true}).waitFor();
  await p.evaluate(()=>window.__cb.finite.complete());await panel.getByText('HTTPS 未验证（含 TLS / 目标响应）',{exact:true}).waitFor();
  assert((await panel.innerText()).includes('未收到有效响应，UDP 可用性未验证'));
  await panel.scrollIntoViewIfNeeded();await shot('connectivity-light');
  await panel.getByRole('button',{name:'有界吞吐',exact:true}).click();assert((await panel.innerText()).includes('4 MiB'));
  await panel.getByRole('button',{name:'开始下载和上传测试',exact:true}).click();await p.evaluate(()=>{const f=window.__cb.finite;f.jobs[f.last].phase='upload';});await panel.getByText('正在进行有界上传…',{exact:true}).waitFor();await shot('uploading');
  await p.evaluate(()=>window.__cb.finite.complete());await panel.getByText('上传未确认，可能已发送部分负载',{exact:true}).waitFor();assert((await panel.innerText()).includes('16.78 Mbps'));await shot('transfer-light');
  await panel.getByRole('button',{name:'连通性',exact:true}).click();
  await p.evaluate(()=>window.__cb.finite.holdTicket=true);const cancelledBefore=await p.evaluate(()=>window.__cb.calls.cancel_node_probe||0);
  await begin();await panel.getByRole('button',{name:'取消测试',exact:true}).click();await p.evaluate(()=>{window.__cb.finite.holdTicket=false;window.__cb.finite.release();});
  await p.waitForFunction(n=>(window.__cb.calls.cancel_node_probe||0)>n,cancelledBefore);await panel.getByText('测试已取消，未写入新记录。',{exact:true}).waitFor();
  await p.evaluate(()=>window.__cb.finite.failState=true);await begin();await panel.getByRole('button',{name:'重试读取状态',exact:true}).waitFor();assert.equal(await dialog.getByRole('button',{name:'测一次',exact:true}).count(),0);
  await p.evaluate(()=>{window.__cb.finite.failState=false;window.__cb.finite.complete();});await panel.getByRole('button',{name:'重试读取状态',exact:true}).click();await panel.getByText('HTTPS 未验证（含 TLS / 目标响应）',{exact:true}).waitFor();
  await dialog.getByRole('button',{name:'清空历史',exact:true}).click();await dialog.getByRole('button',{name:'确认清空',exact:true}).click();await panel.getByText('尚无此类测试记录。选择测试后再显式开始。',{exact:true}).waitFor();
  await begin();const closeCount=await p.evaluate(()=>window.__cb.calls.cancel_node_probe||0);await p.keyboard.press('Escape');await p.waitForFunction(n=>(window.__cb.calls.cancel_node_probe||0)>n,closeCount);assert.equal(await p.getByRole('dialog').count(),0);
  await p.getByRole('button',{name:'质量历史',exact:true}).click();await dialog.getByText('0 次有效探测 · 0 次成功').waitFor();await dialog.getByRole('button',{name:'分项测试',exact:true}).click();await panel.waitFor();
  await p.evaluate(()=>{window.__cb.emit('ui://theme','dark');window.__cb.emit('ui://lang','en');});await panel.getByText('On demand only',{exact:true}).waitFor();await p.setViewportSize({width:880,height:560});await panel.scrollIntoViewIfNeeded();await shot('small-dark-en');
  const clear=dialog.getByRole('button',{name:'Clear history',exact:true});assert(await clear.isVisible(),'privacy controls stay reachable');
  assert.equal(await p.evaluate(()=>window.__cb.calls.select_proxy||0),0,'tests never select current proxy');
 }
 assert.deepEqual(errors,[]);assert.deepEqual(await p.evaluate(()=>window.__cb.unknown),[]);
 writeFileSync(`${out}/checks.json`,JSON.stringify({revision:revision(),before,simulation:true,errors,checks:before?['baseline']:['no-automatic-probe','TCP-HTTPS-UDP-separate','payload-warning','partial-upload','late-ticket-cancel','status-read-failure','history-clear','close-cancel','small-dark-en','no-selection-side-effects']},null,2));console.log(out);
}finally{await browser.close();await vite.close();}
