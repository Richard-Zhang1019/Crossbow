// #639: enforce the source contract, not a screenshot of today's CSS.
import assert from 'node:assert/strict';
import {readFileSync,readdirSync,realpathSync} from 'node:fs';
import path from 'node:path';
import {createRequire} from 'node:module';
import {fileURLToPath} from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../apps/desktop/src');
const req=createRequire(realpathSync(path.join(root,'../node_modules/vite/package.json')));
const postcss=req('postcss');
const files=[];function walk(dir){for(const d of readdirSync(dir,{withFileTypes:true})){const f=path.join(dir,d.name);if(d.isDirectory())walk(f);else if(/\.(css|tsx|ts)$/.test(f)&&!f.endsWith('.test.mjs'))files.push(f)}}walk(root);
const compat=JSON.parse(readFileSync(new URL('./design-token-compat.json',import.meta.url),'utf8')).maximumReferences;
const compatCounts={};
const errors=[],definitions=new Set(["--cb-modal-width","--cb-flow-first-end","--cb-flow-second-start"]),references=[]; // Runtime API/ResizeObserver geometry, documented in DESIGN-SYSTEM.md.
const scalar=new Set(['font-size','font-family','font-weight','line-height','letter-spacing','border-radius','box-shadow','opacity']);
const keywords=/^(?:0|1|none|inherit|initial|unset|normal)$/;
export function violations(prop,value){
 const issues=[];
 if(scalar.has(prop)&&!keywords.test(value)&&!value.includes('var('))issues.push('visual scalar must use a token');
 if(/^(?:padding|margin|gap|row-gap|column-gap)(?:-|$)/.test(prop)&&/(?<![\w.-])-?\d*\.?\d+px\b/.test(value))issues.push('spacing must use a token');
 if(/^(?:color|background(?:-color)?|border(?:-color)?|outline(?:-color)?|fill|stroke)$/.test(prop)&&/(#[\da-f]{3,8}\b|rgba?\()/i.test(value))issues.push('paint must use a token');
 if(/^(?:border(?:-(?:top|bottom|left|right))?(?:-width)?|outline(?:-width|-offset)?)$/.test(prop)&&/(?<![\w.-])\d*\.?\d+px\b/.test(value))issues.push('stroke must use a token');
 if(/\d(?:ms)\b/.test(value))issues.push('duration must use a token');
 return issues;
}
// Negative probes ensure the guard actually rejects new hardcoding, rather than only passing this tree.
assert(violations('font-size','13px').length);
assert(violations('border','1px solid var(--cb-line)').length);
assert(violations('color','#fff').length);
assert(violations('padding','0 12px').length);
assert(violations('transition','color 140ms ease-out').length);
assert.equal(violations('font-size','var(--cb-type-body)').length,0);
assert.equal(violations('width','calc(100% - 32px)').length,0,'layout geometry is a separate contract');
for(const file of files){const source=readFileSync(file,'utf8');
 for(const m of source.matchAll(/var\((--cb-type-compat-[\w-]+)/g))compatCounts[m[1]]=(compatCounts[m[1]]||0)+1;
 if(file.endsWith('.css'))postcss.parse(source,{from:file}).walkDecls(d=>{
  if(d.prop.startsWith('--'))definitions.add(d.prop);
  if(!file.endsWith('/tokens.css')&&!d.prop.startsWith('--'))for(const reason of violations(d.prop,d.value))errors.push(`${path.relative(root,file)}:${d.source.start.line}: ${d.prop}: ${reason}`);
 });
 for(const m of source.matchAll(/var\((--(?:cb-|font-|z-)[\w-]+)/g))references.push({name:m[1],file});
 if(file.endsWith('.tsx')){
  if(/\b(?:fontSize|fontWeight|lineHeight|padding|paddingTop|paddingBottom|paddingLeft|paddingRight|margin|marginTop|marginBottom|marginLeft|marginRight|gap|rowGap|columnGap)\s*:\s*\d/.test(source))errors.push(`${path.relative(root,file)}: literal inline typography/spacing`);
  if(/text-\[[\d.]+px\]|(?<![\w-])text-(?:xs|sm|base|lg|xl|[2-9]xl)\b/.test(source))errors.push(`${path.relative(root,file)}: literal Tailwind type scale`);
  if(/["']#[\da-f]{3,8}["']/i.test(source))errors.push(`${path.relative(root,file)}: inline palette`);
 }
}
for(const r of references)if(!definitions.has(r.name))errors.push(`${path.relative(root,r.file)}: undefined ${r.name}`);
for(const [name,count]of Object.entries(compatCounts))if(count>(compat[name]??0))errors.push(`${name}: new legacy consumers are not allowed`);
assert.deepEqual([...new Set(errors)],[],'Design token contract violated');
console.log(`Design tokens passed: ${files.length} sources, ${definitions.size} definitions; hardcoding and undefined references checked.`);
