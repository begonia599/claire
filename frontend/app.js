// rdm 主窗口前端：任务列表 + 下载确认模态（主窗口内）+ 分类设置
const { invoke } = window.__TAURI__.core;

const $list = document.getElementById("task-list");
const $statusText = document.getElementById("status-text");
const $url = document.getElementById("url");
const $add = document.getElementById("add-btn");

// 确认模态
const $cfModal = document.getElementById("confirm-modal");
const $cfUrl = document.getElementById("cf-url");
const $cfName = document.getElementById("cf-name");
const $cfCat = document.getElementById("cf-cat");
const $cfDir = document.getElementById("cf-dir");
const $cfSegs = document.getElementById("cf-segs");
const $cfOk = document.getElementById("cf-ok");
const $cfCancel = document.getElementById("cf-cancel");
const $cfPick = document.getElementById("cf-pick");
const $cfClose = document.getElementById("cf-close");
const $cfErr = document.getElementById("cf-err");

// 设置弹窗
const $setModal = document.getElementById("set-modal");
const $setBtn = document.getElementById("settings-btn");
const $setList = document.getElementById("set-list");
const $setAdd = document.getElementById("set-add");
const $setSave = document.getElementById("set-save");

let cats = [], defaultDir = "", lastTasks = [], hadAwaiting = false;
let cfSegments = 8, currentConfirmId = null;
const taskEls = new Map();

