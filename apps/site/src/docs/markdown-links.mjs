// Markdown feature routes use language-neutral /guide links. Resolve locale and
// deployment base at build time so exported HTML works without client JavaScript.
export default function documentationLinks({base = '/'} = {}) {
 const prefix=base.replace(/\/+$/,'');
 return (tree,file)=>{
  const chinese=String(file.path||file.history?.[0]||'').replaceAll('\\','/').includes('/pages/zh/');
  const visit=node=>{
   if(node.type==='link' && /^\/guide(?:\/|#|$)/.test(node.url)) node.url=`${prefix}${chinese?'/zh':''}${node.url}`;
   if(node.children)node.children.forEach(visit);
  };visit(tree);
 };
}
