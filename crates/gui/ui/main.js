// Tauri 2.x globals (require app.withGlobalTauri = true in tauri.conf.json).
// `core` and `event` are exposed automatically; plugins are not — for the
// dialog plugin we call its commands directly via invoke.
if (!window.__TAURI__) {
  console.error("Tauri runtime not available — withGlobalTauri must be true");
}
const invoke = window.__TAURI__?.core?.invoke;
const listen = window.__TAURI__?.event?.listen;

// ─── i18n ────────────────────────────────────────────────────────────────
const LANGS = window.HANGAR_LANGS || [{ code: "en", name: "English" }];
const I18N = window.HANGAR_I18N || { en: {} };

function detectLang() {
  const stored = localStorage.getItem("hangar.lang");
  if (stored && I18N[stored]) return stored;
  const navCode = (navigator.language || "en").split("-")[0].toLowerCase();
  return I18N[navCode] ? navCode : "en";
}

let currentLang = detectLang();
const t = (key) => I18N[currentLang]?.[key] ?? I18N.en[key] ?? key;

// Languages whose script flows right-to-left. The HTML `dir` attribute
// flips alignments and form-control affordances for them.
const RTL_LANGS = new Set(["ar", "fa", "he", "ur"]);

function applyTranslations() {
  document.documentElement.lang = currentLang;
  document.documentElement.dir = RTL_LANGS.has(currentLang) ? "rtl" : "ltr";
  document.querySelectorAll("[data-i18n]").forEach((el) => {
    el.textContent = t(el.dataset.i18n);
  });
  document.querySelectorAll("[data-i18n-placeholder]").forEach((el) => {
    el.placeholder = t(el.dataset.i18nPlaceholder);
  });
}

/// Render a backend error in the user's language. The backend emits
/// {code, message}; we look up `code` in the active translation table and
/// fall back to the raw message if there's no entry for that code.
function localizeError(e) {
  if (e && typeof e === "object" && e.code) {
    const translated = I18N[currentLang]?.[e.code] ?? I18N.en[e.code];
    return translated || e.message || String(e);
  }
  return e?.message || String(e);
}

function populateLangSelector(sel) {
  sel.innerHTML = "";
  for (const { code, name } of LANGS) {
    const opt = document.createElement("option");
    opt.value = code;
    opt.textContent = name;
    if (code === currentLang) opt.selected = true;
    sel.appendChild(opt);
  }
}

// ─── Theme ───────────────────────────────────────────────────────────
const THEMES = [
  { id: "hangar", name: "Hangar" },
  { id: "carbon", name: "Carbon" },
  { id: "matrix", name: "Matrix" },
  { id: "tron", name: "Tron" },
  { id: "synthwave", name: "Synthwave" },
  { id: "dracula", name: "Dracula" },
  { id: "catppuccin", name: "Catppuccin" },
  { id: "nord", name: "Nord" },
];

function detectTheme() {
  const stored = localStorage.getItem("hangar.theme");
  if (stored && THEMES.some((t) => t.id === stored)) return stored;
  return "hangar";
}

let currentTheme = detectTheme();

function applyTheme() {
  // The default "hangar" theme uses no data-theme attribute so the :root
  // variables (and prefers-color-scheme media query) take effect.
  if (currentTheme === "hangar") {
    document.documentElement.removeAttribute("data-theme");
  } else {
    document.documentElement.setAttribute("data-theme", currentTheme);
  }
  if (currentTheme === "matrix") startMatrixRain();
  else stopMatrixRain();
}

// ─── Matrix digital rain ─────────────────────────────────────────────
// Canvas-painted falling glyphs sampled from "tnsk/hangar". Only running
// while the Matrix theme is active.
const matrixCanvas = document.getElementById("matrix-rain");
const matrixCtx = matrixCanvas.getContext("2d");
const MATRIX_CHARS = "tnsk/hangar".split("");
const MATRIX_CELL = 16;
let matrixDrops = [];
let matrixRaf = null;

