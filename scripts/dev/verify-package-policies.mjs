// Production profile dialogs with synthetic portable-policy reports; no files/network.
import assert from 'node:assert/strict';
import {mkdirSync} from 'node:fs';
import {startVite,stubSource,launchBrowser} from './verify-lib.mjs';
const out=process.env.CB_PACKAGE_EVIDENCE||'/tmp/cb607-ui';mkdirSync(out,{recursive:true});
const vite=await startVite(),browser=await launchBrowser(true);
try{
 const context=await browser.newContext({viewport:{width:1120,height:734}});await context.addInitScript(stubSource());await context.route('**/*',r=>new URL(r.request().url()).origin===new URL(vite.url).origin?r.continue():r.abort());const page=await context.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
 await page.goto(vite.url);await page.locator('.cb-system-foot').waitFor();
 await page.evaluate(()=>{const f=window.__cb.packageFixture={conflicts:0,fail:false,imports:0,cancels:0};window.__cb.setCommands({preview_config_package:()=>({id:'synthetic-preview',report:{added_profiles:2,replaced_profiles:1,added_overrides:1,replaced_overrides:0,selects_profile:true,added_process_rules:3,replaced_process_rules:1,added_rule_sets:2,replaced_rule_sets:1,policy_conflicts:f.conflicts,same_name_items:1,unverified_targets:1}}),import_config_package:()=>{f.imports++;if(f.fail)throw Error('pkg.errorCommit');},cancel_config_package:()=>{f.cancels++;},export_config_package:()=>false});});
 await page.locator('.cb-primary-nav button',{hasText:'配置'}).click();
 const openMenu=async()=>page.getByRole('button',{name:'备份与迁移',exact:true}).click();
 await openMenu();await page.getByRole('button',{name:'导出配置包',exact:false}).click();await page.locator('.cb-modal').waitFor();await page.screenshot({path:out+'/export.png'});await page.locator('.cb-modal').getByRole('button',{name:'取消',exact:true}).click();
 await openMenu();await page.getByRole('button',{name:'导入配置包',exact:false}).click();let dialog=page.locator('.cb-modal');await dialog.waitFor();
 if(!process.env.CB_PACKAGE_BEFORE){assert((await dialog.textContent()).includes('进程规则：新增 3'));assert((await dialog.textContent()).includes('规则集：新增 2'));}
 await page.screenshot({path:out+(process.env.CB_PACKAGE_BEFORE?'/before.png':'/preview.png')});
 await dialog.getByRole('button',{name:'取消',exact:true}).click();assert.equal(await page.evaluate(()=>window.__cb.packageFixture.imports),0);
 if(!process.env.CB_PACKAGE_BEFORE){
  await page.evaluate(()=>window.__cb.packageFixture.conflicts=1);await openMenu();await page.getByRole('button',{name:'导入配置包',exact:false}).click();await dialog.waitFor();assert(await dialog.getByRole('button',{name:'导入配置包',exact:true}).isDisabled());await page.screenshot({path:out+'/conflict.png'});await dialog.getByRole('button',{name:'取消',exact:true}).click();
  await page.evaluate(()=>{window.__cb.packageFixture.conflicts=0;window.__cb.packageFixture.fail=true;});await openMenu();await page.getByRole('button',{name:'导入配置包',exact:false}).click();await dialog.getByRole('button',{name:'导入配置包',exact:true}).click();await page.waitForFunction(()=>window.__cb.packageFixture.imports===1);await page.waitForTimeout(150);assert.equal(await dialog.count(),0);
  await page.evaluate(()=>{window.__cb.emit('ui://lang','en');window.__cb.emit('ui://theme','dark');});await page.setViewportSize({width:880,height:560});await page.locator('.cb-backup-trigger').click();await page.locator('.cb-backup-menu-item').filter({hasText:/Import/i}).click();await dialog.waitFor();await page.screenshot({path:out+'/dark-small.png'});assert(await dialog.evaluate(el=>el.scrollWidth<=el.clientWidth));
 }
 assert.deepEqual(errors,[]);console.log('Portable-policy production dialog checks passed');
}finally{await browser.close();await vite.close();}
