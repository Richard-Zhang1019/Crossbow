import { Icon } from "@/components/Icon";
import { nodeNameParts } from "./nodeFlags";
import "./NodeName.css";

// Eager URLs ensure Vite includes every asset in the offline application bundle.
// glob 路径相对本文件（components/NodeName/）且必须是内联字面量（vite 编译期
// 展开的要求）——上跳两级才是 src/assets/flags；移动本文件必须同步此串，
// 否则 glob 静默匹配空集、旗标全部退化为两字母文本（v0.14.0 事故，
// nodeFlags.test.mjs 有守卫测试防复发）。
const flags = import.meta.glob<string>("../../assets/flags/*.svg", {
  eager: true, query: "?url", import: "default",
});

export default function NodeName({ name }: { name: string }) {
  const automatic = /^\s*♻\uFE0F?/.test(name);
  const display = automatic ? name.replace(/^\s*♻\uFE0F?\s*/, "") : name;
  return <>{automatic && <span className="cb-node-auto-mark"><Icon name="automatic" size={15}/></span>}{nodeNameParts(display).map((part, index) => {
    if ("text" in part) return <span key={index}>{part.text}</span>;
    const source = flags[`../../assets/flags/${part.region.toLowerCase()}.svg`];
    return source
      ? <img key={index} className="cb-node-flag" src={source} alt={part.region} draggable={false} />
      : <span key={index} className="cb-node-region">{part.region}</span>;
  })}</>;
}