function sizeMatrixCanvas() {
  const dpr = window.devicePixelRatio || 1;
  const w = window.innerWidth;
  const h = window.innerHeight;
  matrixCanvas.width = Math.floor(w * dpr);
  matrixCanvas.height = Math.floor(h * dpr);
  matrixCanvas.style.width = w + "px";
  matrixCanvas.style.height = h + "px";
  matrixCtx.setTransform(dpr, 0, 0, dpr, 0, 0);
  const cols = Math.ceil(w / MATRIX_CELL);
  matrixDrops = new Array(cols)
    .fill(0)
    .map(() => Math.floor((Math.random() * h) / MATRIX_CELL));
}

function paintMatrixRain() {
  const w = window.innerWidth;
  const h = window.innerHeight;
  // Translucent fill creates the trailing-fade look as old draws bleed out.
  matrixCtx.fillStyle = "rgba(0, 0, 0, 0.07)";
  matrixCtx.fillRect(0, 0, w, h);
  matrixCtx.font = `${MATRIX_CELL - 2}px "IBM Plex Mono", ui-monospace, monospace`;
  matrixCtx.textBaseline = "top";

  for (let i = 0; i < matrixDrops.length; i++) {
    const ch = MATRIX_CHARS[Math.floor(Math.random() * MATRIX_CHARS.length)];
    const x = i * MATRIX_CELL;
    const y = matrixDrops[i] * MATRIX_CELL;
    // Brighter head cell, dimmer body — feels like trailing characters.
    matrixCtx.fillStyle = "rgba(170, 255, 190, 0.95)";
    matrixCtx.fillText(ch, x, y);
    if (matrixDrops[i] > 1) {
      matrixCtx.fillStyle = "rgba(77, 255, 122, 0.45)";
      matrixCtx.fillText(
        MATRIX_CHARS[Math.floor(Math.random() * MATRIX_CHARS.length)],
        x,
        y - MATRIX_CELL
      );
    }
    if (y > h && Math.random() > 0.975) {
      matrixDrops[i] = 0;
    }
    matrixDrops[i]++;
  }
  matrixRaf = requestAnimationFrame(paintMatrixRain);
}

function startMatrixRain() {
  sizeMatrixCanvas();
  if (matrixRaf === null) {
    matrixRaf = requestAnimationFrame(paintMatrixRain);
  }
}

function stopMatrixRain() {
  if (matrixRaf !== null) {
    cancelAnimationFrame(matrixRaf);
    matrixRaf = null;
  }
  matrixCtx.clearRect(0, 0, matrixCanvas.width, matrixCanvas.height);
}

window.addEventListener("resize", () => {
  if (matrixRaf !== null) sizeMatrixCanvas();
});

function populateThemeSelector(sel) {
  sel.innerHTML = "";
  for (const { id, name } of THEMES) {
    const opt = document.createElement("option");
    opt.value = id;
    opt.textContent = name;
    if (id === currentTheme) opt.selected = true;
    sel.appendChild(opt);
  }
}

async function dialogSave(options) {
  return await invoke("plugin:dialog|save", { options });
}

async function dialogOpen(options) {
  return await invoke("plugin:dialog|open", { options });
}

const PRESETS = {
  fast:     { level: 3,  solid: false, long: false, threads: 0 },
  balanced: { level: 9,  solid: true,  long: true,  threads: 0 },
  max:      { level: 19, solid: true,  long: true,  threads: 0 },
};

let currentPreset = "balanced";
let busy = false;

const $ = (id) => document.getElementById(id);
const drop = $("drop");
const result = $("result");
const resultTitle = $("result-title");
const resultBody = $("result-body");
const dismissBtn = $("dismiss");
const busyEl = $("busy");
const busyLabel = $("busy-label");
const customPanel = $("custom-panel");
const levelInput = $("level");
const levelVal = $("level-val");
const solidInput = $("solid");
const longInput = $("long");
const threadsInput = $("threads");
const busyPct = $("busy-pct");
const busyFile = $("busy-file");
const busyStats = $("busy-stats");
const progressFill = $("progress-fill");
const progressBar = document.querySelector(".progress");
const langSel = $("lang");
const themeSel = $("theme");
const busyCancel = $("busy-cancel");
const encryptInput = $("encrypt");
const encryptFields = $("encrypt-fields");
const encryptPw = $("encrypt-pw");
const encryptPw2 = $("encrypt-pw2");
const pwprompt = $("pwprompt");
const pwpromptInput = $("pwprompt-input");
const pwpromptOk = $("pwprompt-ok");
const pwpromptCancel = $("pwprompt-cancel");

