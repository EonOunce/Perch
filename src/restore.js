/* 恢复（DESIGN §4.7）：选一份备份或导出的 JSON → 看清里面有什么 → 再确认一次。
   替换前 store 会先把现在这份留成备份，所以这一步本身也能回头。
   只读模式下别的写入口都收起了，恢复是唯一还能走的写路径——文件坏了，正是要从备份换回来。 */

const rs = {};
for (const id of [
  'restoreSheet', 'restoreClose', 'restoreTabs',
  'restoreBackupPane', 'restoreFilePane', 'restoreList', 'restorePick',
  'restorePreview', 'restoreSource', 'restoreCounts', 'restoreRange',
  'restoreState', 'restoreAsk', 'restoreThink', 'restoreGo',
  'restoreFoot', 'restoreCancel', 'restoreNext',
  'bannerRestore', 'aboutRestoreBackup', 'aboutRestoreFile',
]) {
  rs[id] = document.getElementById(id);
}

const restoreUi = { source: 'backup', preview: null, busy: false, opening: false };

function setRestoreState(text) {
  rs.restoreState.textContent = text;
  rs.restoreState.hidden = !text;
}

function showRestoreAsk(show) {
  rs.restoreAsk.hidden = !show;
  rs.restoreFoot.hidden = show;
  rs.restoreGo.disabled = false;
}

function clearPreview() {
  restoreUi.preview = null;
  rs.restorePreview.hidden = true;
  rs.restoreSource.textContent = '';
  rs.restoreCounts.textContent = '';
  rs.restoreRange.textContent = '';
  rs.restoreNext.disabled = true;
  showRestoreAsk(false);
  for (const row of rs.restoreList.querySelectorAll('.is-on')) row.classList.remove('is-on');
}

function paintBackups(list) {
  if (!list.length) {
    rs.restoreList.innerHTML = '<p class="restore-empty">还没有备份。栖迟每次保存前会自动留一份。</p>';
    return;
  }
  rs.restoreList.innerHTML = list.map((row) => `<button class="restore-row" type="button"
      data-name="${esc(row.name)}"${row.readable ? '' : ' disabled'}>
      <span class="restore-when">${esc(row.label)}</span>
      <span class="restore-meta">${row.readable
        ? `${row.habits} 个习惯 · ${row.notes} 条记录`
        : '读不出来'}</span>
    </button>`).join('');
}

async function loadBackups() {
  rs.restoreList.innerHTML = '<p class="restore-empty">正在读备份…</p>';
  try {
    paintBackups(await invoke('restore_backups'));
  } catch (err) {
    rs.restoreList.innerHTML = '';
    setRestoreState(String(err));
  }
}

function previewCounts(view) {
  const parts = [];
  if (view.habits) {
    const archived = view.archivedHabits ? `（${view.archivedHabits} 个已归档）` : '';
    parts.push(`${view.habits} 个习惯${archived}`);
  }
  if (view.checkins) parts.push(`${view.checkins} 次打卡`);
  if (view.notes) parts.push(`${view.notes} 条记录`);
  if (view.moods) parts.push(`${view.moods} 天心情`);
  if (view.markers) parts.push(`${view.markers} 个标记`);
  return parts.join(' · ');
}

// `from` 是备份列表上那行的时间；选文件时没有，就只报文件名。
function paintPreview(view, from) {
  restoreUi.preview = view;
  rs.restoreSource.textContent = view.sourceKind === 'backup' && from
    ? `来源：${from} 的备份`
    : `来源：${view.origin}`;
  rs.restoreCounts.textContent = previewCounts(view) || '这份里没有任何数据。';
  rs.restoreRange.textContent = view.firstDay
    ? (view.firstDay === view.lastDay
        ? `记录都在 ${view.firstDay} 这一天。`
        : `记录从 ${view.firstDay} 到 ${view.lastDay}。`)
    : '';
  rs.restorePreview.hidden = false;
  rs.restoreNext.disabled = false;
  showRestoreAsk(false);
  setRestoreState('');
}

async function chooseBackup(row) {
  if (restoreUi.busy) return;
  restoreUi.busy = true;
  for (const other of rs.restoreList.querySelectorAll('.is-on')) other.classList.remove('is-on');
  row.classList.add('is-on');
  try {
    const view = await invoke('restore_preview_backup', { name: row.dataset.name });
    paintPreview(view, row.querySelector('.restore-when').textContent);
  } catch (err) {
    clearPreview();
    setRestoreState(String(err));
  } finally {
    restoreUi.busy = false;
  }
}