// ---- 工具 ----
function fmtSize(b) { if (!b) return "0 B"; const KB=1024,MB=KB*1024,GB=MB*1024; if (b>=GB) return (b/GB).toFixed(2)+" GiB"; if (b>=MB) return (b/MB).toFixed(2)+" MiB"; if (b>=KB) return (b/KB).toFixed(2)+" KiB"; return b+" B"; }
function shortName(p) { const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\")); return i >= 0 ? p.slice(i + 1) : p; }
function escapeHtml(s) { return String(s).replace(/[&<>"']/g, c => ({ "&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;" })[c]); }
function badgeClass(s) { return ({ "待确认":"awaiting","排队中":"queued","下载中":"downloading","已暂停":"paused","已完成":"completed","失败":"failed" })[s] || "queued"; }
function barClass(s) { if (s==="已完成") return "done"; if (s==="失败") return "fail"; if (s==="下载中") return "run"; return ""; }
function pctOf(t) { return t.total > 0 ? Math.min(100, t.downloaded * 100 / t.total) : 0; }
function extOf(n) { const i = n.lastIndexOf("."); return i >= 0 ? n.slice(i + 1).toLowerCase() : ""; }
function joinPath(d, n) { return d.replace(/[\\/]+$/, "") + "\\" + n; }
function inferNameFromUrl(u) { const c = u.split("?")[0]; const i = c.lastIndexOf("/"); const n = i >= 0 ? c.slice(i + 1) : ""; return n || "download.bin"; }
function actBtn(id, act, icon, title) { return `<button class="act${act==="rm"?" danger":""}" data-act="${act}" data-id="${id}" title="${title}" aria-label="${title}"><svg class="ico"><use href="#${icon}"/></svg></button>`; }
function actionsHtml(t) {
  const a = [];
  if (t.state === "待确认") a.push(actBtn(t.id, "confirm", "i-play", "确认"));
  if (t.state === "下载中" || t.state === "排队中") a.push(actBtn(t.id, "pause", "i-pause", "暂停"));
  if (t.state === "已暂停" || t.state === "失败") a.push(actBtn(t.id, "resume", "i-play", "继续"));
  if (t.state === "失败") a.push(actBtn(t.id, "retry", "i-retry", "重试"));
  a.push(actBtn(t.id, "rm", "i-trash", "删除"));
  return a.join("");
}

// ---- 列表渲染 ----
const EMPTY_HTML = `<div class="empty"><svg class="ico"><use href="#i-download"/></svg><p>暂无任务<br/>在上方粘贴链接，点 <b>添加下载</b> 开始</p></div>`;

function render(tasks) {
  lastTasks = tasks;
  if (!tasks.length) { $list.innerHTML = EMPTY_HTML; taskEls.clear(); return; }
  if ($list.querySelector(".empty")) $list.innerHTML = "";
  tasks.sort((a, b) => a.id - b.id);
  const ids = new Set(tasks.map(t => t.id));
  for (const [id, el] of taskEls) if (!ids.has(id)) { el.remove(); taskEls.delete(id); }
  for (const t of tasks) {
    let el = taskEls.get(t.id);
    if (!el) { el = createTaskEl(); taskEls.set(t.id, el); $list.appendChild(el); }
    updateTaskEl(el, t);
  }
}
function createTaskEl() {
  const el = document.createElement("div"); el.className = "task";
  el.innerHTML = `<div class="task-top"><span class="task-name"></span><span class="badge"></span><span class="task-size"></span></div><div class="bar"><i></i></div><div class="task-bottom"><span class="err"></span><span class="pct"></span><div class="actions"></div></div>`;
  return el;
}
function updateTaskEl(el, t) {
  const name = el.querySelector(".task-name"); const file = shortName(t.file);
  if (name.dataset.f !== file) { name.textContent = file; name.title = file; name.dataset.f = file; }
  const badge = el.querySelector(".badge"); const bc = badgeClass(t.state);
  if (!badge.classList.contains(bc) || badge.textContent !== t.state) { badge.className = "badge " + bc; badge.textContent = t.state; }
  const size = el.querySelector(".task-size"); let sizeText;
  if (t.total > 0) sizeText = `${fmtSize(t.downloaded)} <span style="color:var(--faint)">/</span> ${fmtSize(t.total)}`;
  else if (t.downloaded > 0) sizeText = `已下 ${fmtSize(t.downloaded)} <span style="color:var(--faint)">· 大小未知</span>`;
  else sizeText = (t.state === "待确认") ? "等待确认" : "准备中";
  if (size.dataset.s !== sizeText) { size.innerHTML = sizeText; size.dataset.s = sizeText; }
  const pct = el.querySelector(".pct"); const pctText = t.total > 0 ? pctOf(t).toFixed(0) + "%" : "—";
  if (pct.textContent !== pctText) pct.textContent = pctText;
  const errEl = el.querySelector(".err"); const errMsg = t.error || "";
  if (errEl.dataset.e !== errMsg) { errEl.textContent = errMsg; errEl.title = errMsg; errEl.style.display = errMsg ? "" : "none"; errEl.dataset.e = errMsg; }
  if (el.dataset.state !== t.state) { el.querySelector(".actions").innerHTML = actionsHtml(t); el.dataset.state = t.state; }
  updateBar(el.querySelector(".bar"), t);
}
function updateBar(bar, t) {
  const cls = barClass(t.state);
  const wantSegs = t.segments && t.segments.length > 0 && t.total > 0;
  const indet = t.total == 0 && t.state === "下载中";
  const wantMode = indet ? "indet" : (wantSegs ? "seg" : "single");
  const curMode = bar.dataset.mode || "";
  if (curMode !== wantMode || (wantSegs && bar.children.length !== t.segments.length)) {
    bar.innerHTML = wantSegs ? t.segments.map(() => '<span class="cell"><i></i></span>').join("") : "<i></i>";
    bar.dataset.mode = wantMode;
  }
  const wantClass = "bar " + cls + (wantSegs ? " seg" : "") + (indet ? " indet" : "");
  if (bar.className !== wantClass) bar.className = wantClass;
  if (wantSegs) {
    const cells = bar.children;
    t.segments.forEach((s, i) => {
      const cell = cells[i]; const grow = String(s.size || 1);
      if (cell.style.flexGrow !== grow) cell.style.flexGrow = grow;
      const fill = cell.firstChild; const w = (s.size > 0 ? (s.downloaded * 100 / s.size) : 0).toFixed(2) + "%";
      if (fill.style.width !== w) fill.style.width = w;
    });
  } else if (indet) { const fill = bar.firstChild; if (fill.style.width !== "") fill.style.width = ""; }
  else { const fill = bar.firstChild; const w = pctOf(t).toFixed(2) + "%"; if (fill.style.width !== w) fill.style.width = w; }
}
function renderStatus(s) { $statusText.textContent = `运行 ${s.running} · 排队 ${s.queued} · 待确认 ${s.awaiting} · 完成 ${s.completed} · 失败 ${s.failed}`; }

// ---- 确认模态 ----
function setSeg(v) { $cfSegs.querySelectorAll(".seg").forEach(x => x.classList.toggle("active", parseInt(x.dataset.v, 10) === v)); }
function openConfirm(task) {
  currentConfirmId = task.id;
  $cfErr.textContent = "";
  $cfUrl.value = task.url;
  const name = task.file || inferNameFromUrl(task.url);
  $cfName.value = name;
  $cfCat.innerHTML = cats.map(c => `<option value="${c.name}">${c.name}</option>`).join("");
  const m = cats.find(c => c.extensions.some(e => e.toLowerCase() === extOf(name)));
  if (m) $cfCat.value = m.name;
  const cur = cats.find(c => c.name === $cfCat.value);
  $cfDir.value = (cur && cur.dir) ? cur.dir : defaultDir;
  setSeg(cfSegments);
  $cfModal.hidden = false;
}
function closeConfirm() { $cfModal.hidden = true; }

$cfCat.addEventListener("change", () => { const c = cats.find(c => c.name === $cfCat.value); if (c && c.dir) $cfDir.value = c.dir; });
$cfSegs.addEventListener("click", (e) => { const b = e.target.closest(".seg"); if (!b) return; cfSegments = parseInt(b.dataset.v, 10) || 8; setSeg(cfSegments); });
$cfPick.addEventListener("click", async () => { try { const p = await invoke("pick_folder"); if (p) $cfDir.value = p; } catch (e) { $cfErr.textContent = "出错：" + e; } });
$cfOk.addEventListener("click", async () => {
  const dir = $cfDir.value.trim(); const name = $cfName.value.trim() || inferNameFromUrl($cfUrl.value);
  if (!dir) { $cfErr.textContent = "请选择保存目录"; return; }
  try { await invoke("confirm", { id: currentConfirmId, filePath: joinPath(dir, name), segments: cfSegments }); closeConfirm(); currentConfirmId = null; refresh(); }
  catch (e) { $cfErr.textContent = "出错：" + e; }
});
$cfCancel.addEventListener("click", async () => {
  try { await invoke("rm", { id: currentConfirmId, purge: false }); } catch {}
  closeConfirm(); currentConfirmId = null; refresh();
});
$cfClose.addEventListener("click", () => { closeConfirm(); });

// ---- 刷新循环 ----
async function refresh() {
  try {
    const [tasks, s] = await Promise.all([invoke("list"), invoke("status")]);
    render(tasks); renderStatus(s);
    if (currentConfirmId === null) {
      const aw = tasks.find(t => t.state === "待确认");
      if (aw) {
        openConfirm(aw);
        try { await invoke("focus_window"); } catch {}  // 主窗口跳到最前
      }
    }
  } catch (e) { $statusText.textContent = "刷新失败：" + e; }
}

// ---- 内联添加：建待确认任务，下一轮轮询自动弹模态 ----
async function doAdd() {
  const url = $url.value.trim();
  if (!url) { $url.focus(); return; }
  try {
    await invoke("prompt", { url, filePath: "" });
    $url.value = ""; refresh();
  } catch (e) { alert("添加失败：" + e); }
}

// 列表内按钮
$list.addEventListener("click", async (ev) => {
  const btn = ev.target.closest("button[data-act]"); if (!btn) return;
  const id = Number(btn.dataset.id); const act = btn.dataset.act;
  try {
    if (act === "confirm") { const t = lastTasks.find(x => x.id === id); if (t) { openConfirm(t); try { await invoke("focus_window"); } catch {} } return; }
    if (act === "pause") await invoke("pause", { id });
    else if (act === "resume") await invoke("resume", { id });
    else if (act === "retry") await invoke("retry", { id });
    else if (act === "rm") { if (confirm("删除任务？\n点「确定」连已下载的文件一起删除。")) await invoke("rm", { id, purge: true }); }
    refresh();
  } catch (e) { alert("操作失败：" + e); }
});

$add.addEventListener("click", doAdd);
$url.addEventListener("keydown", (e) => { if (e.key === "Enter") doAdd(); });

// ---- 分类设置 ----
$setBtn.addEventListener("click", async () => { try { cats = await invoke("get_categories"); } catch {} renderCatRows(); $setModal.hidden = false; });
function renderCatRows() {
  $setList.innerHTML = cats.map((c, i) => `<div class="cat-row" data-i="${i}"><div class="cat-col"><span class="lbl">名称</span><input class="c-name" value="${escapeHtml(c.name)}"></div><div class="cat-col"><span class="lbl">保存目录</span><div class="cat-add-dir"><input class="c-dir" value="${escapeHtml(c.dir)}"><button class="cat-pick" data-i="${i}">…</button></div></div><div class="cat-col"><span class="lbl">扩展名（空格分隔，留空=通用兜底）</span><input class="c-ext" value="${escapeHtml(c.extensions.join(" "))}"></div><button class="cat-del" data-i="${i}" title="删除">✕</button></div>`).join("");
}
$setList.addEventListener("click", async (e) => {
  const i = e.target.dataset.i;
  if (e.target.classList.contains("cat-pick")) { const p = await invoke("pick_folder"); if (p) $setList.querySelector(`.cat-row[data-i="${i}"] .c-dir`).value = p; }
  else if (e.target.classList.contains("cat-del")) { cats.splice(parseInt(i), 1); renderCatRows(); }
});
$setAdd.addEventListener("click", () => { cats.push({ name: "新分类", dir: defaultDir, extensions: [] }); renderCatRows(); });
$setSave.addEventListener("click", async () => {
  cats = Array.from($setList.querySelectorAll(".cat-row")).map(row => ({ name: row.querySelector(".c-name").value.trim() || "未命名", dir: row.querySelector(".c-dir").value.trim() || defaultDir, extensions: row.querySelector(".c-ext").value.trim().split(/\s+/).filter(Boolean) }));
  try { await invoke("set_categories", { cats }); $setModal.hidden = true; } catch (e) { alert("保存失败：" + e); }
});
document.addEventListener("click", (e) => { if (e.target.matches("[data-close]")) $setModal.hidden = true; if (e.target === $setModal) $setModal.hidden = true; });

// ---- 启动 ----
(async function init() {
  try { cats = await invoke("get_categories"); } catch {}
  try { const cfg = await invoke("get_config"); defaultDir = cfg.default_dir; } catch {}
  refresh();
  setInterval(refresh, 1000);
})();
