// Production React components + isolated IPC. No real subscriptions/network takeover.
import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { startVite, stubSource, launchBrowser, artifactsDir, revision } from './verify-lib.mjs';
const out=process.env.CB_QUALITY_EVIDENCE || artifactsDir();mkdirSync(out,{recursive:true});
const before=process.env.CB_QUALITY_BEFORE==='1';
const vite=await startVite();const browser=await launchBrowser(true);const errors=[];
try {
 const ctx=await browser.newContext({viewport:{width:1120,height:734}});
 await ctx.route('**/*',r=>new URL(r.request().url()).origin===new URL(vite.url).origin?r.continue():r.abort());
 await ctx.addInitScript(stubSource());
 await ctx.addInitScript(()=>{
  const nodes=[{name:'HK · 稳定线路',kind:'Shadowsocks',alive:true,delay_ms:84},{name:'JP · 备用线路',kind:'Trojan',alive:true,delay_ms:132}];
  const targets=['google-http','google-https','cloudflare-https'];
  const empty=target=>({target,total:0,eligible:0,successes:0,timeouts:0,failures:0,controllerErrors:0,successRate:null,p50Ms:null,p95Ms:null,jitterMs:null,samples:[]});
  const sample={target:'google-http',total:8,eligible:7,successes:5,timeouts:2,failures:0,controllerErrors:1,successRate:5/7,p50Ms:30,p95Ms:120,jitterMs:45,samples:[{atMs:Date.now(),class:'controller',delayMs:null},{atMs:Date.now()-60000,class:'timeout',delayMs:null},{atMs:Date.now()-120000,class:'ok',delayMs:120},{atMs:Date.now()-180000,class:'ok',delayMs:30}]};
  const fixture=window.__cb.quality={paused:false,cleared:false,failPause:false,hold:false,release:null,holdHistory:false,historyRelease:null,lastTarget:null,targets};
  window.__cb.setCommands({
   proxies_test_snapshot:{context:{revision:1,pid:4242},groups:[{name:'节点选择',kind:'Selector',now:nodes[0].name,nodes}]},
   node_quality_report:({node})=>{
    const result={paused:fixture.paused,epoch:0,retentionDays:7,capacity:8192,storedSamples:fixture.cleared?0:8,storageError:null,pendingSave:false,testTargets:fixture.targets,targets:targets.map(t=>t==='google-http'&&!fixture.cleared&&node===nodes[0].name?sample:empty(t))};
    return fixture.holdHistory&&node===nodes[0].name?new Promise(resolve=>{fixture.historyRelease=()=>resolve(result);}):result;
   },
   node_quality_set_paused:({paused})=>{if(fixture.failPause)throw Error('synthetic write failure');fixture.paused=paused;},
   node_quality_clear:()=>{fixture.cleared=true;},
   begin_node_delay_test:({target})=>{fixture.lastTarget=target;return {id:'q-task',budgetMs:30000};},
   test_node_delay_structured:()=>fixture.hold?new Promise(resolve=>{fixture.release=()=>resolve({class:'ok',delayMs:22,reviewed:false,target:'https://www.gstatic.com/generate_204',testedAtMs:Date.now(),sampleCount:1,detail:''});}):({class:'ok',delayMs:22,reviewed:false,target:'https://www.gstatic.com/generate_204',testedAtMs:Date.now(),sampleCount:1,detail:''}),
  });
 });
 const p=await ctx.newPage();p.on('pageerror',e=>errors.push(e.message));await p.goto(vite.url);
 await p.locator('.cb-system-foot',{hasText:'Crossbow v'}).waitFor();
 await p.locator('.cb-primary-nav button',{hasText:'节点'}).click();await p.locator('.cb-node-card').first().waitFor();
 const shot=async name=>{await p.mouse.move(0,0);await p.screenshot({path:`${out}/${name}.png`,animations:'disabled'});};
 await shot(before?'before-light':'nodes-light');
 if(!before){
  const card=p.locator('.cb-node-card').first();const original=await card.innerText();
  await p.getByRole('button',{name:'质量历史',exact:true}).click();const dialog=p.getByRole('dialog');await dialog.getByText('71%',{exact:true}).waitFor();
  await shot('after-light');
  assert((await dialog.innerText()).includes('控制器异常（不计入成功率）'));
  assert.equal(await p.evaluate(()=>window.__cb.calls.test_node_delay_structured||0),0,'viewing history must never probe');
  await dialog.getByRole('button',{name:'Google · HTTPS',exact:true}).click();await dialog.getByText('0 次有效探测 · 0 次成功').waitFor();
  await dialog.getByRole('button',{name:'测一次',exact:true}).click();await dialog.getByRole('button',{name:'测一次',exact:true}).waitFor();
  assert.equal(await p.evaluate(()=>window.__cb.quality.lastTarget),'google-https');
  assert.equal(await p.evaluate(()=>window.__cb.calls.test_node_delay_structured),1);
  assert.equal(await card.innerText(),original,'separate target must not replace card/sort delay');
  await p.evaluate(()=>window.__cb.quality.hold=true);await dialog.getByRole('button',{name:'测一次',exact:true}).click();
  await dialog.getByRole('button',{name:'取消测速',exact:true}).click();
  await p.waitForFunction(()=>window.__cb.calls.end_node_delay_test>=2);await p.evaluate(()=>window.__cb.quality.release());
  await dialog.getByRole('button',{name:'测一次',exact:true}).waitFor();
  const toggle=dialog.getByRole('switch');await toggle.click();await p.waitForFunction(()=>window.__cb.quality.paused);
  assert.equal(await toggle.getAttribute('aria-checked'),'false');
  await p.evaluate(()=>window.__cb.quality.failPause=true);await toggle.click();
  await p.waitForFunction(()=>window.__cb.calls.node_quality_set_paused>=2);
  await dialog.getByRole('button',{name:'刷新质量历史'}).waitFor({state:'visible'});
  assert.equal(await toggle.getAttribute('aria-checked'),'false','failed preference write must not show enabled');
  await p.evaluate(()=>{window.__cb.quality.failPause=false;window.__cb.quality.targets=['google-https'];});
  await dialog.getByRole('button',{name:'刷新质量历史'}).click();
  await dialog.getByRole('button',{name:'Google · HTTP',exact:true}).click();
  await dialog.getByText('当前内核仅支持 Google HTTPS 测速；其他目标保持未测。').waitFor();
  assert(await dialog.getByRole('button',{name:'测一次',exact:true}).isDisabled());
  await p.evaluate(()=>{window.__cb.quality.targets=['google-http','google-https','cloudflare-https'];});
  await dialog.getByRole('button',{name:'刷新质量历史'}).click();
  await p.evaluate(()=>window.__cb.quality.holdHistory=true);
  await dialog.getByRole('button',{name:'刷新质量历史'}).click();await p.waitForFunction(()=>!!window.__cb.quality.historyRelease);
  await dialog.locator('.cb-quality-toolbar .cb-select-trigger').click();
  await p.getByRole('option',{name:'JP · 备用线路'}).click();
  await dialog.getByText('0 次有效探测 · 0 次成功').waitFor();
  await p.evaluate(()=>{window.__cb.quality.holdHistory=false;window.__cb.quality.historyRelease();});
  await p.waitForTimeout(50);assert((await dialog.innerText()).includes('0 次有效探测 · 0 次成功'),'late report from previous node must not overwrite current node');
  await dialog.locator('.cb-quality-toolbar .cb-select-trigger').click();await p.getByRole('option',{name:'HK · 稳定线路'}).click();
  await dialog.getByText('71%',{exact:true}).waitFor();
  await dialog.getByRole('button',{name:'清空历史',exact:true}).click();await dialog.getByRole('button',{name:'确认清空',exact:true}).click();
  await dialog.getByText('0 次有效探测 · 0 次成功').waitFor();await shot('empty');
  await p.evaluate(()=>{window.__cb.quality.cleared=false;window.__cb.emit('ui://theme','dark');});
  await dialog.getByRole('button',{name:'刷新质量历史'}).click();await dialog.getByText('71%',{exact:true}).waitFor();await shot('dark');
  await p.setViewportSize({width:880,height:560});await shot('small-dark');
  await dialog.getByRole('button',{name:'清空历史',exact:true}).scrollIntoViewIfNeeded();await shot('small-footer');
  await p.evaluate(()=>window.__cb.emit('ui://lang','en'));await dialog.getByText('Quality history',{exact:true}).waitFor();
  await p.setViewportSize({width:1120,height:734});await shot('english');
  await p.keyboard.press('Escape');assert.equal(await p.getByRole('dialog').count(),0);
  assert.equal(await p.evaluate(()=>document.activeElement?.textContent),'Quality history');
  assert.equal(await p.evaluate(()=>window.__cb.calls.select_proxy||0),0);
 }
 assert.deepEqual(errors,[]);assert.deepEqual(await p.evaluate(()=>window.__cb.unknown),[]);
 writeFileSync(`${out}/checks.json`,JSON.stringify({revision:revision(),simulation:true,before,errors,checks:before?['baseline']:['no-automatic-probe','controlled-target','separate-card-delay','cancel','late-node-report','pause-failure-real-state','clear','singbox-guard','light-dark-en','small-window','focus-restore']},null,2));
 console.log(out);
}finally{await browser.close();await vite.close();}
