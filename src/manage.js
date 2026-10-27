/* 习惯管理面板：改元数据、改目标、归档与恢复、移除、统计开关。
   面板上每一个显示值都来自 `manage_state`——目标今天算哪一条、归档后还进不进统计，
   都在 Rust 侧判完（DESIGN §5.4、§6.2），这里只把字符串摆进 DOM。
   表单是共用的一张，由 app.js 的 `openSheet(row)` 打开；改目标另走 `habit_goal`。 */

const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);

const TIMING_TEXT = { sameDay: '当天打卡', nextDay: '次日确认' };

const panel = {
  root: document.getElementById('panel'),
  list: document.getElementById('panelList'),
  state: null,
  isOpen: false,
  confirming: null,
};

/* ---------- 显示用的拼接 ---------- */

const unitTail = (row) => (row.unit ? ` ${row.unit}` : '');

const targetText = (row) =>
  row.targetMode === 'recordOnly'
    ? '只记录'
    : `每周 ${row.weeklyTarget}${unitTail(row)}`;

const metaText = (row) => `${TIMING_TEXT[row.confirmationTiming]} · ${targetText(row)}`;

// 待生效的目标单独一行：只提示，不改当前显示（§2.4）。
const pendingText = (goal, row) =>
  `${fmtDay(goal.effectiveFrom)} 起 ${
    goal.targetMode === 'recordOnly' ? '只记录' : `每周 ${goal.weeklyTarget}${unitTail(row)}`
  }`;

const archiveText = (row) => `${fmtDay(row.archivedAt)} 归档 · 只读`;

const styleVars = (row) => {
  const [accent, wash] = paletteOf(row.color);
  return `--habit-accent:${esc(accent)};--habit-wash:${esc(wash)}`;
};

const actsHtml = (row) => `<span class="mrow-acts">
    <button class="icon-btn" type="button" data-edit="${esc(row.id)}"
            aria-label="修改「${esc(row.name)}」">${icon('edit')}</button>
    <button class="icon-btn" type="button" data-archive="${esc(row.id)}"
            aria-label="归档「${esc(row.name)}」">${icon('archive')}</button>
    <button class="icon-btn is-danger" type="button" data-remove="${esc(row.id)}"
            aria-label="移除「${esc(row.name)}」">${icon('trash')}</button>
  </span>`;

// 移除的第一次点击只把这一行换成就地确认，不弹框、不挡别的行（§2.4、§2.9）。
const confirmHtml = (row) => `<div class="mrow is-confirm">
    <p class="mrow-ask">移除「${esc(row.name)}」？它和它的打卡会从所有地方消失，无法恢复。</p>
    <span class="mrow-acts">
      <button class="text-btn" type="button" data-cancel="1">取消</button>
      <button class="mini-save" type="button" data-remove="${esc(row.id)}">移除</button>
    </span>
  </div>`;

const rowHtml = (row) => {
  if (panel.confirming === row.id) return confirmHtml(row);
  const pending = row.pendingGoals.map((g) => `<p class="mrow-note">${esc(pendingText(g, row))}</p>`).join('');
  return `<div class="mrow" style="${styleVars(row)}">
    <span class="mrow-icon">${icon(esc(row.icon))}</span>
    <div class="mrow-main">
      <p class="mrow-name">${esc(row.name)}</p>
      <p class="mrow-meta">${esc(metaText(row))}</p>
      ${pending}
    </div>
    ${actsHtml(row)}
  </div>`;
};

const archivedHtml = (row) => `<div class="mrow is-archived" style="${styleVars(row)}">
    <span class="mrow-icon">${icon(esc(row.icon))}</span>
    <div class="mrow-main">
      <p class="mrow-name">${esc(row.name)}</p>
      <p class="mrow-meta">${esc(archiveText(row))}</p>
    </div>
    <label class="mini-switch">
      <input type="checkbox" data-stats="${esc(row.id)}" ${row.statsVisible ? 'checked' : ''}>
      <span>统计</span>
    </label>
    <button class="icon-btn" type="button" data-restore="${esc(row.id)}"
            aria-label="恢复「${esc(row.name)}」">${icon('restore')}</button>
  </div>`;

