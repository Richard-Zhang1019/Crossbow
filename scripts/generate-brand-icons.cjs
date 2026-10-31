/** Regenerate the approved B1 identity. Requires sharp and the desktop Tauri CLI.
 * CROSSBOW_SHARP_MODULE may point to an existing sharp installation. */
const fs=require('node:fs'),path=require('node:path'),os=require('node:os'),cp=require('node:child_process');
const sharp=require(process.env.CROSSBOW_SHARP_MODULE||'sharp');
const {profileIcns,profileIco}=require('./brand-icon-containers.cjs');
// Persist colour intent in the bytes, including PNGs re-encoded by Tauri.
const profilePng=bytes=>sharp(bytes).withIccProfile('srgb').png().toBuffer();
const root=path.resolve(__dirname,'..'),brand=path.join(root,'design/logo/current');
const read=p=>fs.readFileSync(path.join(root,p),'utf8');
const put=(p,data)=>{const out=path.join(root,p);fs.mkdirSync(path.dirname(out),{recursive:true});fs.writeFileSync(out,data);};
const copy=(from,to)=>put(to,fs.readFileSync(path.join(root,from)));
const source=read('design/logo/current/mark.svg');
// One approved brand color for all themes and platform exports.
const brandColor=source.match(/color="(#[a-fA-F0-9]{6})"/)[1].toLowerCase();
const geometry=source.replace(/^.*?<\/title>/s,'').replace(/<\/svg>\s*$/,'');
const mark=color=>source.replace(/color="#[a-fA-F0-9]+"/,`color="${color}"`);
const tile=(flat=false,dark=false)=>`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" color="${brandColor}"><rect ${flat?'x="0" y="0" width="100" height="100"':'x="1" y="1" width="98" height="98" rx="22" stroke="#e4e7ee"'} fill="${dark?'#23262d':'#f7f8fb'}"/><g transform="translate(13 13) scale(.74)">${geometry}</g></svg>`;
async function png(svg,size,to,opaque=false){let im=sharp(Buffer.from(svg)).resize(size,size);if(opaque)im=im.removeAlpha();put(to,await im.withIccProfile('srgb').png().toBuffer());}
(async()=>{
  for(const [name,color] of [['mark',brandColor],['mark-dark',brandColor],['mark-black','#000000'],['mark-white','#ffffff']]){
    put(`design/logo/current/${name}.svg`,mark(color));
    await png(mark(color),512,`design/logo/current/${name}-512.png`);
  }
  put('design/logo/current/app-icon.svg',tile());put('design/logo/current/app-icon-ios.svg',tile(true));put('design/logo/current/app-icon-dark.svg',tile(true,true));
  for(const size of [16,32,64,128,256,512,1024])await png(tile(),size,`design/logo/current/app/icon-${size}.png`);
  for(const [name,color] of [['black','#000000'],['white','#ffffff']])for(const size of [16,18,22,32,36,44,66,512])await png(mark(color),size,`design/logo/current/tray/${name}-${size}.png`);
  for(const size of [16,32,48,192,256])await png(tile(),size,`design/logo/current/favicon/favicon-${size}.png`);
  await png(tile(true),180,'design/logo/current/favicon/apple-touch-icon.png',true);
  const adaptive=mark(brandColor);
  put('design/logo/current/favicon/favicon.svg',adaptive);
  // Native multi-resolution ICNS/ICO are authored by Tauri's official icon generator.
  const temp=fs.mkdtempSync(path.join(os.tmpdir(),'crossbow-b1-icons-'));
  try{
    const cli=path.join(root,'apps/desktop/node_modules/.bin',process.platform==='win32'?'tauri.cmd':'tauri');
    cp.execFileSync(cli,['icon',path.join(brand,'app/icon-1024.png'),'--output',temp],{stdio:'pipe',shell:process.platform==='win32'});
    const icns=await profileIcns(fs.readFileSync(path.join(temp,'icon.icns')),profilePng);
    const ico=await profileIco(fs.readFileSync(path.join(temp,'icon.ico')),profilePng);
    put('apps/desktop/src-tauri/icons/icon.icns',icns);
    put('apps/desktop/src-tauri/icons/icon.ico',ico);
    put('design/logo/current/favicon/favicon.ico',ico);
    // Direct NSApplication icons bypass IconServices' tile inset. Give only the
    // running macOS Dock icon its own safe area; preserve all other brand sizes.
    // 98% source tile × 82% scale = 80.36% visible tile, centred on the canvas.
    const dockSvg=tile().replace(/(<svg[^>]*>)([\s\S]*)(<\/svg>)/,'$1<g transform="translate(9 9) scale(.82)">$2</g>$3');
    put('design/logo/current/dock-icon.svg',dockSvg);
    const dockPng=path.join(temp,'dock-source.png');
    fs.writeFileSync(dockPng,await sharp(Buffer.from(dockSvg)).resize(1024,1024).withIccProfile('srgb').png().toBuffer());
    const dockOutput=path.join(temp,'dock');
    cp.execFileSync(cli,['icon',dockPng,'--output',dockOutput],{stdio:'pipe',shell:process.platform==='win32'});
    put('apps/desktop/src-tauri/icons/dock-icon.icns',await profileIcns(fs.readFileSync(path.join(dockOutput,'icon.icns')),profilePng));
  }finally{fs.rmSync(temp,{recursive:true,force:true});}
  for(const [size,name] of [[32,'32x32.png'],[128,'128x128.png'],[256,'128x128@2x.png'],[1024,'icon-source.png']])copy(`design/logo/current/app/icon-${size}.png`,`apps/desktop/src-tauri/icons/${name}`);
  copy('design/logo/current/tray/black-44.png','apps/desktop/src-tauri/icons/tray-icon.png');
  for(const target of ['apps/desktop/src/assets/logo-glyph.png','apps/site/src/assets/logo-glyph.png'])copy('design/logo/current/mark-512.png',target);
  copy('design/logo/current/app/icon-512.png','apps/site/src/assets/app-icon.png');
  for(const size of [16,32,192])copy(`design/logo/current/favicon/favicon-${size}.png`,`apps/site/public/favicon-${size}.png`);
  for(const name of ['favicon.ico','favicon.svg','apple-touch-icon.png'])copy(`design/logo/current/favicon/${name}`,`apps/site/public/${name}`);
  const catalog='apps/ios/Crossbow/Resources/Assets.xcassets';
  put(`${catalog}/Contents.json`,JSON.stringify({info:{author:'xcode',version:1}},null,2)+'\n');
  await png(tile(true),1024,`${catalog}/AppIcon.appiconset/AppIcon.png`,true);
  put(`${catalog}/AppIcon.appiconset/Contents.json`,JSON.stringify({images:[{filename:'AppIcon.png',idiom:'universal',platform:'ios',size:'1024x1024'}],info:{author:'xcode',version:1}},null,2)+'\n');
  const legacyLogo=path.join(root,'apps/ios/Crossbow/Resources/logo-glyph.png');
  if(fs.existsSync(legacyLogo))fs.unlinkSync(legacyLogo);
  const images=[];
  for(const scale of [1,2,3])for(const dark of [false,true]){
    const filename=`logo-${dark?'dark':'light'}@${scale}x.png`;
    await png(mark(brandColor),32*scale,`${catalog}/logo-glyph.imageset/${filename}`);
    images.push({filename,idiom:'universal',scale:`${scale}x`,...(dark?{appearances:[{appearance:'luminosity',value:'dark'}]}:{})});
  }
  put(`${catalog}/logo-glyph.imageset/Contents.json`,JSON.stringify({images,info:{author:'xcode',version:1}},null,2)+'\n');
  const outputs=[];for(const d of ['design/logo/current','apps/desktop/src-tauri/icons','apps/site/public',catalog]){
    function walk(dir){for(const e of fs.readdirSync(path.join(root,dir),{withFileTypes:true})){const p=`${dir}/${e.name}`;if(e.isDirectory())walk(p);else if(/\.(png|ico|icns|svg|json)$/.test(p)&&!p.endsWith('/manifest.json'))outputs.push(p);}}walk(d);
  }
  outputs.push('apps/desktop/src/assets/logo-glyph.png','apps/site/src/assets/logo-glyph.png','apps/site/src/assets/app-icon.png');
  const crypto=require('node:crypto');
  put('design/logo/current/manifest.json',JSON.stringify({design:'B1 / original v01 arc',issue:202,source:'design/logo/current/mark.svg',colors:{light:brandColor,dark:brandColor,tile:'#f7f8fb'},colorSpace:'sRGB',embeddedIcc:true,files:outputs.map(p=>({path:p,sha256:crypto.createHash('sha256').update(fs.readFileSync(path.join(root,p))).digest('hex')}))},null,2)+'\n');
  cp.execFileSync(process.execPath,[path.join(__dirname,'verify-brand-icons.cjs')],{stdio:'inherit'});
  console.log('Generated B1 brand marks, desktop ICNS/ICO, template tray, web favicons and iOS asset catalogs.');
})().catch(e=>{console.error(e.message);process.exit(1);});
