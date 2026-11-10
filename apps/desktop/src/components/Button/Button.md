# Desktop buttons

Use `Button` for actions. There are exactly two visual variants:

```tsx
<Button variant="primary" onClick={save}>保存</Button>
<Button onClick={cancel}>取消</Button>
<Button context="toolbar" onClick={diagnose}><Icon name="pulse" />诊断</Button>
<Button context="toolbar" iconOnly aria-label="最近通知"><Icon name="bell" /></Button>
<Button tone="danger" onClick={remove}>删除</Button>
<Button loading={saving} disabled={!valid} onClick={save}>保存</Button>
```

- `variant`: `primary` (solid) or `secondary` (outlined, default).
- `context`: `content` (32px, default) or `toolbar` (34px).
- `size="small"`: 28px for dense content actions.
- `iconOnly`: square shape; supply `aria-label`, optionally a tooltip `title`.
- `tone="danger"`: semantic danger colors, independent of the two variants.
- `loading`: disables the native button, sets `aria-busy`, overlays a shared spinner. The label stays in layout and the accessibility tree, so width and accessible name stay stable.
- Native props, events and refs are forwarded. Default type is `button`; use `type="submit"` explicitly in forms.

Pages own placement and business state. Do not override button paint, padding, height or hover with inline styles or utility classes; use the props above. Button.css owns theme tokens, focus, hover, pressed, disabled, reduced-motion and forced-color treatments. Async state and errors remain owned by the calling operation/Notification service, not by Button.

`ButtonBase` is the semantic foundation for controls that already have their own design: navigation, tabs, switches, pagination, data cards, compact notification links and primitive composition (Select.Trigger). It forwards native props/ref/type and shares disabled/busy behavior, without action-button surfaces or wrappers. Do not use it to create a separate action-button style.

All hand-authored desktop `<button>` elements must be inside ButtonBase. Guard:

```sh
node --test apps/desktop/src/components/buttonUsage.test.mjs
```
