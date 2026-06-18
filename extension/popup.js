// popup：开关 + 默认分段 + rdm 连接状态
const $enabled = document.getElementById("enabled");
const $confirm = document.getElementById("confirm");
const $status = document.getElementById("status");
const $dot = document.getElementById("dot");
const $segs = document.getElementById("segs");

const RDM = "http://127.0.0.1:7319";

// 读取设置
chrome.storage.local.get(["enabled", "confirm", "segments"], (r) => {
  $enabled.checked = r.enabled !== false;
  $confirm.checked = r.confirm !== false; // 默认开
  setSeg(r.segments || 8);
});

$enabled.addEventListener("change", () => chrome.storage.local.set({ enabled: $enabled.checked }));
$confirm.addEventListener("change", () => chrome.storage.local.set({ confirm: $confirm.checked }));

$segs.addEventListener("click", (e) => {
  const b = e.target.closest(".seg");
  if (!b) return;
  $segs.querySelectorAll(".seg").forEach(x => x.classList.remove("active"));
  b.classList.add("active");
  chrome.storage.local.set({ segments: parseInt(b.dataset.v, 10) || 8 });
});

function setSeg(v) {
  $segs.querySelectorAll(".seg").forEach(x => {
    x.classList.toggle("active", parseInt(x.dataset.v, 10) === v);
  });
}

// 探测 rdm 状态
async function poll() {
  try {
    const s = await (await fetch(`${RDM}/status`)).json();
    $dot.className = "dot on";
    $status.textContent = `运行 ${s.running} · 排队 ${s.queued} · 完成 ${s.completed} · 失败 ${s.failed}`;
  } catch {
    $dot.className = "dot off";
    $status.textContent = "未连接：请启动 rdm GUI 或 rdm serve";
  }
}
poll();
setInterval(poll, 2000);
