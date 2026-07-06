const { invoke } = window.__TAURI__.core;

const LANG_KEY = "solopic.lang";

const state = {
  dir: null,
  files: [],          // [{name, width, height, bytes}]
  selected: new Set(),
  tab: "crop",
  undoLog: null,
  previewTimer: null,
  lastReport: null,
};

const $ = (id) => document.getElementById(id);

// ---------------------------------------------------------------- i18n

function detectLang() {
  const saved = localStorage.getItem(LANG_KEY);
  if (saved && I18N[saved]) return saved;
  const nav = (navigator.language || "en").toLowerCase();
  for (const code of Object.keys(I18N)) {
    if (nav.startsWith(code)) return code;
  }
  return "en";
}

let LANG = detectLang();

function t(key, vars) {
  let s = (I18N[LANG] && I18N[LANG][key]) ?? I18N.en[key] ?? key;
  if (vars) {
    for (const [k, v] of Object.entries(vars)) s = s.replaceAll(`{${k}}`, v);
  }
  return s;
}

function applyI18n() {
  document.documentElement.lang = LANG;
  document.title = "SoloPic · " + t("subtitle");
  document.querySelectorAll("[data-i18n]").forEach((el) => (el.textContent = t(el.dataset.i18n)));
  document.querySelectorAll("[data-i18n-html]").forEach((el) => (el.innerHTML = t(el.dataset.i18nHtml)));
  document.querySelectorAll("[data-i18n-ph]").forEach((el) => (el.placeholder = t(el.dataset.i18nPh)));
  document.querySelectorAll("[data-i18n-title]").forEach((el) => (el.title = t(el.dataset.i18nTitle)));
  if (!state.dir) $("folder-path").textContent = t("folderNone");
  $("busy-text").textContent = t("busyDefault");
}

async function setLang(lang) {
  LANG = lang;
  localStorage.setItem(LANG_KEY, lang);
  applyI18n();
  await invoke("set_locale", { lang }).catch(() => {});
  schedulePreview();
  if (state.lastReport) renderReport(...state.lastReport);
}

// ---------------------------------------------------------------- 通用

function busy(on, text) {
  $("busy").style.display = on ? "flex" : "none";
  if (text) $("busy-text").textContent = text;
}

function firstSelected() {
  return state.files.find((f) => state.selected.has(f.name)) || null;
}

function updateSelCount() {
  $("sel-count").textContent = `${state.selected.size} / ${state.files.length}`;
  $("btn-run").disabled = state.selected.size === 0 || state.tab === "rename";
  $("chk-all").checked = state.files.length > 0 && state.selected.size === state.files.length;
}

function outputMode() {
  const overwrite = document.querySelector('input[name="outmode"]:checked').value === "overwrite";
  return { outputDir: null, overwrite };
}

// ---------------------------------------------------------------- 文件夹与文件列表

async function pickFolder() {
  const dir = await invoke("pick_folder");
  if (dir) {
    state.dir = dir;
    $("folder-path").textContent = dir;
    $("folder-path").title = dir;
    $("btn-reload").disabled = false;
    await reload();
  }
}

async function reload() {
  if (!state.dir) return;
  busy(true, t("busyLoad"));
  try {
    state.files = await invoke("list_dir", { dir: state.dir });
    state.selected = new Set(state.files.map((f) => f.name));
    renderGrid();
    schedulePreview();
  } catch (e) {
    alert(t("alertLoad") + e);
  } finally {
    busy(false);
    updateSelCount();
  }
}

function renderGrid() {
  const grid = $("filegrid");
  grid.innerHTML = "";
  if (state.files.length === 0) {
    const div = document.createElement("div");
    div.className = "empty";
    div.textContent = t("noImages");
    grid.appendChild(div);
    return;
  }
  for (const f of state.files) {
    const card = document.createElement("div");
    card.className = "fcard sel";
    card.dataset.name = f.name;
    card.innerHTML = `<img loading="lazy" alt="" /><div class="fname" title="${f.name}">${f.name}</div><div class="fdim">${f.width}×${f.height}</div>`;
    card.addEventListener("click", () => {
      if (state.selected.has(f.name)) {
        state.selected.delete(f.name);
        card.classList.remove("sel");
      } else {
        state.selected.add(f.name);
        card.classList.add("sel");
      }
      updateSelCount();
      schedulePreview();
    });
    grid.appendChild(card);
    invoke("thumb", { dir: state.dir, name: f.name, max: 200 })
      .then((url) => (card.querySelector("img").src = url))
      .catch(() => {});
  }
}

