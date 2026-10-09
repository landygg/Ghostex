---
name: ghostex-visuals
description: >-
  Show charts, stat tiles, tables and text layouts inline in the Ghostex chat
  with a fenced ```visual block, and publish self-contained HTML pages (UI
  mockups, small interactive tools) with `ghostex show` so the chat shows them
  as a card that opens the page. Use only when the user asks for
  $ghostex-visuals.
disable-model-invocation: true
---

# ghostex-visuals

Ghostex draws a fenced ```visual block in your reply natively, inline in the
chat, on desktop, web and phone.

- Prefer a block for charts, tables, stats and text layouts.
- Use an HTML page only for what a block cannot express: UI mockups and small
  interactive tools.
- Don't restate in prose what the visual already shows. Add only what it
  doesn't: the conclusion, a caveat, the next step.

## Visual blocks

One JSON object in a fence whose info string is `visual`. The block is one of:

- **A Vega-Lite spec** at the top level: an object with `"mark"`. It may have
  `"title"` and `"height"`.
- **A composition**: `{"title"?: "…", "rows": [part, …]}`, drawn top to
  bottom. A part is one of the following, and every part may have `"title"`:
  - `{"chart": <Vega-Lite spec>}`
  - `{"stats": [{"label": "…", "value": "…", "note"?: "…", "tone"?: "good" | "bad" | "neutral"}]}`
    (`value` may be a number)
  - `{"table": {"columns": ["…", …], "rows": [["…", …], …]}}`, or rows as
    objects keyed by column name (then `"columns"` is optional)
  - `{"text": "…"}`
  - `{"columns": [part, …]}`: parts side by side
- **A page**: `{"page": {"title": "…", "url": "…"}}`. Only ever paste this from
  `ghostex show` output (below); never write one yourself.

### The Vega-Lite subset

- `mark`: `bar`, `line`, `area`, `point` (also `circle`, `square`), `arc` (pie;
  `{"type": "arc", "innerRadius": 50}` for a donut). The object form
  `{"type": "line", "point": true}` adds points to a line.
- Encoding channels: `x`, `y`, `color`, `theta`, `tooltip`, `xOffset` (grouped
  bars).
- Field definition keys: `field`, `type` (`quantitative`, `nominal`,
  `ordinal`, `temporal`), `aggregate` (`sum`, `mean`, `count`, `min`, `max`,
  `median`), `title`, `sort` (`"ascending"`, `"descending"`, `null` for data
  order, or an array of values), `axis` (`null` to hide it, or
  `{"title": …}`), `scale` (`{"domain": [min, max], "zero": false}`), `stack`
  (`null`/`false`, `true`, `"normalize"`).
- Data only as `"data": {"values": [ … ]}` with the rows inlined; `data.url`
  isn't supported. Aggregate large data before writing the block.
- Not supported: `layer`, `facet`, `repeat`, `concat`, `transform`,
  `params`/selections, `bin`, and the `rule`, `text`, `tick` and `rect` marks.
  Compute bins, totals or moving averages into the rows yourself.
- The width always fits the chat column; `"height"` sets the chart height
  (100 to 600).

### Check the block before replying

Write the JSON to a temporary file and draw it:

```bash
ghostex visual check /tmp/block.json              # prints a PNG path, or names the problem
ghostex visual check /tmp/block.json --light      # light theme
ghostex visual check /tmp/block.json --width 360  # phone width
```

Look at the PNG, fix what reads badly, then paste the same JSON into your
reply. If `ghostex visual` is an unknown command, this Ghostex is too old to
check: skip the check.

### Examples

Grouped bars:

