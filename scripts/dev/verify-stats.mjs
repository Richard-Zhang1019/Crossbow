import {startVite,stubSource,launchBrowser,sleep,artifactsDir} from './verify-lib.mjs';
import {mkdirSync,writeFileSync} from 'node:fs';
const after=process.env.STATS_AFTER !== '0';
const dir=process.env.STATS_EVIDENCE || artifactsDir();mkdirSync(dir,{recursive:true});
const vite=await startVite();const browser=await launchBrowser(true);
const MB=1024**2; const daily=Array.from({length:7},(_,i)=>({key:`2026-11-${14+i}`,up:(i+1)*12*MB,down:[320,190,650,120,430,170,880][i]*MB}));
const report={daily,domains:['youtube.com','github.com','chatgpt.com','npmjs.org','x.com'].map((key,i)=>({key,up:[180,70,50,20,16][i]*MB,down:[1200,550,400,300,310][i]*MB})),nodes:[{key:'香港 · 家宽出口',up:220*MB,down:1800*MB},{key:'日本 · IPLC',up:100*MB,down:800*MB},{key:'DIRECT',up:16*MB,down:160*MB}]};
try { const context=await browser.newContext({viewport:{width:1120,height:720}});await context.addInitScript(stubSource());const p=await context.newPage();const errors=[];p.on('pageerror',e=>errors.push(String(e)));await p.goto(vite.url);await p.locator('.cb-system-foot').waitFor();await p.evaluate(report=>window.__cb.setCommands({traffic_stats:report}),report);await p.locator('.cb-primary-nav button',{hasText:'监测'}).click();await p.getByRole('button',{name:'统计',exact:true}).click();await p.locator('.cb-stats-page').waitFor();await sleep(400);await p.screenshot({path:dir+'/light.png'});
if(after){const html=await p.content();writeFileSync(dir+'/preview.html',html.replace(/<script[^>]*>[\s\S]*?<\/script>/g,''));}
if(after){
 for(const [name,w,h,theme,lang] of [['dark',1120,720,'dark','zh'],['small',880,560,'light','zh'],['wide',1440,900,'light','en'],['small-en',880,560,'dark','en']]){await p.setViewportSize({width:w,height:h});await p.evaluate(({theme,lang})=>{window.__cb.emit('ui://theme',theme);window.__cb.emit('ui://lang',lang);},{theme,lang});await sleep(300);await p.screenshot({path:`${dir}/${name}.png`});}
 await p.evaluate(()=>window.__cb.emit('ui://lang','zh'));
 await p.setViewportSize({width:1120,height:720});
 const bars=p.locator('.cb-stats-day');await bars.first().click();if(await bars.first().getAttribute('aria-pressed') !== 'true')throw Error('date selection missing');
 await p.evaluate(()=>{window.__cb.waitStats=[];window.__cb.setCommands({traffic_stats:({days})=>new Promise(resolve=>window.__cb.waitStats.push({days,resolve}))});});
 await p.getByRole('radio',{name:'今日',exact:true}).click();await p.getByRole('radio',{name:'近 30 天',exact:true}).click();await sleep(50);
 await p.evaluate(report=>{window.__cb.waitStats.find(r=>r.days===30).resolve(report)},report);await sleep(80);await p.evaluate(()=>window.__cb.waitStats.find(r=>r.days===1).resolve({daily:[],domains:[],nodes:[]}));await sleep(80);if(!await p.getByText('youtube.com',{exact:true}).isVisible())throw Error('old request replaced new data');
 await p.evaluate(()=>window.__cb.setCommands({traffic_stats:()=>Promise.reject('read failed')}));await p.getByRole('radio',{name:'今日',exact:true}).click();await sleep(150);await p.screenshot({path:dir+'/error.png'});
 await p.evaluate(()=>window.__cb.setCommands({traffic_stats:{daily:[],domains:[],nodes:[]}}));await p.getByRole('button',{name:'重试',exact:true}).click();await sleep(150);await p.screenshot({path:dir+'/empty.png'});
}
if(after){
 await p.evaluate(report=>{
  const f=window.__cb.statsPrivacy={paused:false,failRead:false,failWrite:false,clear:false};
  window.__cb.setCommands({
   traffic_stats:()=>f.clear?{daily:[],domains:[],nodes:[]}:report,
   traffic_stats_control:()=>{if(f.failRead)throw Error('Simulated control read failure');return {paused:f.paused}},
   traffic_stats_set_paused:async({paused})=>{await new Promise(r=>setTimeout(r,150));if(f.failWrite)throw Error('Simulated write failure');f.paused=paused;},
   traffic_stats_clear:()=>{if(f.failWrite)throw Error('Simulated clear failure');f.clear=true;},
  });
 },report);
 await p.getByRole('button',{name:'刷新统计',exact:true}).click();await sleep(150);
 const pause=p.getByRole('button',{name:'暂停记录',exact:true});
 await p.evaluate(()=>window.__cb.setCommands({traffic_stats_control:()=>new Promise(resolve=>{window.__cb.oldControl=resolve})}));
 await p.getByRole('button',{name:'刷新统计',exact:true}).click();
 await p.waitForFunction(()=>Boolean(window.__cb.oldControl));
 await pause.click();if(!await pause.isDisabled())throw Error('pending control must be disabled');
 await p.getByRole('button',{name:'继续记录',exact:true}).waitFor();
 await p.evaluate(()=>{window.__cb.oldControl({paused:false});window.__cb.setCommands({traffic_stats_control:()=>{const f=window.__cb.statsPrivacy;if(f.failRead)throw Error('Simulated read failure');return {paused:f.paused}}})});await sleep(50);
 if(!await p.getByRole('button',{name:'继续记录',exact:true}).isVisible())throw Error('stale control read replaced confirmed pause');
 await p.screenshot({path:dir+'/paused.png'});
 await p.getByRole('button',{name:'继续记录',exact:true}).click();await pause.waitFor();
 await p.evaluate(()=>{window.__cb.statsPrivacy.failRead=true});
 await p.getByRole('button',{name:'刷新统计',exact:true}).click();await sleep(100);
 if(!await pause.isDisabled())throw Error('unknown pause state must disable control');
 await p.evaluate(()=>{window.__cb.statsPrivacy.failRead=false;window.__cb.statsPrivacy.failWrite=true});
 await p.getByRole('button',{name:'刷新统计',exact:true}).click();await sleep(100);
 await pause.click();await sleep(220);
 if(!await pause.isVisible())throw Error('failed pause must not pretend to succeed');
 await p.evaluate(()=>{window.__cb.statsPrivacy.failWrite=false});
 await p.getByRole('button',{name:'清除本地统计',exact:true}).click();await sleep(150);
 if(await p.getByText('youtube.com',{exact:true}).isVisible())throw Error('clear did not refresh confirmed data');
}
if(errors.length)throw Error(errors.join('\n'));console.log('Stats production preview passed: '+dir);
}finally{await browser.close();await vite.close();}
