import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync,existsSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
import links from '../src/docs/markdown-links.mjs';
const registry=readFileSync(new URL('../src/docs/navigation.ts',import.meta.url),'utf8');
const docs=JSON.parse(registry.match(/export const documents = ([\s\S]+?) as const;/)[1]);
test('every navigation entry has an independent route in both languages',()=>{
 const slugs=new Set();
 for(const d of docs){assert(!slugs.has(d.slug));slugs.add(d.slug);for(const prefix of ['', 'zh/']){
  const path=`../src/pages/${prefix}guide/${d.slug}`;
  const files=['astro','md'].map(ext=>new URL(`${path}.${ext}`,import.meta.url)).filter(existsSync);
  assert.equal(files.length,1,`${prefix}${d.slug} must have exactly one route`);
  assert(d.title[prefix?'zh':'en']&&d.description[prefix?'zh':'en']);
 }}
});
test('documentation links use language and deployment base without touching external links',()=>{
 const make=()=>({children:[{type:'link',url:'/guide/dns#resolver'},{type:'link',url:'https://example.test/guide/dns'},{type:'link',url:'#section'},{type:'link',url:'/guidebook'}]});
 const zh=make();links({base:'/Crossbow/'})(zh,{path:'/repo/src/pages/zh/guide/a.md'});
 assert.deepEqual(zh.children.map(n=>n.url),['/Crossbow/zh/guide/dns#resolver','https://example.test/guide/dns','#section','/guidebook']);
 const en=make();links()(en,{path:'/repo/src/pages/guide/a.md'});assert.equal(en.children[0].url,'/guide/dns#resolver');
});
test('all former section links have routes after migration',()=>{
 const old=['quick-start','deeplink','proxy-vs-tun','engines','override','ports','lightweight','subscriptions','recovery','data-locations','tray-speed','notifications','linux','windows','troubleshooting','updates'];
 for(const id of old)assert(docs.some(d=>d.slug===id),id);
});
test('published script example preserves rules and removes deleted-node references',()=>{
 const src=readFileSync(new URL('../src/components/docs-content/override.astro',import.meta.url),'utf8').match(/const CODE_FILTER = `([\s\S]+?)`;/)[1];
 const main=runInNewContext(`${src};main`);
 const input={proxies:[{name:'JP-1'},{name:'US-1'}],'proxy-groups':[{name:'select',proxies:['JP-1','US-1','DIRECT']}],rules:['MATCH,select']};
 const result=main(input);assert.deepEqual(result.proxies,[{name:'JP-1'}]);assert.deepEqual(result['proxy-groups'][0].proxies,['JP-1','DIRECT']);assert.deepEqual(result.rules,['MATCH,select']);
 assert.throws(()=>main({proxies:[{name:'US-1'}]}),/No matching nodes/);
});