function setPreset(name) {
  currentPreset = name;
  document.querySelectorAll(".preset").forEach((b) => {
    b.classList.toggle("active", b.dataset.preset === name);
  });
  customPanel.classList.toggle("hidden", name !== "custom");
  if (name !== "custom") {
    const p = PRESETS[name];
    levelInput.value = p.level;
    levelVal.textContent = p.level;
    solidInput.checked = p.solid;
    longInput.checked = p.long;
    threadsInput.value = p.threads;
  }
}

function currentSettings() {
  if (currentPreset === "custom") {
    return {
      level: parseInt(levelInput.value, 10),
      solid: solidInput.checked,
      long: longInput.checked,
      threads: parseInt(threadsInput.value, 10) || 0,
    };
  }
  return PRESETS[currentPreset];
}

document.querySelectorAll(".preset").forEach((b) => {
  b.addEventListener("click", () => setPreset(b.dataset.preset));
});

levelInput.addEventListener("input", () => {
  levelVal.textContent = levelInput.value;
});

encryptInput.addEventListener("change", () => {
  encryptFields.classList.toggle("hidden", !encryptInput.checked);
  if (encryptInput.checked) {
    encryptPw.focus();
  } else {
    encryptPw.value = "";
    encryptPw2.value = "";
  }
});

dismissBtn.addEventListener("click", () => {
  result.classList.add("hidden");
  result.classList.remove("error", "success");
});

// Throughput tracker — updated by every progress event during the active op.
let opStart = 0;
let opTotalBytes = 0;
let pendingProgress = null;
let rafQueued = false;

function applyProgress() {
  rafQueued = false;
  const p = pendingProgress;
  if (!p) return;
  if (p.total_bytes > 0) {
    progressBar.classList.remove("indeterminate");
    const pct = Math.min(100, (100 * p.current_bytes) / p.total_bytes);
    progressFill.style.width = `${pct}%`;
    busyPct.textContent = `${pct.toFixed(0)}%`;
  } else {
    progressBar.classList.add("indeterminate");
    busyPct.textContent = "";
  }
  busyFile.textContent = p.current_file || "";
  const elapsed = Math.max(0.001, (performance.now() - opStart) / 1000);
  const mbps = p.current_bytes / 1024 / 1024 / elapsed;
  const filesPart =
    p.files_total > 0
      ? `${p.files_done}/${p.files_total} ${t("statsFilesUnit")} · `
      : "";
  busyStats.textContent = `${filesPart}${mbps.toFixed(1)} MB/s`;
}

function queueProgress(p) {
  pendingProgress = p;
  if (!rafQueued) {
    rafQueued = true;
    requestAnimationFrame(applyProgress);
  }
}

function setBusy(on, label) {
  busy = on;
  busyEl.classList.toggle("hidden", !on);
  if (label) busyLabel.textContent = label;
  if (on) {
    opStart = performance.now();
    opTotalBytes = 0;
    pendingProgress = null;
    progressFill.style.width = "0%";
    busyPct.textContent = "0%";
    busyFile.textContent = "";
    busyStats.textContent = "";
    progressBar.classList.add("indeterminate");
    busyCancel.disabled = false;
    busyCancel.textContent = t("cancelButton");
  }
}

busyCancel.addEventListener("click", async () => {
  if (!busy) return;
  busyCancel.disabled = true;
  busyCancel.textContent = t("cancelling");
  try {
    await invoke("cancel_op");
  } catch (e) {
    console.warn("cancel_op failed", e);
  }
});

