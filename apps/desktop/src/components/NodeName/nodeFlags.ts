/** Presentation only. Never feed display text back to the controller. */
export type NodeNamePart = { text: string } | { region: string };

export function nodeNameParts(name: string): NodeNamePart[] {
  const parts: NodeNamePart[] = [];
  const flags = /[\u{1F1E6}-\u{1F1FF}]{2}\uFE0F?/gu;
  let end = 0;
  for (const match of name.matchAll(flags)) {
    if (match.index! > end) parts.push({ text: name.slice(end, match.index) });
    const region = Array.from(match[0]).slice(0, 2)
      .map((letter) => String.fromCharCode(letter.codePointAt(0)! - 0x1f1e6 + 65)).join("");
    parts.push({ region });
    end = match.index! + match[0].length;
    // Remove a flag's decorative separator, never a literal JP/HK name prefix.
    end += /^[\s|｜]+/u.exec(name.slice(end))?.[0].length ?? 0;
  }
  if (end < name.length) parts.push({ text: name.slice(end) });
  return parts;
}

/** Plain-text surfaces (tooltips/accessibility) must not rely on emoji fonts. */
export function nodeNameText(name: string): string {
  return nodeNameParts(name).map((part) => "text" in part ? part.text : `[${part.region}] `).join("").trimEnd();
}
