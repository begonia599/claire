// rdm 浏览器接管（service worker, MV3）
//   1. 拦截下载：chrome.downloads.onCreated → 发给 rdm /add → 成功才取消浏览器下载
//      （rm 没开时把下载还给浏览器，不丢下载）
//   2. 右键菜单：链接/视频/音频/图片上「用 rdm 下载」
const RDM_BASE = "http://127.0.0.1:7319";
const DEFAULT_SEGMENTS = 8;

// 我们"还给浏览器"的下载会再次触发 onCreated，用这个集合放行一次，避免死循环
const rearm = new Set();

// ---- 初始化 ----
chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: "rdm-link",
    title: "用 rdm 下载",
    contexts: ["link", "video", "audio", "image"],
  });
  chrome.storage.local.get(["enabled", "confirm", "segments"], (r) => {
    if (r.enabled === undefined) chrome.storage.local.set({ enabled: true });
    if (r.confirm === undefined) chrome.storage.local.set({ confirm: true });
    if (!r.segments) chrome.storage.local.set({ segments: DEFAULT_SEGMENTS });
  });
});

// ---- 拦截下载 ----
chrome.downloads.onCreated.addListener(async (item) => {
  // 自己重启的下载放行
  if (rearm.delete(item.url)) return;

  const { enabled, confirm, segments } = await getSettings();
  if (!enabled) return;

  const url = item.url;
  // 只接管 http/https；blob/data/ftp 交给浏览器自己处理
  if (!url || !/^https?:/i.test(url)) return;

  // 立刻停掉浏览器下载，避免和 rdm 重复下
  chrome.downloads.cancel(item.id, () => chrome.downloads.erase({ id: item.id }));

  const filename = item.filename ? basename(item.filename) : "";
  // 确认开 → /prompt（GUI 弹窗确认）；关 → /add（立即下）
  const ok = await sendToRdm(url, filename, segments, confirm);

  if (!ok) {
    // rdm 不可用：把下载还给浏览器（放行这一次，避免再次拦截）
    rearm.add(url);
    chrome.downloads.download({ url, filename: item.filename || undefined });
    notify("rdm 未运行", "已把下载还给浏览器。启动 rdm 即可接管。");
  }
});

// ---- 右键菜单 ----
chrome.contextMenus.onClicked.addListener(async (info) => {
  if (info.menuItemId !== "rdm-link") return;
  const url = info.linkUrl || info.srcUrl;
  if (!url) return;
  const { confirm, segments } = await getSettings();
  await sendToRdm(url, "", segments, confirm);
});

// ---- 与 rdm 通信 ----
// confirm=true 发 /prompt（待确认，GUI 弹窗）；false 发 /add（立即下）
async function sendToRdm(url, filename, segments, confirm) {
  const endpoint = confirm ? "/prompt" : "/add";
  const body = confirm
    ? { url, file_path: filename }
    : { url, file_path: filename, segments };
  try {
    const resp = await fetch(`${RDM_BASE}${endpoint}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!resp.ok) return false;
    const data = await resp.json();
    if (data.ok) {
      notify(confirm ? "已转交 rdm 确认" : "已接管到 rdm", `任务 #${data.id}：${filename || shortUrl(url)}`);
      return true;
    }
    notify("rdm 添加失败", data.error || "未知错误");
    return false;
  } catch (e) {
    return false;
  }
}

// ---- 工具 ----
function basename(p) {
  const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return i >= 0 ? p.slice(i + 1) : p;
}
function shortUrl(u) {
  try { const x = new URL(u); return x.pathname.split("/").pop() || x.host; }
  catch { return u.length > 40 ? u.slice(0, 40) + "…" : u; }
}
function getSettings() {
  return new Promise((res) =>
    chrome.storage.local.get(["enabled", "confirm", "segments"], (r) =>
      res({ enabled: r.enabled !== false, confirm: r.confirm !== false, segments: r.segments || DEFAULT_SEGMENTS })
    )
  );
}
function notify(title, message) {
  chrome.notifications.create({
    type: "basic",
    iconUrl: chrome.runtime.getURL("icons/128.png"),
    title,
    message,
    priority: 0,
  });
}
