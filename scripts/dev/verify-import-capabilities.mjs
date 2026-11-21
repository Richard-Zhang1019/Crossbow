import assert from 'node:assert/strict';
import {mkdirSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {startVite,stubSource,launchBrowser} from './verify-lib.mjs';
const out=process.env.CB_IMPORT_EVIDENCE||join(process.env.CB_VERIFY_ARTIFACTS_ROOT||tmpdir(),'import-capabilities');mkdirSync(out,{recursive:true});
const vite=await startVite();const browser=await launchBrowser(true);
try{
 const context=await browser.newContext({viewport:{width:1120,height:734}});
 await context.route('**/*',route=>new URL(route.request().url()).origin===new URL(vite.url).origin?route.continue():route.abort());
 await context.addInitScript(stubSource());const page=await context.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.goto(vite.url);await page.locator('.cb-system-foot').waitFor();
 await page.evaluate(()=>{
  const f=window.__cb.importFixture={engine:'mihomo',fail:false,pending:[],imports:0};
  window.__cb.setCommands({
   inspect_profile_import:({content})=>{
    if(f.fail)throw Error('Simulated inspection failure');
    const report={engine:f.engine,tested_version:'mihomo v1.19.32 / sing-box v1.14.2',node_count:content.trim()?1:0,protocols:['tuic'],status:!content.trim()?'empty':content.startsWith('https://')?'remote':content.includes('bad')?'invalid':f.engine==='singbox'?'unsupported':'compatible',capabilities:[{protocol:'hy2',mihomo:'subset',singbox:'limited'},{protocol:'tuic',mihomo:'subset',singbox:'unsupported'},{protocol:'reality',mihomo:'subset',singbox:'unsupported'}]};
    if(content.includes('delayed'))return new Promise(resolve=>f.pending.push(()=>resolve(report)));
    return report;
   },core_binary_info_for:({engine})=>({source:'builtin',version:engine==='singbox'?'sing-box v1.14.2 (fixture)':'mihomo v1.19.32 (fixture)'}),import_profile_url:()=>{f.imports++;throw Error('Simulated final engine preflight rejected changed engine');}
  });
 });
 await page.locator('.cb-primary-nav button',{hasText:'配置'}).click();
 await page.getByRole('button',{name:'添加订阅',exact:true}).first().click();
 const dialog=page.getByRole('dialog');const input=dialog.locator('textarea');const submit=dialog.getByRole('button',{name:'＋ 导入',exact:true});
 if(process.env.CB_IMPORT_BEFORE){await input.fill('tuic://00000000-0000-4000-8000-000000000001:test@node.example:443');await page.screenshot({path:out+'/before.png'});console.log('Before capture');}
 else{
  assert(await submit.isDisabled());
  await input.fill('tuic://00000000-0000-4000-8000-000000000001:test@node.example:443');await page.getByText('已通过当前配置预检',{exact:false}).waitFor();assert(await submit.isEnabled());
  await page.screenshot({path:out+'/light.png'});
  await dialog.locator('summary').click();await page.screenshot({path:out+'/expanded.png'});await dialog.locator('summary').click();
  await input.fill('delayed');await page.waitForFunction(()=>window.__cb.importFixture.pending.length===1);
  await input.fill('bad');await page.getByText('格式或参数不支持',{exact:false}).waitFor();
  await page.evaluate(()=>window.__cb.importFixture.pending.shift()());assert(await submit.isDisabled());
  await page.evaluate(()=>{window.__cb.importFixture.engine='singbox';window.__cb.emit('engine://changed','singbox');});
  await input.fill('tuic://00000000-0000-4000-8000-000000000001:test@node.example:443');await page.getByText('当前内核无法保留',{exact:false}).waitFor();assert(await submit.isDisabled());
  await input.press('Meta+Enter');assert.equal(await page.evaluate(()=>window.__cb.importFixture.imports),0);
  await page.evaluate(()=>{window.__cb.emit('ui://lang','en');window.__cb.emit('ui://theme','dark');});await page.setViewportSize({width:880,height:560});
  await page.getByText('The current engine cannot preserve',{exact:false}).waitFor();await page.screenshot({path:out+'/dark-small.png'});
  assert(await dialog.evaluate(el=>el.scrollWidth<=el.clientWidth),'dialog must fit the minimum width');
  await page.evaluate(()=>{window.__cb.importFixture.fail=true;window.__cb.emit('engine://changed','mihomo');});
  await page.getByRole('button',{name:'Retry check',exact:true}).waitFor();await page.screenshot({path:out+'/failed.png'});
  await page.evaluate(()=>{window.__cb.importFixture.fail=false;window.__cb.importFixture.engine='mihomo';});await page.getByRole('button',{name:'Retry check',exact:true}).click();
  await page.getByText('Current configuration preflight passed',{exact:false}).waitFor();await dialog.getByRole('button',{name:'＋ Import',exact:true}).click();
  await page.waitForFunction(()=>window.__cb.importFixture.imports===1);assert(await dialog.isVisible(),'rejected final preflight must preserve input');assert((await input.inputValue()).startsWith('tuic://'));
  await input.fill('https://subscription.example.test/path');await page.getByText('Subscription compatibility is checked',{exact:false}).waitFor();
  assert.deepEqual(await page.evaluate(()=>window.__cb.unknown),[]);assert.deepEqual(errors,[]);console.log('Production import capability checks passed');
 }
}finally{await browser.close();await vite.close();}
