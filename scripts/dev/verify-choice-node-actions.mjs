// #647: production components, synthetic IPC; no real node or network changes.
import assert from 'node:assert/strict';
import {mkdirSync,writeFileSync} from 'node:fs';
import {startVite,stubSource,launchBrowser,revision} from './verify-lib.mjs';
const before=process.env.CB_CHOICE_BEFORE==='1';
const out=process.env.CB_CHOICE_EVIDENCE||'/tmp/cb647-preview';mkdirSync(out,{recursive:true});
const vite=await startVite(),browser=await launchBrowser(true);
try{
 const c=await browser.newContext({viewport:{width:1120,height:734}});await c.addInitScript(stubSource());
 await c.route('**/*',r=>new URL(r.request().url()).origin===new URL(vite.url).origin?r.continue():r.abort());
 await c.addInitScript(()=>{
  const nodes=Array.from({length:20},(_,i)=>({name:i===0?'🇯🇵 Tokyo · Selected':i===1?'🇸🇬 Singapore · A very long node name for overflow verification':`🇭🇰 Hong Kong · ${i+1}`,kind:'VLESS',alive:i!==2,delay_ms:i===2?0:90+i*8}));
  const f=window.__cb.choice={now:nodes[0].name,hold:false,fail:false,release:null};
  window.__cb.setCommands({proxies_test_snapshot:()=>({context:{revision:1,pid:4242},groups:[{name:'Node selection',kind:'Selector',now:f.now,nodes}]}),select_proxy:({name})=>{f.now=name;},begin_node_delay_test:()=>({id:'choice-task',budgetMs:30000}),test_node_delay_structured:()=>f.hold?new Promise(resolve=>{f.release=()=>resolve({class:f.fail?'failed':'ok',delayMs:f.fail?null:74,reviewed:false,target:'https://example.test',testedAtMs:Date.now(),sampleCount:1,detail:''});}):({class:'ok',delayMs:74,reviewed:false,target:'https://example.test',testedAtMs:Date.now(),sampleCount:1,detail:''}),end_node_delay_test:null,node_quality_report:{paused:false,epoch:0,retentionDays:7,capacity:8192,storedSamples:0,storageError:null,pendingSave:false,testTargets:['google-http','google-https','cloudflare-https'],targets:['google-http','google-https','cloudflare-https'].map(target=>({target,total:0,eligible:0,successes:0,timeouts:0,failures:0,controllerErrors:0,successRate:null,p50Ms:null,p95Ms:null,jitterMs:null,samples:[]}))}});
 });
 const p=await c.newPage(),errors=[];p.on('pageerror',e=>errors.push(e.message));await p.goto(vite.url);await p.locator('.cb-system-foot').waitFor();await p.locator('.cb-primary-nav button',{hasText:'节点'}).click();await p.locator('.cb-node-card').first().waitFor();
 const shot=async n=>{await p.screenshot({path:`${out}/${n}.png`,animations:'disabled'});};
 const chip=p.locator('.cb-node-filters .cb-filter-chip'),card=p.locator('.cb-node-card').first();
 await p.mouse.move(0,0);await shot('light-off');const chipWidth=(await chip.boundingBox()).width;
 await chip.click();await p.mouse.move(0,0);await shot('light-on');
 await card.hover();await shot('light-hover');
 if(!before){
  assert.equal(await chip.getAttribute('aria-pressed'),'true');assert.equal((await chip.boundingBox()).width,chipWidth,'filter selection must not change toolbar width');
  await card.locator('.cb-node-selected').waitFor({state:'visible'});
  await p.mouse.move(0,0);assert(await card.locator('.cb-node-test').evaluate(b=>Number(getComputedStyle(b).opacity)>0),'test is discoverable without hover');
  assert(await card.evaluate(c=>{const mark=c.querySelector('.cb-node-selected').getBoundingClientRect(),r=c.getBoundingClientRect();return r.right-mark.right<mark.width;}),'selected marker must sit at the normal card edge, without an empty action slot');
  await card.locator('.cb-node-test').focus();await p.keyboard.press('Space');await p.waitForFunction(()=>window.__cb.calls.test_node_delay_structured===1);
  assert.equal(await p.evaluate(()=>window.__cb.calls.select_proxy||0),0,'keyboard test must not select the node');
  await p.waitForFunction(()=>document.querySelector('.cb-node-test')?.getAttribute('aria-busy')!=='true');
  await p.evaluate(()=>{window.__cb.choice.hold=true;window.__cb.choice.fail=true;});await card.locator('.cb-node-test').click();await p.waitForFunction(()=>!!window.__cb.choice.release);
  assert(await card.locator('.cb-node-test').isDisabled());await card.locator('.cb-node-selected').waitFor({state:'visible'});await shot('testing');
  await p.evaluate(()=>window.__cb.choice.release());await p.waitForFunction(()=>document.querySelector('.cb-node-test')?.getAttribute('aria-busy')!=='true');
  assert.equal(await p.evaluate(()=>window.__cb.calls.select_proxy||0),0,'pointer test must not select the node');await card.locator('.cb-node-selected').waitFor({state:'visible'});
  const next=p.locator('.cb-node-card').nth(1);await next.focus();await p.keyboard.press('Enter');await p.waitForFunction(()=>window.__cb.calls.select_proxy===1);await next.locator('.cb-node-selected').waitFor({state:'visible'});
  assert.equal(await next.getAttribute('aria-pressed'),'true');
 }
 await p.evaluate(()=>{window.__cb.emit('ui://theme','dark');window.__cb.emit('ui://lang','en');});await p.setViewportSize({width:880,height:560});await p.getByRole('button',{name:'Reachable only',exact:true}).waitFor();await p.mouse.move(0,0);await shot('small-dark-en');
 if(!before){
  assert(await p.locator('.cb-node-filters').evaluate(e=>e.scrollWidth<=e.clientWidth));
  const test=p.locator('.cb-node-card').first().locator('.cb-node-test');await test.focus();await shot('small-keyboard');
  assert(await p.locator('.cb-node-card').evaluateAll(cs=>cs.every(c=>{const b=c.querySelector('.cb-node-test').getBoundingClientRect(),r=c.getBoundingClientRect();return b.left>=r.left&&b.right<=r.right&&b.bottom<=r.bottom;})),'test buttons stay inside minimum-height cards');
  await p.getByRole('button',{name:'Quality history',exact:true}).click();await p.getByRole('dialog').waitFor();await shot('quality-filters');
 }
 assert.deepEqual(errors,[]);
 writeFileSync(`${out}/checks.json`,JSON.stringify({revision:revision(),before,simulation:true,errors,checks:before?['baseline']:['stable-filter-width','hover-keeps-selected','always-discoverable-action','keyboard-and-pointer-tests-do-not-select','loading-preserves-selection','card-keyboard-selection','minimum-window-long-name-no-overflow','quality-filter-consumer']},null,2));
 console.log(out);
}finally{await browser.close();await vite.close();}
