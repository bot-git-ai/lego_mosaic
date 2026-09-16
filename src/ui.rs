// Copyright (c) 2026 Witalis Domitrz <witekdomitrz@gmail.com>
// AGPL License

//! The web UI: one page that renders the form, calls `POST /api/convert`,
//! and shows the stud preview, per-color parts list and build rows.

use std::fmt::Write as _;

/// One `<option>` element with the selected flag when `selected`.
fn option(value: &str, label: &str, selected: bool) -> String {
    let marker = if selected { " selected" } else { "" };
    format!("<option value=\"{value}\"{marker}>{label}</option>")
}

/// The palette swatch data the JS exclusion toggles are built from:
/// `const paletteSwatches = {"id": [[name, hex, index], …], …};`.
/// Keys are quoted — bare `mosaic-maker` is invalid JS and would make the
/// whole script fail to parse.
fn palette_swatches() -> String {
    let mut out = String::from("const paletteSwatches = {");
    for (i, (id, _, tiles)) in crate::palette::all().iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write!(out, "\"{id}\": [").expect("writing to String cannot fail");
        for (j, tile) in tiles.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            write!(out, "[\"{}\", \"{}\", {j}]", tile.name, tile.hex())
                .expect("writing to String cannot fail");
        }
        out.push(']');
    }
    out.push_str("};");
    out
}

