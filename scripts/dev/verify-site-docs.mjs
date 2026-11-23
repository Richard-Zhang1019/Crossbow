// Real static Astro output. No user profiles or desktop IPC.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {readFileSync,existsSync,mkdirSync,writeFileSync,statSync} from 'node:fs';
import {resolve,extname} from 'node:path';
import {launchBrowser} from './verify-lib.mjs';
const dist=resolve('apps/site/dist'),out=process.env.CB_SITE_EVIDENCE||'/tmp/crossbow-site-docs';mkdirSync(out,{recursive:true});
const before=process.env.CB_SITE_BEFORE==='1',base=(process.env.SITE_BASE||'').replace(/\/$/,'');
const server=createServer((req,res)=>{let path=decodeURIComponent(new URL(req.url,'http://localhost').pathname);if(base){if(!path.startsWith(base+'/')){res.writeHead(404);return res.end();}path=path.slice(base.length);}
 const f=resolve(dist,'.'+path);if(!f.startsWith(dist+'/')&&f!==dist){res.writeHead(403);return res.end();}
 const file=[f,f+'.html',f+'/index.html'].find(p=>existsSync(p)&&statSync(p).isFile());if(!file){res.writeHead(404);return res.end('Not found');}
 res.setHeader('Content-Type',({'.html':'text/html','.js':'text/javascript','.css':'text/css','.svg':'image/svg+xml','.png':'image/png','.woff2':'font/woff2'})[extname(file)]||'application/octet-stream');res.end(readFileSync(file));});
