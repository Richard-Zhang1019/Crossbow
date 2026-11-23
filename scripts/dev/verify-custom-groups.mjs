import assert from 'node:assert/strict';
import {mkdirSync} from 'node:fs';
import {startVite,stubSource,launchBrowser} from './verify-lib.mjs';
const out=process.env.CB_GROUP_EVIDENCE||'/tmp/cb606-ui';mkdirSync(out,{recursive:true});
const vite=await startVite(),browser=await launchBrowser(true);
try{
 const context=await browser.newContext({viewport:{width:1120,height:734}});await context.addInitScript(stubSource());await context.route('**/*',r=>new URL(r.request().url()).origin===new URL(vite.url).origin?r.continue():r.abort());const page=await context.newPage(),errors=[];page.on('pageerror',e=>errors.push(e.message));await page.goto(vite.url);await page.locator('.cb-system-foot').waitFor();
 await page.evaluate(()=>{const f=window.__cb.groupFixture={groups:[],fail:false,changed:false,saves:0};window.__cb.setCommands({proxies_test_snapshot:()=>({context:{revision:1,pid:4242},groups:[{name:'Existing',kind:'Selector',now:'A',nodes:[{name:'A',kind:'http',alive:true,delay_ms:12},{name:'B',kind:'http',alive:true,delay_ms:20}]}]}),custom_groups_get:()=>({groups:structuredClone(f.groups),sources:[['p','Synthetic source']],profile_id:'p',profile_name:'Synthetic source',candidates:['A','B'],context:f.changed?'new-source':'source',error:null,resolutions:f.groups.map(g=>[g.id,{members:g.membership==='filter'?['A','B'].filter(n=>n.toLowerCase().includes(g.filter.toLowerCase())):g.members,missing:[],error:null}])}),preview_custom_group:async({draft,sourceContext})=>{await new Promise(r=>setTimeout(r,60));if(f.changed||sourceContext!=='source')throw Error('Source changed; reload');return {members:draft.membership==='filter'?['A','B'].filter(n=>n.toLowerCase().includes(draft.filter.toLowerCase())):draft.members,missing:[],error:!draft.name?'Name required':draft.membership==='explicit'&&!draft.members.length?'No concrete nodes':null};},custom_groups_set:({groups})=>{f.saves++;if(f.fail)throw Error('Synthetic save failed');f.groups=structuredClone(groups);}});});
 await page.locator('.cb-primary-nav button',{hasText:'节点'}).click();await page.locator('.cb-proxies-page').waitFor();await page.locator('.cb-node-card').first().waitFor();
 if(process.env.CB_GROUP_BEFORE){await page.screenshot({path:out+'/before.png'});}else{
 // #634：入口在「策略组」标题行内，不再是标题与导航之间的独立一行。
 assert.equal(await page.locator('.cb-node-heading-actions button',{hasText:'自定义组'}).count(),1,'自定义组入口应在节点页右上角动作区');
 assert.equal(await page.locator('.cb-node-nav-head, .cb-node-custom-manage').count(),0,'侧栏不应再有入口行');
 await page.getByRole('button',{name:'自定义组',exact:true}).click();const dialog=page.locator('.cb-modal');await dialog.waitFor();
 await page.screenshot({path:out+'/list-entry.png'});
 assert.equal(await dialog.locator('.cb-modal-footer').count(),0,'空列表没有虚构底栏');
 assert.equal(await dialog.locator('.cb-custom-help[open]').count(),0,'说明默认折叠');
 const toolbarHeights=await dialog.locator('.cb-process-actions button').evaluateAll(bs=>bs.slice(0,2).map(b=>Math.round(b.getBoundingClientRect().height)));
 assert.equal(toolbarHeights.length,2,'工具条应有刷新与添加组');assert.equal(toolbarHeights[0],toolbarHeights[1],'刷新与添加组应同高');
 await page.getByRole('button',{name:'添加组',exact:true}).click();await dialog.getByRole('textbox',{name:'组名',exact:true}).fill('Personal');
 const combos=dialog.getByRole('combobox');await combos.nth(1).click();await page.getByRole('option',{name:'名称包含（非正则）',exact:true}).click();await dialog.getByRole('textbox',{name:'名称包含（非正则）',exact:true}).fill('A');await dialog.locator('.cb-custom-preview').getByText('A',{exact:true}).waitFor();await page.screenshot({path:out+'/preview.png'});
 await page.evaluate(()=>window.__cb.groupFixture.fail=true);await dialog.getByRole('button',{name:'保存',exact:true}).click();await page.waitForFunction(()=>window.__cb.groupFixture.saves===1);await page.waitForTimeout(150);assert.equal(await dialog.getByRole('textbox',{name:'组名',exact:true}).inputValue(),'Personal');await page.screenshot({path:out+'/failure.png'});
 await page.evaluate(()=>window.__cb.groupFixture.fail=false);await dialog.getByRole('button',{name:'保存',exact:true}).click();await dialog.getByRole('button',{name:'编辑',exact:true}).waitFor();await page.screenshot({path:out+'/list.png'});
 // #641: only separators between rows, never a trailing rule masquerading as a footer.
 const rowBorders=()=>dialog.locator('.cb-custom-list-row').evaluateAll(rows=>rows.map(row=>Number.parseFloat(getComputedStyle(row).borderBottomWidth)));
 assert.deepEqual(await rowBorders(),[0],'单行列表不应显示底部尾线');
 assert.equal(await dialog.locator('.cb-modal-footer').count(),0,'列表态不渲染公共 Modal 底栏');
 await page.evaluate(()=>{const f=window.__cb.groupFixture;f.groups=[f.groups[0],{...f.groups[0],id:'second',name:'Second'},{...f.groups[0],id:'third',name:'Third'}];});
 await dialog.getByRole('button',{name:'刷新',exact:true}).click();await dialog.locator('.cb-custom-list-row').nth(2).waitFor();
 assert.deepEqual(await rowBorders(),[1,1,0],'多行列表保留行间分隔，末行不画尾线');
 await page.screenshot({path:out+'/multi-row.png'});
 await page.evaluate(()=>{window.__cb.groupFixture.groups=window.__cb.groupFixture.groups.slice(0,1);});
 await dialog.getByRole('button',{name:'刷新',exact:true}).click();await page.waitForFunction(()=>document.querySelectorAll('.cb-custom-list-row').length===1);

 await dialog.getByRole('button',{name:'编辑',exact:true}).click();await page.evaluate(()=>window.__cb.groupFixture.changed=true);await dialog.getByRole('textbox',{name:'组名',exact:true}).fill('Personal updated');await dialog.locator('.cb-custom-preview').getByText('Source changed; reload',{exact:false}).waitFor();assert(await dialog.getByRole('button',{name:'保存',exact:true}).isDisabled());await dialog.getByRole('button',{name:'取消',exact:true}).click();
 await page.evaluate(()=>{window.__cb.groupFixture.changed=false;window.__cb.emit('ui://lang','en');window.__cb.emit('ui://theme','dark');});await page.setViewportSize({width:880,height:560});await dialog.getByRole('button',{name:'Edit',exact:true}).click();await dialog.getByRole('textbox',{name:'Group name',exact:true}).waitFor();await dialog.locator('.cb-custom-preview').getByText('A',{exact:true}).waitFor();await page.screenshot({path:out+'/dark-small.png'});await dialog.getByRole('combobox').first().click();await page.getByRole('option',{name:'URL test',exact:true}).click();await dialog.getByRole('textbox',{name:'Test URL',exact:true}).waitFor();await dialog.locator('.cb-custom-preview').getByText('A',{exact:true}).waitFor();await page.screenshot({path:out+'/auto-small.png'});await dialog.getByRole('button',{name:'Save',exact:true}).scrollIntoViewIfNeeded();assert(await dialog.getByRole('button',{name:'Save',exact:true}).isVisible());await page.screenshot({path:out+'/auto-small-bottom.png'});assert(await dialog.evaluate(e=>e.scrollWidth<=e.clientWidth));await dialog.getByRole('button',{name:'Cancel',exact:true}).click();await dialog.getByRole('button',{name:'Edit',exact:true}).waitFor();
 assert.deepEqual(await rowBorders(),[0],'深色最小窗口列表也不显示尾线');
 await dialog.locator('.cb-custom-help summary').click();await page.screenshot({path:out+'/list-dark-small.png'});
 assert.equal(await dialog.locator('.cb-modal-footer').count(),0);
 assert.deepEqual(errors,[]);console.log('Custom group production checks passed');
 }
}finally{await browser.close();await vite.close();}
