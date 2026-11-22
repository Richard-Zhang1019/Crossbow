import assert from 'node:assert/strict';
import {mkdirSync} from 'node:fs';
import {startVite,stubSource,launchBrowser} from './verify-lib.mjs';
const out=process.env.CB_USER_EVIDENCE||'/tmp/cb605-ui';mkdirSync(out,{recursive:true});
const vite=await startVite(),browser=await launchBrowser(true);
try{
 const context=await browser.newContext({viewport:{width:1120,height:734}});await context.addInitScript(stubSource());const page=await context.newPage();
 const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await context.route('**/*',r=>new URL(r.request().url()).origin===new URL(vite.url).origin?r.continue():r.abort());
 await page.goto(vite.url);await page.locator('.cb-system-foot').waitFor();
 await page.evaluate(()=>{const f=window.__cb.userFixture={fail:false,rules:[{id:'a',kind:'DOMAIN_SUFFIX',value:'example.com',target:'DIRECT',enabled:true},{id:'b',kind:'IP_CIDR',value:'2001:db8::/64',target:'Work policy',enabled:false}]};window.__cb.setCommands({user_rules_get:()=>({rules:structuredClone(f.rules),targets:['DIRECT','REJECT','Work policy']}),user_rules_set:({rules,expected})=>{if(f.fail)throw Error('Synthetic persistence failure');if(JSON.stringify(expected)!==JSON.stringify(f.rules))throw Error('Stale rules');f.rules=structuredClone(rules);}});});
 await page.locator('.cb-primary-nav button',{hasText:'配置'}).click();
 if(process.env.CB_USER_BEFORE){await page.screenshot({path:out+'/before.png'});}else{
 await page.locator('.cb-subnav button',{hasText:'用户规则'}).click();await page.getByRole('table',{name:'用户规则'}).waitFor();await page.waitForTimeout(250);await page.screenshot({path:out+'/light.png'});
 await page.getByRole('button',{name:'下移',exact:true}).first().click();await page.waitForFunction(()=>window.__cb.userFixture.rules[0].id==='b');
 await page.getByRole('button',{name:'已停用',exact:true}).click();await page.waitForFunction(()=>window.__cb.userFixture.rules[0].enabled);
 await page.getByRole('button',{name:'编辑',exact:true}).first().click();const dialog=page.getByRole('dialog');await dialog.getByRole('textbox').fill('2001:db8:1234::/48');
 await page.evaluate(()=>window.__cb.userFixture.fail=true);await dialog.getByRole('button',{name:'保存',exact:true}).click();await page.waitForFunction(()=>window.__cb.userFixture.fail);await page.waitForTimeout(300);assert(await dialog.isVisible());assert.equal(await dialog.getByRole('textbox').inputValue(),'2001:db8:1234::/48');await page.screenshot({path:out+'/failed.png'});
 await page.evaluate(()=>window.__cb.userFixture.fail=false);await dialog.getByRole('button',{name:'保存',exact:true}).click();await page.waitForFunction(()=>window.__cb.userFixture.rules[0].value==='2001:db8:1234::/48');
 await page.evaluate(()=>{window.__cb.emit('ui://lang','en');window.__cb.emit('ui://theme','dark');});await page.setViewportSize({width:880,height:560});await page.getByText('User rules',{exact:true}).first().waitFor();await page.screenshot({path:out+'/dark-small.png'});
 await page.getByRole('button',{name:'Edit',exact:true}).first().click();await page.screenshot({path:out+'/editor.png'});await page.getByRole('dialog').getByRole('button',{name:'Cancel',exact:true}).click();await page.evaluate(()=>window.dispatchEvent(new CustomEvent('cb:goto',{detail:JSON.stringify({page:'profiles',control:'cb-user-rule',source:'2001:db8::1:443'})})));await page.getByRole('dialog').waitFor();assert.equal(await page.getByRole('dialog').getByRole('textbox').inputValue(),'2001:db8::1/128');
 assert.deepEqual(errors,[]);console.log('User-rule production component checks passed');
 }
}finally{await browser.close();await vite.close();}
