/* 同步设置与状态（DESIGN §4.1、§4.3、§4.7、§5.3）。
   口径都在 Rust 侧：这里只把 `sync_settings` / `sync_status` 的原样字段放进 DOM，
   不自己判断「算不算配好」，也不把地址规矩再抄一份。
   同步内容是明文（不变项 #1）：那张表单顶部固定写清楚，不弹第二次确认。 */

const sy = {};
for (const id of [
  'syncSheet', 'syncForm', 'syncClose', 'syncState',
  'syncAuto', 'syncAccount', 'syncPassword', 'syncPasswordHint',
  'syncEndpoint', 'syncForget', 'syncRun',
]) {
  sy[id] = document.getElementById(id);
}

const syncUi = { mode: 'off', view: null, opening: false, forgetTimer: 0 };

// 只用于显示：后端给的同步时刻是带时区的 ISO 串。
const syncWhen = (iso) => {
  const day = ui.view ? relDay(iso, ui.view.today) : fmtDay(iso);
  return `${day} ${iso.slice(11, 16)}`;
};

function syncStatusText(view) {
  if (view.running) return '正在同步…';
  if (view.lastError) return view.lastError;
  // 没配好时「还差什么」比排队状态要紧：清掉口令后那次排队其实什么都不会做。
  if (!view.hasPassword) return '还差应用专用密码。填好保存后就能同步。';
  if (!view.configured) return '还差坚果云账号或地址。填好保存后就能同步。';
  // 启动、回到卡片后都会再排一轮：已经同步过就说清「还会再对一次」，
  // 别让「还有一轮在排队」看起来像上一轮一直没跑完。
  if (view.queued) {
    return view.lastSyncedAt
      ? `上次同步：${syncWhen(view.lastSyncedAt)}。稍后还会再对一次。`
      : '还有一轮在排队。';
  }
  if (view.autoPaused) return '自动同步暂停了。检查账号和密码，保存后继续。';
  if (view.lastNote) return view.lastNote;
  if (view.lastSyncedAt) return `上次同步：${syncWhen(view.lastSyncedAt)}。`;
  return '还没同步过。保存后可以点「立即同步」，或者打开自动同步。';
}

function paintSync(view) {
  syncUi.view = view;
  sy.syncState.textContent = syncStatusText(view);
  sy.syncForget.hidden = !view.hasPassword;
  sy.syncPasswordHint.hidden = !view.hasPassword;
  sy.syncPassword.placeholder = view.hasPassword ? '已保存，留空不改' : '不是登录密码';
}

async function loadSync() {
  const [settings, view] = await Promise.all([
    invoke('sync_settings'),
    invoke('sync_status'),
  ]);
  sy.syncAccount.value = settings.account;
  sy.syncEndpoint.value = settings.endpoint;
  sy.syncPassword.value = '';
  syncUi.mode = settings.enabled ? 'on' : 'off';
  setSeg(sy.syncAuto, syncUi.mode);
  paintSync(view);
}

async function openSync() {
  if (syncUi.opening || !sy.syncSheet.hidden) return;
  syncUi.opening = true;
  try {
    // 从「关于栖迟」里点进来时两张面板会叠在一起，先收起下面那张（§2.10）。
    closeAbout();
    await loadSync();
    sy.syncSheet.hidden = false;
    sy.syncAccount.focus({ preventScroll: true });
  } catch (err) {
    toast(String(err));
  } finally {
    syncUi.opening = false;
  }
}

function closeSync() {
  if (sy.syncSheet.hidden) return;
  sy.syncSheet.hidden = true;
  sy.syncPassword.value = '';
  syncUi.forgetTimer = 0;
  sy.syncForget.textContent = '清除口令';
}

// 保存表单原样交给 Rust：地址规矩与「留空不改口令」都由 backend 决定（§4.2）。
async function saveSync() {
  try {
    const view = await invoke('sync_configure', {
      account: sy.syncAccount.value.trim(),
      endpoint: sy.syncEndpoint.value.trim(),
      enabled: syncUi.mode === 'on',
      password: sy.syncPassword.value || null,
    });
    sy.syncPassword.value = '';
    paintSync(view);
    return true;
  } catch (err) {
    sy.syncState.textContent = String(err);
    return false;
  }
}

async function syncNow() {
  if (!(await saveSync())) return;
  if (!syncUi.view.configured) {
    sy.syncState.textContent = syncStatusText(syncUi.view);
    return;
  }
  sy.syncState.textContent = '正在同步…';
  try {
    paintSync(await invoke('sync_now'));
  } catch (err) {
    sy.syncState.textContent = String(err);
  }
}

sy.syncForm.addEventListener('submit', async (event) => {
  event.preventDefault();
  if (await saveSync()) toast('同步设置已保存');
});

sy.syncRun.addEventListener('click', syncNow);
sy.syncClose.addEventListener('click', closeSync);

sy.syncAuto.addEventListener('click', (event) => {
  const button = event.target.closest('button');
  if (!button) return;
  syncUi.mode = button.dataset.value;
  setSeg(sy.syncAuto, syncUi.mode);
});

// 清口令是破坏性操作：第一次只把按钮换成问句，四秒内再点才算数（§2.9）。
sy.syncForget.addEventListener('click', async () => {
  if (!syncUi.forgetTimer) {
    sy.syncForget.textContent = '确定清除？';
    syncUi.forgetTimer = setTimeout(() => {
      syncUi.forgetTimer = 0;
      sy.syncForget.textContent = '清除口令';
    }, 4000);
    return;
  }
  clearTimeout(syncUi.forgetTimer);
  syncUi.forgetTimer = 0;
  sy.syncForget.textContent = '清除口令';
  try {
    paintSync(await invoke('sync_configure', { password: '' }));
    toast('这台电脑上的口令已清除');
  } catch (err) {
    sy.syncState.textContent = String(err);
  }
});

// 点遮罩走开；点面板里面不算。
sy.syncSheet.addEventListener('pointerdown', (event) => {
  if (event.target === sy.syncSheet) closeSync();
});

window.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && !sy.syncSheet.hidden) closeSync();
});

// 关于面板里那个入口：脚本加载顺序保证 closeAbout 已经在了（about.js 先加载）。
document.getElementById('aboutSync').addEventListener('click', openSync);

window.__TAURI__.event.listen('perch:sync-settings', openSync);

// 每轮同步结束 Rust 都会发一份新状态；设置面板开着就当场更新，关着就不用管。
window.__TAURI__.event.listen('perch:sync', (event) => {
  if (sy.syncSheet.hidden) return;
  paintSync(event.payload);
});