// ---------------------------------------------------------------- 参数读取

function cropSpec() {
  return {
    left: +$("crop-left").value || 0,
    top: +$("crop-top").value || 0,
    right: +$("crop-right").value || 0,
    bottom: +$("crop-bottom").value || 0,
  };
}

function adjustSpec() {
  return {
    brightness: +$("adj-brightness").value / 100,
    contrast: +$("adj-contrast").value / 100,
    saturation: +$("adj-saturation").value / 100,
    sharpness: +$("adj-sharpness").value / 100,
    grayscale: $("adj-grayscale").checked,
  };
}

function enhanceOpts() {
  return {
    mode: document.querySelector('input[name="enh-mode"]:checked').value,
    deskew: $("enh-deskew").checked,
    maxDeskewDeg: Math.min(45, Math.max(1, +$("enh-deskew-max").value || 20)),
    denoise: $("enh-denoise").checked,
    onlyDocuments: $("enh-onlydoc").checked,
  };
}

// ---------------------------------------------------------------- 预览

function schedulePreview() {
  clearTimeout(state.previewTimer);
  state.previewTimer = setTimeout(refreshPreview, 350);
}

async function refreshPreview() {
  const f = firstSelected();
  const zone = $("preview-zone");
  if (!f || state.tab === "rename") {
    zone.style.display = state.tab === "rename" ? "none" : "flex";
    if (!f) {
      $("pv-before").src = "";
      $("pv-after").src = "";
      renderCropOverlay(null);
      $("crop-size-hint").textContent = "";
    }
    return;
  }
  zone.style.display = "flex";
  const args = { dir: state.dir, name: f.name, kind: state.tab };
  if (state.tab === "crop") {
    const spec = cropSpec();
    const w = f.width - spec.left - spec.right;
    const h = f.height - spec.top - spec.bottom;
    $("crop-size-hint").textContent =
      w > 0 && h > 0
        ? t("cropFirst", { name: f.name, w: f.width, h: f.height, w2: w, h2: h })
        : t("cropExceed", { name: f.name, w: f.width, h: f.height });
    if (spec.left + spec.top + spec.right + spec.bottom === 0 || w <= 0 || h <= 0) {
      try {
        const url = await invoke("thumb", { dir: state.dir, name: f.name, max: 900 });
        $("pv-before").src = url;
        $("pv-after").src = url;
        renderCropOverlay(f, spec);
      } catch (_) {}
      return;
    }
    args.crop = spec;
  } else if (state.tab === "adjust") {
    const spec = adjustSpec();
    args.adjust = spec;
    if (
      spec.brightness === 1 && spec.contrast === 1 && spec.saturation === 1 &&
      spec.sharpness === 1 && !spec.grayscale
    ) {
      try {
        const url = await invoke("thumb", { dir: state.dir, name: f.name, max: 900 });
        $("pv-before").src = url;
        $("pv-after").src = url;
        renderCropOverlay(null);
      } catch (_) {}
      return;
    }
  } else if (state.tab === "enhance") {
    const o = enhanceOpts();
    args.mode = o.mode;
    args.deskew = o.deskew;
    args.maxDeskewDeg = o.maxDeskewDeg;
    args.denoise = o.denoise;
  }
  try {
    const [before, after] = await invoke("preview", args);
    $("pv-before").src = before;
    $("pv-after").src = after;
    renderCropOverlay(state.tab === "crop" ? f : null, state.tab === "crop" ? cropSpec() : null);
  } catch (e) {
    console.error(e);
  }
}

