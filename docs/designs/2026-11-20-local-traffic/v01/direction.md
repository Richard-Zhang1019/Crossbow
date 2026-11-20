# APP SHEET — Framed sheet
seed 1912770209 · drawn 2026-11-20 · kind analytics · light
Brief: Crossbow local network usage, daily bytes and destination rankings

This is the style for this product. Adopt it whole and keep its move intact; everything the sheet does not fix comes from the
shared defaults in references/app-screens.md. Redraw only for a reason you can state (it contradicts the brand, or the product is
used in a way the style does not suit) and say why in your report. The values were measured on real product screens: use them
as given rather than rounding them towards your habits.

## 1. The style
The app chrome is one tinted L-shaped frame; the work sits on a white sheet with a hairline edge and soft corners, so depth comes from a single tone step and no shadow.

**The move:** One tinted frame holds the sidebar and the 48px top bar with no rule between them; the page is a white sheet inset 8px from the window's right and bottom edges, with a 1px edge, 16px corners and no shadow. The active tab is a 2px underline in ink or the accent; everything else in the chrome is grey.

**Colour budget:** under 3%; the brand shrinks to the logo and a 2px tab underline, the data carries the chroma.
**Off-style here:** a border between the sidebar and the top bar; a shadow under the sheet; cards whose border is heavier than the sheet's (#cdcfcf on cards read as heavy in the study); a fourth nesting level: frame, sheet and card are enough; different gutters for the KPI row and the chart row (20 against 24 was flagged).
Distilled from 6 screens: a developer platform overview, an analytics overview, an events analytics tab, a list detail, a reports dashboard, a flow builder. The study's second data hue in this style was an orange (#e88e46) that falls inside the house terracotta range; the third chart colour here is an amber moved out of that range and darkened to 3:1 on white.

**Its face.** Choose one of these (or one of your own of the same weight) before you build, write it in your notes next to
the screen's question, and check that it shows on the first screen. Without one the screen reads as a well-applied kit.
- The frame tinted from the product's colour in place of #f9fbfc: one low-chroma step of its hue (a green-grey for a climate or farm tool, a blue-grey for an observability tool, a stone for a print shop). The sheet stays white.
- The brand's own hue from the brief as --chart-1 in place of the dealt teal, on the main series and the one selected state, at 3:1 or more on white, never violet or indigo.
- IBM Plex Mono for IDs, keys and the figures in tables (one job): its figures have their own drawing, and a developer reads them as the product's.

## 2. Tokens
Kept from the brand: map its colours onto this style's roles (ground, surface, raised, border, rule, control, text, muted, accent, status pills, chart) and keep its proportions: under 3%; the brand shrinks to the logo and a 2px tab underline, the data carries the chroma. Take the sizes, radii, row and control heights from the style as they are.

## 4. Shell — Tinted frame, white sheet
Sidebar and top bar share one tinted ground with no rule between them, forming an L; the page lives on a white sheet. Variant one: the sheet butts against the sidebar and the top bar and is inset 8px from the right and bottom edges, 16px radius, 1px edge. Variant two: a flush L-frame with no inset, the frame marked by one rule. Variant three: sidebar and content are two floating panels inset 12px from every edge with a 12px gutter. The top bar, when present, is 48px and holds a centred search, the account button and a help or AI icon. Inside the sheet the content has 24px padding and starts with the breadcrumb or title row.
Numbers: chrome #f6f6f6-#f9fbfc (a warm #f7f6f2 once); sheetInset 8px from the window's right and bottom edges (12 and 16 once each); sheetRadius 10-16, median 16; sheetEdge 1px #eaeaea-#ededed, no shadow; sidebarWidth 220-263 (228 on two screens); topBar 48px on the chrome ground with no rule; search 548x32 centred on the window with a shortcut hint; contentPadding 24; floatingVariant both panels inset 12px with a 12px gutter, radius 10.
On a phone: The frame disappears: the sheet becomes the screen, with a 48px top bar (menu button, title, one action) on the chrome tint. The search moves into the menu sheet or behind an icon.

## 5. Screens

### analytics — Per-period and running-total pairs
Title at 24-32px with one sentence under it saying what the filters do, then one filter row of 36-38px controls (one filled or near-black, the rest borderless or text, a reset link last) and 16-24px to the KPIs. KPI row: 3 equal cards in the content column (3 x 372 + 2 x 17 = 1152 in the study) or one divided container; a 16px label over the value with the change written as a phrase on the value's baseline ('+8 in the last 90 days'), grey when it is zero. Then one row per metric: the per-period column chart on the left and its cumulative line on the right, at equal width (2 x 568 + 16 = 1152) and with the same x ticks; square bars, a 2px straight line, integer ticks from zero, no legend because each chart has one series. One gap value horizontally and vertically.
On a phone: Pairs stack, the cumulative chart directly under its per-period chart; KPI cards become a single column of label-value rows.

## 6. Always
Real data for this product, written before the markup: names, amounts that add up, dates in order, units on every figure,
honest zeros and dashes. One chart answers one question. No gradients, glows, glass, icons in tinted squares, sparklines on KPIs,
rainbow charts, emoji, lorem or Acme. `<skill>/assets/app.css` and `<skill>/assets/charts.js` are a neutral start set to these
rules. Check with `node <skill>/scripts/lint.mjs <files>` and `node <skill>/scripts/inspect.mjs <page>`: this sheet switches both
to app rules.

Saved to /tmp/crossbow-stats-style/direction.json · reprint with `node draw.mjs --show`
