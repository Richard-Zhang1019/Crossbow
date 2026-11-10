import { Icon } from "@/components/Icon";
import { nodeNameParts } from "./nodeFlags";
import "./NodeName.css";

// Eager URLs ensure Vite includes every asset in the offline application bundle.
const flags = import.meta.glob<string>("../assets/flags/*.svg", {
  eager: true, query: "?url", import: "default",
});

export default function NodeName({ name }: { name: string }) {
  const automatic = /^\s*♻\uFE0F?/.test(name);
  const display = automatic ? name.replace(/^\s*♻\uFE0F?\s*/, "") : name;
  return <>{automatic && <span className="cb-node-auto-mark"><Icon name="automatic" size={15}/></span>}{nodeNameParts(display).map((part, index) => {
    if ("text" in part) return <span key={index}>{part.text}</span>;
    const source = flags[`../assets/flags/${part.region.toLowerCase()}.svg`];
    return source
      ? <img key={index} className="cb-node-flag" src={source} alt={part.region} draggable={false} />
      : <span key={index} className="cb-node-region">{part.region}</span>;
  })}</>;
}