function fmtBytes(n) {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v >= 100 ? 0 : v >= 10 ? 1 : 2)} ${units[i]}`;
}

function fmtSecs(s) {
  if (s < 1) return `${(s * 1000).toFixed(0)} ms`;
  if (s < 60) return `${s.toFixed(2)} s`;
  return `${(s / 60).toFixed(1)} min`;
}

// Result card now leads with a HERO metric (e.g., "66.4%" "saved" for
// success, error message for errors). Pass `hero: { metric, unit }` to
// override; otherwise rows-only fall back. Rows are still shown beneath.
function showResult(kind, title, rows, hero) {
  result.classList.remove("hidden", "error", "success");
  result.classList.add(kind);
  resultTitle.textContent = title;
  const metricEl = $("result-metric");
  const unitEl = $("result-metric-unit");
  if (hero) {
    metricEl.textContent = hero.metric;
    unitEl.textContent = hero.unit || "";
  } else {
    metricEl.textContent = "";
    unitEl.textContent = "";
  }
  resultBody.innerHTML = rows
    .map(([k, v]) => `<span>${k}</span><strong>${v}</strong>`)
    .join("");
}

async function compressPaths(paths) {
  const settings = currentSettings();

  // Validate password fields before opening any dialog.
  let password = null;
  if (encryptInput.checked) {
    const pw = encryptPw.value;
    const pw2 = encryptPw2.value;
    if (!pw) {
      showResult("error", t("resultCompressFailed"), [], {
        metric: t("passwordRequired"),
        unit: "",
      });
      return;
    }
    if (pw !== pw2) {
      showResult("error", t("resultCompressFailed"), [], {
        metric: t("passwordMismatch"),
        unit: "",
      });
      return;
    }
    password = pw;
  }

  // Suggest a default archive name from the first input.
  const first = paths[0];
  const baseName = first.split(/[/\\]/).pop() || "archive";
  const defaultName = baseName.replace(/\.[^.]+$/, "") + ".hgr";
  const output = await dialogSave({
    title: t("dialogSaveTitle"),
    defaultPath: defaultName,
    filters: [{ name: t("dialogSaveFilter"), extensions: ["hgr"] }],
  });
  if (!output) return;

  setBusy(true, t("compressing"));
  try {
    const r = await invoke("compress", {
      inputs: paths,
      output,
      level: settings.level,
      threads: settings.threads,
      solid: settings.solid,
      long: settings.long,
      blockSize: 64 * 1024 * 1024,
      password,
    });
    const ratio = r.ratio_pct.toFixed(1);
    // % saved is the more emotionally satisfying number than ratio-of-input.
    const savedPct =
      r.bytes_in > 0 ? (100 * (1 - r.bytes_out / r.bytes_in)).toFixed(1) : "0";
    const speed = (r.bytes_in / 1024 / 1024 / r.elapsed_secs).toFixed(1);
    showResult(
      "success",
      t("resultCompressed"),
      [
        [t("rowFiles"), r.files.toLocaleString()],
        [t("rowInput"), fmtBytes(r.bytes_in)],
        [t("rowOutput"), fmtBytes(r.bytes_out)],
        [t("rowRatio"), `${ratio}${t("ratioOfInput")}`],
        [t("rowTime"), fmtSecs(r.elapsed_secs)],
        [t("rowThroughput"), `${speed} MB/s`],
        [t("rowSavedTo"), r.archive],
      ],
      { metric: `${savedPct}%`, unit: t("savedLabel") }
    );
  } catch (e) {
    showResult(
      "error",
      t("resultCompressFailed"),
      [],
      { metric: localizeError(e), unit: "" }
    );
  } finally {
    setBusy(false);
  }
}

/// Open the password modal; resolves to the entered string or null on cancel.
function askPassword() {
  return new Promise((resolve) => {
    pwpromptInput.value = "";
    pwprompt.classList.remove("hidden");
    pwpromptInput.focus();
    const cleanup = () => {
      pwprompt.classList.add("hidden");
      pwpromptOk.removeEventListener("click", onOk);
      pwpromptCancel.removeEventListener("click", onCancel);
      pwpromptInput.removeEventListener("keydown", onKey);
    };
    const onOk = () => {
      const pw = pwpromptInput.value;
      cleanup();
      resolve(pw || null);
    };
    const onCancel = () => {
      cleanup();
      resolve(null);
    };
    const onKey = (e) => {
      if (e.key === "Enter") onOk();
      else if (e.key === "Escape") onCancel();
    };
    pwpromptOk.addEventListener("click", onOk);
    pwpromptCancel.addEventListener("click", onCancel);
    pwpromptInput.addEventListener("keydown", onKey);
  });
}

async function extractArchive(archive) {
  // Probe first to see if the archive is encrypted; only prompt then.
  let encrypted = false;
  try {
    const probeResult = await invoke("probe", { archive });
    encrypted = !!probeResult.encrypted;
  } catch (e) {
    showResult("error", t("resultExtractFailed"), [
      [t("rowError"), localizeError(e)],
    ]);
    return;
  }

  let password = null;
  if (encrypted) {
    password = await askPassword();
    if (password === null) return; // user cancelled
  }

  const outputDir = await dialogOpen({
    title: t("dialogExtractTitle"),
    directory: true,
    multiple: false,
  });
  if (!outputDir) return;

  setBusy(true, t("extracting"));
  try {
    const r = await invoke("extract", { archive, outputDir, password });
    showResult(
      "success",
      t("resultExtracted"),
      [
        [t("rowEntries"), r.entries.toLocaleString()],
        [t("rowTime"), fmtSecs(r.elapsed_secs)],
        [t("rowOutputDir"), r.output_dir],
      ],
      { metric: r.entries.toLocaleString(), unit: t("entriesLabel") }
    );
  } catch (e) {
    showResult(
      "error",
      t("resultExtractFailed"),
      [],
      { metric: localizeError(e), unit: "" }
    );
  } finally {
    setBusy(false);
  }
}

async function handlePaths(paths) {
  if (busy || !paths || paths.length === 0) return;
  // Single .hgr → extract; otherwise compress.
  if (paths.length === 1 && paths[0].toLowerCase().endsWith(".hgr")) {
    await extractArchive(paths[0]);
  } else {
    await compressPaths(paths);
  }
}

// HTML5 dragover/drop drives hover styling; real paths arrive via Tauri.
drop.addEventListener("dragover", (e) => {
  e.preventDefault();
  drop.classList.add("hover");
});
drop.addEventListener("dragleave", () => drop.classList.remove("hover"));
drop.addEventListener("drop", (e) => {
  e.preventDefault();
  drop.classList.remove("hover");
});

if (typeof listen === "function") {
  listen("tauri://drag-enter", () => drop.classList.add("hover"));
  listen("tauri://drag-leave", () => drop.classList.remove("hover"));
  listen("tauri://drag-drop", (event) => {
    drop.classList.remove("hover");
    const paths = event.payload?.paths || [];
    handlePaths(paths);
  });
  listen("progress", (event) => queueProgress(event.payload));
} else {
  console.warn("Tauri event listener unavailable; drag-drop disabled.");
}

// Click-to-pick fallback (handy on systems where DnD wasn't enabled).
drop.addEventListener("click", async () => {
  if (busy) return;
  const sel = await dialogOpen({ multiple: true, directory: false });
  if (!sel) return;
  await handlePaths(Array.isArray(sel) ? sel : [sel]);
});

// Wire up i18n: render selector, apply translations, persist on change.
populateLangSelector(langSel);
applyTranslations();
langSel.addEventListener("change", () => {
  currentLang = langSel.value;
  localStorage.setItem("hangar.lang", currentLang);
  applyTranslations();
});

// Wire up themes the same way.
populateThemeSelector(themeSel);
applyTheme();
themeSel.addEventListener("change", () => {
  currentTheme = themeSel.value;
  localStorage.setItem("hangar.theme", currentTheme);
  applyTheme();
});

setPreset("balanced");
