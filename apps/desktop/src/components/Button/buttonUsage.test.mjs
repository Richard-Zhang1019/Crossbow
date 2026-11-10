// Architectural guard: newly added desktop controls must use the shared native-button boundary.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
function sourceFiles(dir) {
  return fs
    .readdirSync(dir, { withFileTypes: true })
    .flatMap((entry) =>
      entry.isDirectory()
        ? sourceFiles(path.join(dir, entry.name))
        : entry.name.endsWith(".tsx")
          ? [path.join(dir, entry.name)]
          : [],
    );
}
test("desktop native buttons are created only by ButtonBase", () => {
  const violations = [];
  for (const file of sourceFiles(root)) {
    if (file === path.join(root, "components", "Button", "index.tsx")) continue;
    const source = ts.createSourceFile(
      file,
      fs.readFileSync(file, "utf8"),
      ts.ScriptTarget.Latest,
      true,
      ts.ScriptKind.TSX,
    );
    function visit(node) {
      if (
        (ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) &&
        node.tagName.getText(source) === "button"
      ) {
        violations.push(
          path.relative(root, file) +
            ":" +
            (source.getLineAndCharacterOfPosition(node.getStart(source)).line +
              1),
        );
      }
      ts.forEachChild(node, visit);
    }
    visit(source);
  }
  assert.deepEqual(
    violations,
    [],
    "Use Button for actions or ButtonBase for structural controls",
  );
});