/// The full page. Vanilla JS, no build step: the fetch submits the form as
/// multipart, the response JSON carries the SVG preview and the parts data.
pub(crate) fn page() -> String {
    let palette_options: String = crate::palette::all()
        .iter()
        .map(|(id, label, _)| option(id, label, *id == "mosaic-maker"))
        .collect();
    let size_options: String = [48usize, 32, 64, 96]
        .iter()
        .map(|n| option(&n.to_string(), &format!("{n} × {n}"), *n == 48))
        .collect();
    let palette_json = palette_swatches();
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>LEGO Mosaic Maker</title>
<style>
  :root {{ --bg:#f4f2ec; --card:#fff; --ink:#1c1c1c; --accent:#d01012; --line:#e2ded4; }}
  * {{ box-sizing:border-box; }}
  body {{ margin:0; font:15px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif;
         background:var(--bg); color:var(--ink); }}
  header {{ background:var(--accent); color:#fff; padding:14px 20px; }}
  header h1 {{ margin:0; font-size:20px; font-weight:700; letter-spacing:.3px; }}
  header p {{ margin:2px 0 0; opacity:.85; font-size:13px; }}
  main {{ max-width:1100px; margin:20px auto; padding:0 16px;
          display:grid; grid-template-columns:340px 1fr; gap:20px; }}
  @media (max-width:800px) {{ main {{ grid-template-columns:1fr; }} }}
  .card {{ background:var(--card); border:1px solid var(--line); border-radius:10px;
           padding:16px; }}
  label {{ display:block; margin:10px 0 2px; font-weight:600; font-size:13px; }}
  select, input[type=number] {{ width:100%; padding:7px 8px; border:1px solid var(--line);
          border-radius:6px; font:inherit; background:#fff; }}
  .row {{ display:flex; gap:10px; }} .row > div {{ flex:1; }}
  input[type=range] {{ width:100%; }}
  .check {{ display:flex; align-items:center; gap:8px; margin-top:10px; font-weight:400; }}
  .check input {{ width:auto; }}
  button {{ margin-top:16px; width:100%; padding:11px; font:inherit; font-weight:700;
            background:var(--accent); color:#fff; border:0; border-radius:8px;
            cursor:pointer; }}
  button:disabled {{ opacity:.5; cursor:wait; }}
  .hint {{ font-size:12px; color:#777; margin-top:4px; }}
  #result {{ display:none; }}
  #preview svg {{ width:100%; height:auto; border-radius:6px;
                  box-shadow:0 2px 10px rgba(0,0,0,.12); image-rendering:auto; }}
  h2 {{ font-size:16px; margin:18px 0 8px; }}
  .parts {{ display:flex; flex-wrap:wrap; gap:8px; }}
  .part {{ display:flex; align-items:center; gap:7px; border:1px solid var(--line);
           border-radius:20px; padding:4px 12px 4px 5px; font-size:13px; }}
  .dot {{ width:20px; height:20px; border-radius:50%; border:1px solid rgba(0,0,0,.15); }}
  table {{ border-collapse:collapse; font-size:12px; width:100%; }}
  td {{ border:1px solid var(--line); padding:0; }}
  td span {{ display:block; width:26px; height:26px; }}
  .rows-note {{ font-size:12px; color:#777; }}
  #error {{ color:var(--accent); display:none; margin-top:10px; font-weight:600; }}
  .drop {{ border:2px dashed var(--line); border-radius:8px; padding:14px; text-align:center;
           cursor:pointer; }}
  .drop.has-file {{ border-color:var(--accent); }}
  #filename {{ font-size:12px; color:#777; }}
</style>
</head>
<body>
<header>
  <h1>LEGO Mosaic Maker</h1>
  <p>Picture → {size_default}×{size_default} round-plate mosaic, with parts list &amp; build rows</p>
</header>
<main>
  <div class="card">
    <form id="form">
      <div class="drop" id="drop">
        <strong>Choose a picture</strong><br>or drop it here
        <input type="file" id="image" name="image" accept="image/*" hidden>
        <div id="filename"></div>
      </div>

      <label for="palette">Palette</label>
      <select id="palette" name="palette">{palette_options}</select>

      <label for="size">Size (studs)</label>
      <select id="size" name="size">{size_options}</select>

      <label for="order">Matching</label>
      <select id="order" name="order">
        <option value="sharp" selected>Sharp (flat art) — quantize, then vote per stud</option>
        <option value="smooth">Smooth (photos) — average per stud</option>
      </select>

      <label for="saturation">Saturation ×<span id="sat_v">1.15</span></label>
      <input type="range" id="saturation" name="saturation" min="1" max="3" step="0.05" value="1.15">
      <div class="hint">Low palettes wash pastels out — a boost helps.</div>

      <label for="contrast">Contrast ×<span id="con_v">1.05</span></label>
      <input type="range" id="contrast" name="contrast" min="0.7" max="1.8" step="0.05" value="1.05">

      <label class="check"><input type="checkbox" id="dither" name="dither"> Dither (gradients / photos)</label>
      <label class="check"><input type="checkbox" id="white_background" name="white_background" checked>
        Force white background</label>
      <div class="hint">Keeps cut-out illustrations clean; uncheck for photos.</div>

      <div id="exclusions"></div>

      <button type="submit" id="go">Build mosaic</button>
      <div id="error"></div>
    </form>
  </div>

  <div class="card" id="result">
    <h2 style="margin-top:0">Preview <span class="hint" id="gridnote"></span></h2>
    <div id="preview"></div>
    <h2>Parts needed</h2>
    <div class="parts" id="parts"></div>
    <h2>Build rows</h2>
    <div class="rows-note">Row 1 = bottom of the mosaic. Follow bottom-up.</div>
    <div id="rows" style="overflow-x:auto"></div>
  </div>
</main>
<script>
"use strict";
const form = document.getElementById("form");
const apiBase = location.pathname.endsWith("/")
  ? location.pathname
  : location.pathname + "/";
const drop = document.getElementById("drop");
const fileInput = document.getElementById("image");
const errorBox = document.getElementById("error");
let file = null;

function pickFile(f) {{
  if (!f || !f.type.startsWith("image/")) return;
  file = f;
  document.getElementById("filename").textContent = f.name + " (" + Math.round(f.size/1024) + " kB)";
  drop.classList.add("has-file");
}}
fileInput.addEventListener("change", () => pickFile(fileInput.files[0]));
drop.addEventListener("click", e => {{
  if (e.target === fileInput) return; // synthetic click from .click() bubbling up
  fileInput.click();
}});
drop.addEventListener("dragover", e => {{ e.preventDefault(); }});
drop.addEventListener("drop", e => {{
  e.preventDefault();
  pickFile(e.dataTransfer.files[0]);
}});
document.getElementById("saturation").addEventListener("input", e =>
  document.getElementById("sat_v").textContent = Number(e.target.value).toFixed(2));
document.getElementById("contrast").addEventListener("input", e =>
  document.getElementById("con_v").textContent = Number(e.target.value).toFixed(2));

// Exclusions: one toggle per color of the chosen palette.
{palette_json}
const paletteSelect = document.getElementById("palette");
function renderExclusions() {{
  const box = document.getElementById("exclusions");
  box.innerHTML = "";
  (paletteSwatches[paletteSelect.value] || []).forEach(([name, hex, index]) => {{
    const label = document.createElement("label");
    label.className = "check";
    const cb = document.createElement("input");
    cb.type = "checkbox"; cb.checked = true;
    cb.addEventListener("change", renderExclusions);
    label.appendChild(cb);
    const dot = document.createElement("span");
    dot.className = "dot"; dot.style.background = hex;
    label.appendChild(dot);
    label.appendChild(document.createTextNode(" " + name));
    box.appendChild(label);
  }});
}}
paletteSelect.addEventListener("change", renderExclusions);
renderExclusions();

form.addEventListener("submit", async e => {{
  e.preventDefault();
  errorBox.style.display = "none";
  if (!file) {{
    errorBox.textContent = "Choose a picture first.";
    errorBox.style.display = "block";
    return;
  }}
  const go = document.getElementById("go");
  go.disabled = true; go.textContent = "Building…";
  try {{
    const fd = new FormData();
    fd.append("image", file);
    fd.append("palette", paletteSelect.value);
    fd.append("size", document.getElementById("size").value);
    fd.append("order", document.getElementById("order").value);
    fd.append("saturation", document.getElementById("saturation").value);
    fd.append("contrast", document.getElementById("contrast").value);
    fd.append("dither", document.getElementById("dither").checked ? "on" : "");
    fd.append("white_background", document.getElementById("white_background").checked ? "on" : "");
    document.querySelectorAll("#exclusions input").forEach((cb, i) => {{
      if (!cb.checked) {{
        fd.append("exclude", String(i));
      }}
    }});
    const res = await fetch(apiBase + "api/convert", {{ method: "POST", body: fd }});
    if (!res.ok) throw new Error((await res.json()).error || res.statusText);
    const data = await res.json();
    document.getElementById("preview").innerHTML = data.svg;
    document.getElementById("gridnote").textContent =
      data.width + " × " + data.height + " studs, " + data.tiles.toLocaleString() + " tiles";
    const parts = document.getElementById("parts");
    parts.innerHTML = "";
    for (const p of data.parts) {{
      const el = document.createElement("div");
      el.className = "part";
      el.innerHTML = '<span class="dot" style="background:' + p.hex + '"></span>' +
        p.count.toLocaleString() + " × " + p.name;
      parts.appendChild(el);
    }}
    const rows = document.getElementById("rows");
    let html = "<table>";
    data.rows.forEach((row, i) => {{
      html += "<tr>";
      row.forEach(cell => {{
        html += '<td title="' + cell[0] + '"><span style="background:' + cell[1] +
                '"></span></td>';
      }});
      html += "</tr>";
    }});
    html += "</table>";
    rows.innerHTML = html;
    document.getElementById("result").style.display = "block";
    document.getElementById("result").scrollIntoView({{ behavior: "smooth", block: "start" }});
  }} catch (err) {{
    errorBox.textContent = "Conversion failed: " + err.message;
    errorBox.style.display = "block";
  }} finally {{
    go.disabled = false; go.textContent = "Build mosaic";
  }}
}});
</script>
</body>
</html>"##,
        size_default = 48,
        palette_options = palette_options,
        size_options = size_options
    )
}
