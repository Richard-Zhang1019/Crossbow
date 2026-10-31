/** Validate the shipped B1 binary assets and catalog references, not just source filenames. */
const fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict'),crypto=require('node:crypto');
const sharp=require(process.env.CROSSBOW_SHARP_MODULE||'sharp');
const {isPng}=require('./brand-icon-containers.cjs');
const root=path.resolve(__dirname,'..'),file=p=>path.join(root,p),read=p=>fs.readFileSync(file(p));
(async()=>{
  const approved=read('docs/designs/2026-10-28-logo/v01/assets/b1.svg');
  const normalizeColor=svg=>svg.toString().replace(/color="#[a-fA-F0-9]+"/,'color="APPROVED"');
  assert.equal(normalizeColor(read('design/logo/current/mark.svg')),normalizeColor(approved),'B1 geometry must remain the approved original, not a V2 variation');
  const brandColor='#3f52cf';
  assert(read('design/logo/current/mark.svg').toString().includes('color="'+brandColor+'"'),'Master must use approved B color');
  const rgb=[63,82,207];
  for(const p of ['design/logo/current/mark-512.png','design/logo/current/mark-dark-512.png','apps/desktop/src/assets/logo-glyph.png','apps/site/src/assets/logo-glyph.png']){
    const {data}=await sharp(read(p)).ensureAlpha().raw().toBuffer({resolveWithObject:true});
    let opaque=0;for(let i=0;i<data.length;i+=4)if(data[i+3]===255){opaque++;assert.deepEqual([...data.subarray(i,i+3)],rgb,p+' uses a stale brand color');}
    assert(opaque>0,p+' must contain opaque brand ink');
  }
  for(const scale of [1,2,3])assert.deepEqual(read('apps/ios/Crossbow/Resources/Assets.xcassets/logo-glyph.imageset/logo-light@'+scale+'x.png'),read('apps/ios/Crossbow/Resources/Assets.xcassets/logo-glyph.imageset/logo-dark@'+scale+'x.png'),'iOS theme logos must use the same ink');
  assert.equal(read('apps/site/public/favicon.svg').toString(),read('design/logo/current/mark.svg').toString(),'Favicon must use the same brand color in both themes');
  const css=read('apps/desktop/src/index.css').toString();
  assert(css.includes('--cb-brand: '+brandColor),'Desktop brand token must match master');
  assert(/\.cb-logo-glyph\s*\{[^}]*background:\s*var\(--cb-brand\)/s.test(css),'Desktop logo must not inherit the theme accent');
  const manifest=JSON.parse(read('design/logo/current/manifest.json'));
  assert.equal(manifest.colors.light,brandColor);assert.equal(manifest.colors.dark,brandColor);
  assert.equal(manifest.colorSpace,'sRGB');assert.equal(manifest.embeddedIcc,true);
  const canonicalIcc=(await sharp(read('apps/desktop/src-tauri/icons/icon-source.png')).metadata()).icc;
  assert(canonicalIcc && canonicalIcc.length>128,'Master PNG must contain an ICC profile');
  async function checkPng(bytes,label){
    const m=await sharp(bytes).metadata();
    assert(m.icc && m.icc.equals(canonicalIcc),label+' must embed the canonical sRGB profile');
    return m;
  }
  for(const f of manifest.files)if(f.path.endsWith('.png'))await checkPng(read(f.path),f.path);
  for(const f of manifest.files)assert.equal(crypto.createHash('sha256').update(read(f.path)).digest('hex'),f.sha256,f.path+' drifted; regenerate');
  for(const [p,n] of [['apps/desktop/src-tauri/icons/32x32.png',32],['apps/desktop/src-tauri/icons/128x128.png',128],['apps/desktop/src-tauri/icons/128x128@2x.png',256],['apps/desktop/src-tauri/icons/icon-source.png',1024],['apps/site/public/apple-touch-icon.png',180]]){
    const m=await sharp(read(p)).metadata();assert.equal(m.width,n,p);assert.equal(m.height,n,p);
  }
  for(const [name,value] of [['black',0],['white',255]]){
    const p=`design/logo/current/tray/${name}-44.png`;const {data,info}=await sharp(read(p)).ensureAlpha().raw().toBuffer({resolveWithObject:true});let count=0;
    for(let i=0;i<data.length;i+=4)if(data[i+3]){count++;assert.equal(data[i],value,p);assert.equal(data[i+1],value,p);assert.equal(data[i+2],value,p);}
    assert(count>0&&count<info.width*info.height,'Tray must contain a transparent silhouette');
  }
  const ico=read('apps/desktop/src-tauri/icons/icon.ico');assert.equal(ico.readUInt16LE(2),1);const n=ico.readUInt16LE(4);assert(n>=3);let sizes=[];
  for(let i=0;i<n;i++){const o=6+16*i;sizes.push(ico[o]||256);assert(ico.readUInt32LE(o+12)+ico.readUInt32LE(o+8)<=ico.length);}
  for(const s of [16,32,256])assert(sizes.includes(s),'Missing ICO size '+s);
  for(const name of ['icon.icns','dock-icon.icns']){
    const icns=read('apps/desktop/src-tauri/icons/'+name);assert.equal(icns.toString('ascii',0,4),'icns');assert.equal(icns.readUInt32BE(4),icns.length);
    let frames=0;
    for(let o=8;o<icns.length;){
      const length=icns.readUInt32BE(o+4);assert(length>=8 && o+length<=icns.length,'Invalid ICNS chunk');
      const data=icns.subarray(o+8,o+length);
      if(isPng(data)){
        const m=await checkPng(data,name+' '+icns.toString('ascii',o,o+4));frames++;
        if(name==='dock-icon.icns'){
          // Check the shipped representations, not just the SVG transform. Reject
          // full-bleed or off-centre tiles and colour drift after native encoding.
          const {data:pixels,info}=await sharp(data).ensureAlpha().raw().toBuffer({resolveWithObject:true});
          let left=info.width,top=info.height,right=-1,bottom=-1,ink=0;
          for(let y=0;y<info.height;y++)for(let x=0;x<info.width;x++){
            const p=(y*info.width+x)*4;
            if(pixels[p+3]>=128){left=Math.min(left,x);right=Math.max(right,x);top=Math.min(top,y);bottom=Math.max(bottom,y);}
            if(pixels[p]===63 && pixels[p+1]===82 && pixels[p+2]===207 && pixels[p+3]===255)ink++;
          }
          const tolerance=2/info.width;
          assert(Math.abs((right-left+1)/info.width-.812)<tolerance,name+' tile width must retain Dock safe area');
          assert(Math.abs((bottom-top+1)/info.height-.812)<tolerance,name+' tile height must retain Dock safe area');
          assert(Math.abs(left-(info.width-1-right))<=1 && Math.abs(top-(info.height-1-bottom))<=1,name+' must be centred');
          if(m.width>=64)assert(ink>0,name+' must retain approved opaque sRGB ink');
        }
      }
      o+=length;
    }
    assert(frames>=8,'Missing profiled ICNS resolutions');
  }
  for(const p of ['apps/desktop/src-tauri/icons/icon.ico','design/logo/current/favicon/favicon.ico','apps/site/public/favicon.ico']){
    const b=read(p),count=b.readUInt16LE(4);
    for(let i=0;i<count;i++){
      const o=6+16*i,length=b.readUInt32LE(o+8),start=b.readUInt32LE(o+12);
      assert(start>=6+16*count && start+length<=b.length,'Invalid ICO entry');
      const data=b.subarray(start,start+length);assert(isPng(data),'Expected PNG-backed ICO');
      const m=await checkPng(data,p+' entry '+i);
      assert.equal(m.width,b[o]||256);assert.equal(m.height,b[o+1]||256);
    }
  }
  const catalog='apps/ios/Crossbow/Resources/Assets.xcassets';
  for(const set of ['AppIcon.appiconset','logo-glyph.imageset']){
    const c=JSON.parse(read(`${catalog}/${set}/Contents.json`));for(const img of c.images){assert(fs.existsSync(file(`${catalog}/${set}/${img.filename}`)));}
  }
  const app=await sharp(read(`${catalog}/AppIcon.appiconset/AppIcon.png`)).metadata();assert.equal(app.width,1024);assert.equal(app.hasAlpha,false,'iOS AppIcon must have no alpha channel');
  assert(!fs.existsSync(file('apps/ios/Crossbow/Resources/logo-glyph.png')),'Legacy loose iOS image would shadow the themed catalog');
  console.log(`Verified ${manifest.files.length} B1 assets, transparent templates, ICO sizes, ICNS and opaque iOS catalog.`);
})().catch(e=>{console.error(e.message);process.exit(1);});