```visual
{
  "title": "Build time by platform",
  "mark": "bar",
  "height": 220,
  "data": {"values": [
    {"platform": "macOS", "build": "debug", "minutes": 4.2},
    {"platform": "macOS", "build": "release", "minutes": 9.8},
    {"platform": "Windows", "build": "debug", "minutes": 5.1},
    {"platform": "Windows", "build": "release", "minutes": 12.4},
    {"platform": "Linux", "build": "debug", "minutes": 3.6},
    {"platform": "Linux", "build": "release", "minutes": 8.9}
  ]},
  "encoding": {
    "x": {"field": "platform", "type": "nominal", "sort": null, "axis": {"title": null}},
    "xOffset": {"field": "build"},
    "y": {"field": "minutes", "type": "quantitative", "title": "Minutes"},
    "color": {"field": "build", "type": "nominal", "title": "Build"}
  }
}
```

For stacked bars, drop `xOffset`; bars that share an `x` stack by `color`.

A line chart with two series:

```visual
{
  "title": "Response time",
  "mark": {"type": "line", "point": true},
  "data": {"values": [
    {"day": "2026-10-01", "series": "p50", "ms": 120},
    {"day": "2026-10-02", "series": "p50", "ms": 118},
    {"day": "2026-10-03", "series": "p50", "ms": 131},
    {"day": "2026-10-01", "series": "p95", "ms": 340},
    {"day": "2026-10-02", "series": "p95", "ms": 310},
    {"day": "2026-10-03", "series": "p95", "ms": 365}
  ]},
  "encoding": {
    "x": {"field": "day", "type": "temporal", "title": "Day"},
    "y": {"field": "ms", "type": "quantitative", "title": "Milliseconds"},
    "color": {"field": "series", "type": "nominal", "title": "Percentile"},
    "tooltip": [{"field": "day", "type": "temporal"}, {"field": "ms", "type": "quantitative"}]
  }
}
```

Stats above a table:

```visual
{
  "title": "Test run",
  "rows": [
    {"stats": [
      {"label": "Passed", "value": 412, "tone": "good"},
      {"label": "Failed", "value": 3, "note": "all in sync/", "tone": "bad"},
      {"label": "Duration", "value": "2m 41s"}
    ]},
    {"title": "Failures", "table": {
      "columns": ["Test", "Error"],
      "rows": [
        ["sync::resume_after_sleep", "timed out after 30s"],
        ["sync::merge_conflict", "expected 2 rows, got 3"],
        ["sync::large_file", "permission denied"]
      ]
    }}
  ]
}
```

## HTML pages

For a mockup or a small interactive tool:

1. Write **one self-contained HTML file**: inline CSS and JS. Libraries from an
   https CDN load fine. Local images referenced by absolute path
   (`<img src="/Users/me/shot.png">`, `url(C:/shots/a.png)`) are embedded
   automatically; every other request must be https.
2. Style it with the theme variables Ghostex adds, so it matches the app in
   dark and light: `--background`, `--foreground`, `--muted-foreground`,
   `--border`, `--card`, `--primary` (accent color), `--accent` (hover and
   selected surfaces), `--chart-1` … `--chart-6`, `--radius`, `--font-sans`,
   `--font-mono`. The page body already has the theme's background, text color
   and font.
3. The page runs in a sandbox with no origin of its own: `localStorage`,
   cookies and requests to `http://` or `localhost` don't work. Keep state in
   memory.
4. Publish it:

   ```bash
   ghostex show page.html --title "Settings mockup"
   ```

5. Paste the ```visual block it prints into your reply exactly as printed.
   The chat shows it as a card for your page. The block carries
   `"open": "popup"`, the floating mark: clicking the card opens the page in a
   floating window over the chat (desktop), which closes when the reader
   clicks away. Leave the mark in for mockups and tools the reader looks at
   and dismisses. Publish with `--browser` after the file name (or remove the
   mark) when the page is better read in a full browser tab, such as a long
   report. Older Ghostex versions print the block without `"open"`: their
   cards open the page in the browser, and they ignore `--browser`.

If `ghostex show` is an unknown command, say this Ghostex is too old to show
pages and give the HTML file's path instead.