function renderCropOverlay(f, spec) {
  const ov = $("crop-overlay");
  ov.innerHTML = "";
  if (!f || !spec) return;
  const img = $("pv-before");
  const place = () => {
    ov.style.left = img.offsetLeft + "px";
    ov.style.top = img.offsetTop + "px";
    ov.style.width = img.clientWidth + "px";
    ov.style.height = img.clientHeight + "px";
    const sx = img.clientWidth / f.width;
    const sy = img.clientHeight / f.height;
    const mk = (css) => {
      const d = document.createElement("div");
      d.className = "shade";
      Object.assign(d.style, css);
      ov.appendChild(d);
    };
    if (spec.left > 0) mk({ left: 0, top: 0, bottom: 0, width: spec.left * sx + "px" });
    if (spec.right > 0) mk({ right: 0, top: 0, bottom: 0, width: spec.right * sx + "px" });
    if (spec.top > 0) mk({ left: 0, right: 0, top: 0, height: spec.top * sy + "px" });
    if (spec.bottom > 0) mk({ left: 0, right: 0, bottom: 0, height: spec.bottom * sy + "px" });
  };
  if (img.complete && img.clientWidth > 0) place();
  else img.onload = place;
}

// ---------------------------------------------------------------- 执行与报告

function renderReport(report, undoLog, parseErrors) {
  state.lastReport = [report, undoLog, parseErrors];
  const el = $("report");
  const summary = t("repSummary", {
    ok: report.ok.length,
    skip: report.skipped.length,
    err: report.errors.length,
  });
  let html = `<div class="rep-summary">${report.dry_run ? t("repPreview") : ""}${summary}`;
  const overwrite = document.querySelector('input[name="outmode"]:checked').value === "overwrite";
  if (!report.dry_run && report.ok.length > 0 && state.tab !== "rename" && !overwrite && state.dir) {
    html += `<button class="openbtn small ghost" onclick="window.__openOut()">${t("repOpen")}</button>`;
  }
  html += "</div><ul>";
  for (const e of (parseErrors || [])) html += `<li class="err">⚠ ${e}</li>`;
  for (const e of report.ok) {
    const base = e.file.split("\\").pop();
    const out = e.output ? " → " + e.output.split("\\").pop() : "";
    html += `<li class="ok">✓ ${base}${out}${e.detail ? "　(" + e.detail + ")" : ""}</li>`;
  }
  for (const e of report.skipped) html += `<li class="skip">− ${e.file.split("\\").pop()}　[${e.detail || ""}]</li>`;
  for (const e of report.errors) html += `<li class="err">✗ ${e.file.split("\\").pop()}　[${e.detail || ""}]</li>`;
  html += "</ul>";
  el.innerHTML = html;
  if (undoLog) {
    state.undoLog = undoLog;
    $("ren-undo").style.display = "";
  }
}

window.__openOut = () => {
  if (state.dir) invoke("open_in_explorer", { path: state.dir + "\\pic-output" }).catch(() => {});
};

async function run() {
  if (!state.dir || state.selected.size === 0) return;
  const files = state.files.filter((f) => state.selected.has(f.name)).map((f) => f.name);
  const { outputDir, overwrite } = outputMode();
  if (overwrite && !confirm(t("cfmOverwrite", { n: files.length }))) return;
  busy(true, t("busyRun", { n: files.length }));
  try {
    let report;
    if (state.tab === "crop") {
      report = await invoke("run_crop", { dir: state.dir, files, spec: cropSpec(), outputDir, overwrite });
    } else if (state.tab === "adjust") {
      report = await invoke("run_adjust", { dir: state.dir, files, spec: adjustSpec(), outputDir, overwrite });
    } else if (state.tab === "enhance") {
      const o = enhanceOpts();
      report = await invoke("run_enhance", {
        dir: state.dir, files, mode: o.mode, deskew: o.deskew, maxDeskewDeg: o.maxDeskewDeg,
        denoise: o.denoise, onlyDocuments: o.onlyDocuments, outputDir, overwrite,
      });
    }
    renderReport(report);
    if (overwrite) reload();
  } catch (e) {
    alert(t("alertRun") + e);
  } finally {
    busy(false);
  }
}

async function runRename(execute) {
  if (!state.dir) return alert(t("alertPickFolder"));
  const text = $("ren-text").value.trim();
  if (!text) return alert(t("alertFillMap"));
  if (execute && !confirm(t("cfmRename"))) return;
  busy(true, execute ? t("busyRename") : t("busyCheck"));
  try {
    const res = await invoke("run_rename", { dir: state.dir, mappingText: text, execute });
    renderReport(res.report, res.undo_log, res.parse_errors);
    if (execute) reload();
  } catch (e) {
    alert(t("alertFail") + e);
  } finally {
    busy(false);
  }
}