await new Promise(r=>server.listen(0,'127.0.0.1',r));const origin=`http://127.0.0.1:${server.address().port}`;const browser=await launchBrowser(true);
try{
 const context=await browser.newContext({viewport:{width:1440,height:1000}});await context.route('**/*',r=>new URL(r.request().url()).origin===origin?r.continue():r.abort());
 const p=await context.newPage(),errors=[];p.on('pageerror',e=>errors.push(e.message));
 await p.goto(origin+base+'/zh/guide');await p.screenshot({path:out+'/overview-light.png',fullPage:before});
 if(before){await p.setViewportSize({width:390,height:844});await p.screenshot({path:out+'/mobile.png'});console.log(out);}
 else{
  const paths=await p.locator('.docs-navigation a').evaluateAll(as=>as.map(a=>a.getAttribute('href')));
  // Check complete generated documents and their local links, not just the overview screenshot.
  let pages=0;const broken=[];
  for(const lang of ['zh','en'])for(const first of paths){
   const path=lang==='zh'?first:first.replace('/zh/guide','/guide');
   const response=await context.request.get(origin+path);assert.equal(response.status(),200,path);const html=await response.text();
   assert.equal((html.match(/<h1[ >]/g)||[]).length,1,path+' has one title');assert(html.includes(`lang="${lang==='zh'?'zh-CN':'en'}"`),path+' language');
   assert(html.includes('rel="canonical"')&&html.includes('hreflang="zh-CN"')&&html.includes('hreflang="en"'),path+' metadata');
   const checks=[...html.matchAll(/href="([^"]+)"/g)].map(m=>m[1].replaceAll('&amp;','&')).filter(h=>h.startsWith(base+'/')&&!h.startsWith('//'));
   for(const h of new Set(checks)){const url=new URL(h,origin);const target=await context.request.get(url.href);if(target.status()!==200)broken.push(url.pathname);}
   pages++;
  }
  assert.deepEqual([...new Set(broken)],[],'internal links');
  await p.goto(origin+base+'/zh/guide/dns');assert.equal(await p.locator('.docs-navigation [aria-current="page"]').count(),1);
  const top=await p.locator('.docs-sidebar').boundingBox();await p.locator('.docs-article').evaluate(e=>e.scrollTop=450);assert.equal((await p.locator('.docs-sidebar').boundingBox()).y,top.y,'directory remains while article scrolls');await p.locator('.docs-article').evaluate(e=>e.scrollTop=0);
  assert(await p.locator('.docs-sidebar-body').evaluate(e=>e.scrollHeight>e.clientHeight),'directory has its own scroll area');assert(await p.evaluate(()=>document.documentElement.scrollHeight<=innerHeight),'workspace does not grow with directory');await p.screenshot({path:out+'/dns-light.png',fullPage:true});
  await p.locator('.docs-onpage nav a').last().click();assert(await p.locator('.docs-article').evaluate(e=>e.scrollTop>0),'section links scroll the article');await p.locator('.docs-article').evaluate(e=>e.scrollTop=0);
  await p.locator('.docs-pagination a').last().click();await p.waitForURL('**/guide/network-scenes');await p.goBack();await p.waitForURL(u=>u.pathname===base+'/zh/guide/dns');await p.locator('.docs-article').evaluate(e=>e.scrollTop=0);
  await p.locator('[data-doc-search]').fill('没有这个功能xyz');assert(await p.locator('[data-doc-empty]').isVisible());await p.locator('[data-doc-search]').fill('');
  await p.locator('[data-lang-pop]').locator('..').locator('[data-pref-button]').click();await p.locator('[data-lang-pop] input[value="en"]').check();await p.waitForURL(origin+base+'/guide/dns');assert(!p.url().includes('/zh/'));
  await p.reload();await p.locator('.docs-article').waitFor();assert((await p.locator('h1').innerText()).includes('DNS'));
  await p.evaluate(()=>localStorage.setItem('crossbow.site.theme','dark'));await p.reload();await p.screenshot({path:out+'/dns-dark-en.png',fullPage:true});
  await p.goto(origin+base+'/zh/guide#troubleshooting');await p.waitForURL('**/zh/guide/troubleshooting');
  await p.goto(origin+base+'/zh/guide/local-import');await p.setViewportSize({width:768,height:900});await p.screenshot({path:out+'/tablet.png'});
  await p.setViewportSize({width:390,height:844});await p.goto(origin+base+'/zh/guide/custom-groups');assert.equal(await p.locator('.docs-sidebar').getAttribute('open'),null);await p.locator('.docs-sidebar>summary').click();await p.screenshot({path:out+'/mobile-menu.png'});
  await p.locator('[data-doc-search]').fill('DNS');await p.locator('.docs-navigation a',{hasText:'DNS 设置'}).click();await p.waitForURL('**/zh/guide/dns');assert.equal(await p.locator('.docs-sidebar').getAttribute('open'),null);await p.screenshot({path:out+'/mobile.png',fullPage:true});
  assert(await p.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'mobile has no horizontal overflow');await p.setViewportSize({width:360,height:740});assert(await p.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'360px has no horizontal overflow');
  await p.setViewportSize({width:1440,height:1000});await p.goto(origin+base+'/zh/guide/override');await p.screenshot({path:out+'/override.png',fullPage:true});
  assert(await p.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  const nojs=await browser.newContext({javaScriptEnabled:false,viewport:{width:1280,height:900}});const plain=await nojs.newPage();await plain.goto(origin+base+'/zh/guide/dns');assert(await plain.locator('.docs-navigation').isVisible());assert(await plain.getByRole('link',{name:'DNS 解析路径诊断',exact:true}).first().isVisible());await nojs.close();
  writeFileSync(out+'/checks.json',JSON.stringify({staticPages:pages,broken,errors,base,checks:['all-routes-refreshable','local-links','canonical-hreflang','one-title','sticky-directory','directory-filter-empty','same-doc-language-switch','legacy-hash-migration','theme','mobile-menu-navigation','tablet','section-and-pagination-navigation','360px','no-horizontal-overflow','no-javascript-content']},null,2));console.log(out);
 }
 assert.deepEqual(errors,[]);
}finally{await browser.close();await new Promise(r=>server.close(r));}
