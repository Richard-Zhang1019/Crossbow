const {startVite,stubSource,launchBrowser}=await import(new URL('../../../../scripts/dev/verify-lib.mjs',import.meta.url));
import {mkdirSync,writeFileSync} from 'node:fs';
const dir=process.env.STATS_EVIDENCE || '/tmp/crossbow-stats-toolbar-preview';mkdirSync(dir,{recursive:true});
const vite=await startVite(), browser=await launchBrowser(true);
try {
 const c=await browser.newContext({viewport:{width:1120,height:720}});await c.addInitScript(stubSource());
 await c.route('**/*',r=>new URL(r.request().url()).origin===new URL(vite.url).origin?r.continue():r.abort());
 const p=await c.newPage();await p.goto(vite.url);await p.locator('.cb-system-foot').waitFor();
 await p.evaluate(()=>window.__cb.setCommands({traffic_stats:{daily:[{key:new Date().toLocaleDateString('sv-SE'),up:6000000000,down:3000000000}],domains:[{key:'example.test',up:6000000000,down:3000000000}],nodes:[{key:'Synthetic Tokyo',up:6000000000,down:3000000000}]}}));
 await p.locator('.cb-primary-nav button',{hasText:'监测'}).click();await p.getByRole('button',{name:'统计',exact:true}).click();await p.locator('.cb-stats-overview').waitFor();
 const checks=[];
 for (const [name,w,h,theme,lang] of [['light',1120,720,'light','zh'],['dark',1120,720,'dark','zh'],['small-en',880,560,'dark','en']]) {
  await p.setViewportSize({width:w,height:h});await p.evaluate(({theme,lang})=>{window.__cb.emit('ui://theme',theme);window.__cb.emit('ui://lang',lang);},{theme,lang});
  await p.waitForTimeout(200);
  checks.push(await p.locator('.cb-stats-controls').evaluate((e,name)=>({name,width:innerWidth,height:innerHeight,overflow:e.scrollWidth>e.clientWidth,controls:[e.querySelector('.cb-segments'),...e.querySelectorAll('.cb-button')].map(b=>({label:b.textContent||b.getAttribute('aria-label'),height:b.getBoundingClientRect().height,font:getComputedStyle(b.matches('.cb-segments')?b.querySelector('button'):b).fontSize}))}),name));
  await p.screenshot({path:`${dir}/${name}.png`});
 }
 writeFileSync(`${dir}/checks.json`,JSON.stringify(checks,null,2));console.log(JSON.stringify(checks));
}finally{await browser.close();await vite.close();}