async function pickFile() {
  if (restoreUi.busy) return;
  restoreUi.busy = true;
  rs.restorePick.disabled = true;
  // 后端一开始就会丢掉上一次的暂存，所以取消选择后不能留着旧预览。
  clearPreview();
  try {
    const view = await invoke('restore_pick_file');
    if (view) paintPreview(view, '');
  } catch (err) {
    setRestoreState(String(err));
  } finally {
    rs.restorePick.disabled = false;
    restoreUi.busy = false;
  }
}

function resetRestore(source) {
  restoreUi.source = source;
  for (const button of rs.restoreTabs.children) {
    const on = button.dataset.source === source;
    button.classList.toggle('is-on', on);
    button.setAttribute('aria-selected', String(on));
  }
  rs.restoreBackupPane.hidden = source !== 'backup';
  rs.restoreFilePane.hidden = source !== 'file';
  rs.restoreList.innerHTML = '';
  clearPreview();
  setRestoreState('');
}

async function openRestore(source) {
  if (restoreUi.opening || !rs.restoreSheet.hidden) return;
  restoreUi.opening = true;
  try {
    // 从「关于栖迟」里点进来时两张面板会叠在一起，先收起下面那张（§2.10）。
    closeAbout();
    resetRestore(source);
    rs.restoreSheet.hidden = false;
    rs.restoreClose.focus({ preventScroll: true });
    if (source === 'backup') await loadBackups();
    else await pickFile();
  } finally {
    restoreUi.opening = false;
  }
}

function closeRestore() {
  if (rs.restoreSheet.hidden || restoreUi.busy) return;
  rs.restoreSheet.hidden = true;
  invoke('restore_cancel').catch(() => {});
  resetRestore('backup');
}

async function applyRestore() {
  if (!restoreUi.preview || restoreUi.busy) return;
  restoreUi.busy = true;
  rs.restoreGo.disabled = true;
  try {
    const applied = await invoke('restore_apply');
    rs.restoreSheet.hidden = true;
    resetRestore('backup');
    toast(`已恢复：${applied.habits} 个习惯，${applied.notes} 条记录`);
    // 今天页、记录页与统计页各有一份快照，换完数据都要重取；管理面板开着才跟着刷。
    await refresh();
    syncRecords();
    syncStats();
    if (panel.isOpen) syncPanel();
  } catch (err) {
    rs.restoreGo.disabled = false;
    showRestoreAsk(false);
    setRestoreState(String(err));
  } finally {
    restoreUi.busy = false;
  }
}

rs.restoreTabs.addEventListener('click', (event) => {
  const button = event.target.closest('button');
  if (!button || restoreUi.busy || button.dataset.source === restoreUi.source) return;
  invoke('restore_cancel').catch(() => {});
  resetRestore(button.dataset.source);
  if (button.dataset.source === 'backup') loadBackups();
  else rs.restorePick.focus({ preventScroll: true });
});

rs.restoreList.addEventListener('click', (event) => {
  const row = event.target.closest('.restore-row');
  if (row && !row.disabled) chooseBackup(row);
});

rs.restorePick.addEventListener('click', pickFile);

rs.restoreNext.addEventListener('click', () => {
  if (!restoreUi.preview || restoreUi.busy) return;
  showRestoreAsk(true);
  rs.restoreGo.focus({ preventScroll: true });
});

rs.restoreThink.addEventListener('click', () => {
  showRestoreAsk(false);
  rs.restoreNext.focus({ preventScroll: true });
});

rs.restoreGo.addEventListener('click', applyRestore);
rs.restoreCancel.addEventListener('click', closeRestore);
rs.restoreClose.addEventListener('click', closeRestore);

rs.restoreSheet.addEventListener('pointerdown', (event) => {
  if (event.target === rs.restoreSheet) closeRestore();
});

window.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && !rs.restoreSheet.hidden) closeRestore();
});

rs.bannerRestore.addEventListener('click', () => openRestore('backup'));
rs.aboutRestoreBackup.addEventListener('click', () => openRestore('backup'));
rs.aboutRestoreFile.addEventListener('click', () => openRestore('file'));