async function undoRename() {
  if (!state.undoLog) return;
  busy(true, t("busyUndo"));
  try {
    const report = await invoke("run_undo", { dir: state.dir, undoLog: state.undoLog });
    renderReport(report);
    state.undoLog = null;
    $("ren-undo").style.display = "none";
    reload();
  } catch (e) {
    alert(t("alertUndo") + e);
  } finally {
    busy(false);
  }
}

// ---------------------------------------------------------------- 事件绑定

$("btn-pick").addEventListener("click", pickFolder);
$("btn-reload").addEventListener("click", reload);
$("btn-run").addEventListener("click", run);
$("chk-all").addEventListener("change", (e) => {
  state.selected = e.target.checked ? new Set(state.files.map((f) => f.name)) : new Set();
  document.querySelectorAll(".fcard").forEach((c) => c.classList.toggle("sel", e.target.checked));
  updateSelCount();
  schedulePreview();
});

document.querySelectorAll(".tab").forEach((tb) =>
  tb.addEventListener("click", () => {
    state.tab = tb.dataset.tab;
    document.querySelectorAll(".tab").forEach((x) => x.classList.toggle("active", x === tb));
    document.querySelectorAll(".panel").forEach((p) => p.classList.remove("active"));
    $("panel-" + state.tab).classList.add("active");
    $("output-zone").style.display = state.tab === "rename" ? "none" : "flex";
    $("report").innerHTML = "";
    state.lastReport = null;
    updateSelCount();
    schedulePreview();
  })
);

for (const id of ["crop-left", "crop-top", "crop-right", "crop-bottom"]) {
  $(id).addEventListener("input", schedulePreview);
}
for (const id of ["adj-brightness", "adj-contrast", "adj-saturation", "adj-sharpness"]) {
  $(id).addEventListener("input", () => {
    $(id + "-v").textContent = $(id).value + "%";
    schedulePreview();
  });
}
$("adj-grayscale").addEventListener("change", schedulePreview);
$("adj-reset").addEventListener("click", () => {
  for (const [id, v] of [["adj-brightness", 100], ["adj-contrast", 100], ["adj-saturation", 100], ["adj-sharpness", 100]]) {
    $(id).value = v;
    $(id + "-v").textContent = v + "%";
  }
  $("adj-grayscale").checked = false;
  schedulePreview();
});
document.querySelectorAll('input[name="enh-mode"]').forEach((r) => r.addEventListener("change", schedulePreview));
$("enh-deskew").addEventListener("change", schedulePreview);
$("enh-deskew-max").addEventListener("input", schedulePreview);
$("enh-denoise").addEventListener("change", schedulePreview);

$("ren-import").addEventListener("click", async () => {
  try {
    const text = await invoke("pick_map_file");
    if (text != null) $("ren-text").value = text;
  } catch (e) {
    alert(t("alertRead") + e);
  }
});
$("ren-preview").addEventListener("click", () => runRename(false));
$("ren-exec").addEventListener("click", () => runRename(true));
$("ren-undo").addEventListener("click", undoRename);

$("lang-sel").value = LANG;
$("lang-sel").addEventListener("change", (e) => setLang(e.target.value));

// ---------------------------------------------------------------- 启动

applyI18n();
invoke("set_locale", { lang: LANG }).catch(() => {});
invoke("initial_lang")
  .then((lang) => {
    if (lang && I18N[lang] && lang !== LANG) {
      $("lang-sel").value = lang;
      setLang(lang);
    }
  })
  .catch(() => {});

// 启动参数带文件夹时直接载入（拖文件夹到 exe / 命令行打开）
invoke("initial_dir")
  .then(async (dir) => {
    if (dir) {
      state.dir = dir;
      $("folder-path").textContent = dir;
      $("folder-path").title = dir;
      $("btn-reload").disabled = false;
      await reload();
      if (await invoke("autotest_mode").catch(() => false)) {
        setTimeout(() => {
          document.querySelector('[data-tab="enhance"]').click();
          setTimeout(() => $("btn-run").click(), 2000);
        }, 1500);
      }
    }
  })
  .catch(() => {});
