import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readdirSync, readFileSync } from 'node:fs';
import { nodeNameParts, nodeNameText } from './nodeFlags.ts';

test('only regional indicator pairs become flags; literal country codes stay intact', () => {
  assert.deepEqual(nodeNameParts('🇯🇵|软银-JP-Hy2'), [{region:'JP'},{text:'软银-JP-Hy2'}]);
  assert.deepEqual(nodeNameParts('🇭🇰｜ HKT-Hy2'), [{region:'HK'},{text:'HKT-Hy2'}]);
  for (const name of ['JP|软银-JP-Hy2','HK-Hy2','DIRECT','自动选择','🇯single','']) {
    assert.equal(nodeNameText(name), name);
  }
});
test('flags in chains, multiple flags, variations, flag-only and unknown regions', () => {
  assert.equal(nodeNameText('节点选择[🇯🇵|东京] → 🇺🇸 US-01'), '节点选择[[JP] 东京] → [US] US-01');
  assert.equal(nodeNameText('🇯🇵\uFE0F | 东京'), '[JP] 东京');
  assert.equal(nodeNameText('🇿🇿|node'), '[ZZ] node');
  assert.equal(nodeNameText('🇭🇰'), '[HK]');
});
test('presentation does not mutate identities even when display text collides', () => {
  const nodes=['🇯🇵|Tokyo','🇯🇵｜Tokyo'];
  assert.equal(nodeNameText(nodes[0]),nodeNameText(nodes[1]));
  const selected=new Set(nodes); assert.equal(selected.size,2);
  assert.deepEqual(nodes,['🇯🇵|Tokyo','🇯🇵｜Tokyo']);
});
test('offline assets include common regions, have no remote references and retain license', () => {
  const root=new URL("../../assets/flags/", import.meta.url);
  const files=readdirSync(root).filter(f=>f.endsWith('.svg'));
  for(const code of ['jp','hk','us','cn','tw','eu','sg','gb']) assert(files.includes(`${code}.svg`));
  for(const file of files){const svg=readFileSync(new URL(file,root),'utf8');assert(!/<script|(?:href|url\()\s*=?\s*["']?https?:/i.test(svg),file);}
  assert(readFileSync(new URL('LICENSE',root),'utf8').includes('MIT'));
});