function paintPanel(state) {
  panel.state = state;
  if (!state) {
    panel.list.innerHTML = '';
    return;
  }
  if (!state.active.length && !state.archived.length) {
    panel.list.innerHTML = '<p class="panel-empty">还没有习惯。</p>';
    return;
  }
  const archived = state.archived.length
    ? `<p class="panel-sub">已归档</p>${state.archived.map(archivedHtml).join('')}`
    : '';
  panel.list.innerHTML = state.active.map(rowHtml).join('') + archived;
}

/* ---------- 读写 ---------- */

async function syncPanel() {
  try {
    paintPanel(await invoke('manage_state'));
  } catch (err) {
    toast(String(err));
  }
}

// 面板里的写入返回的是那份面板快照；卡片本身另外刷新，归档与移除会立刻从今天视图走开。
async function manageWrite(command, args) {
  if (ui.busy) return false;
  if (panel.state && panel.state.storageError) {
    toast(panel.state.storageError);
    return false;
  }
  ui.busy = true;
  try {
    paintPanel(await invoke(command, args));
    panel.confirming = null;
    await refresh();
    return true;
  } catch (err) {
    toast(String(err));
    await Promise.all([syncPanel(), refresh()]);
    return false;
  } finally {
    ui.busy = false;
  }
}

/* ---------- 开关 ---------- */

function openPanel() {
  panel.root.hidden = false;
  panel.isOpen = true;
  panel.confirming = null;
  syncPanel();
}

function closePanel() {
  panel.root.hidden = true;
  panel.isOpen = false;
  panel.confirming = null;
}

function findRow(id) {
  if (!panel.state) return null;
  return (
    panel.state.active.find((r) => r.id === id) ||
    panel.state.archived.find((r) => r.id === id) ||
    null
  );
}

const rowName = (id) => {
  const row = findRow(id);
  return row ? row.name : '';
};

/* ---------- 事件 ---------- */

document.getElementById('openManage').addEventListener('click', openPanel);
document.getElementById('panelClose').addEventListener('click', closePanel);
document.getElementById('panelAdd').addEventListener('click', () => openSheet(null));

panel.list.addEventListener('click', (event) => {
  const hit = (name) => event.target.closest(`[data-${name}]`);

  if (hit('cancel')) {
    panel.confirming = null;
    paintPanel(panel.state);
    return;
  }
  const edit = hit('edit');
  if (edit) {
    const row = findRow(edit.dataset.edit);
    if (row) openSheet(row);
    return;
  }
  const archive = hit('archive');
  if (archive) {
    const id = archive.dataset.archive;
    manageWrite('habit_archive', { habitId: id, archived: true })
      .then((ok) => ok && toast(`已归档「${rowName(id)}」`));
    return;
  }
  const restore = hit('restore');
  if (restore) {
    const id = restore.dataset.restore;
    manageWrite('habit_archive', { habitId: id, archived: false })
      .then((ok) => ok && toast(`已恢复「${rowName(id)}」`));
    return;
  }
  const remove = hit('remove');
  if (remove) {
    const id = remove.dataset.remove;
    // 第一次点只进入就地确认，第二次才真的写墓碑。
    if (panel.confirming !== id) {
      panel.confirming = id;
      paintPanel(panel.state);
      return;
    }
    const name = rowName(id);
    manageWrite('habit_remove', { habitId: id })
      .then((ok) => ok && toast(`已移除「${name}」`));
  }
});

// 「统计里显示」是个开关，点下去就是一次写入。
panel.list.addEventListener('change', (event) => {
  const box = event.target.closest('[data-stats]');
  if (!box) return;
  manageWrite('habit_stats', { habitId: box.dataset.stats, visible: box.checked });
});

// 今天视图里那张卡的铅笔：直达这个习惯的修改表单，不必先打开面板（§2.4）。
el.habitList.addEventListener('click', async (event) => {
  const pencil = event.target.closest('[data-edit]');
  if (!pencil) return;
  try {
    const state = await invoke('manage_state');
    const row = state.active.find((r) => r.id === pencil.dataset.edit);
    if (row) openSheet(row);
  } catch (err) {
    toast(String(err));
  }
});

document.addEventListener('keydown', (event) => {
  // 表单盖在面板上面时，Esc 先关表单（由 app.js 负责），这里不抢。
  if (event.key === 'Escape' && panel.isOpen && el.sheet.hidden) closePanel();
});
