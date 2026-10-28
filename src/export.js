/* 导出（DESIGN §2.5、§4.6、§6.5）：记录与每周回顾先在这里摊成 Markdown，改完再写盘。
   文本怎么排由 perch-core 的 export 决定；保存到哪个文件夹由 shell 决定，这里只负责把
   「会存到哪」和「已经存到哪」说清楚。JSON 那份不经过编辑，写的就是数据文件里那份。 */

const ex = {};
for (const id of [
  'exportSheet', 'exportForm', 'exportTitle', 'exportClose', 'exportHint',
  'exportText', 'exportState', 'exportJson', 'exportFolder',
]) {
  ex[id] = document.getElementById(id);
}

const exportUi = { kind: 'notes', name: '', busy: false };

// 成功与普通提示只在颜色上有差别：不弹框，也不把路径缩成一句「已导出」。
function setExportState(text, done) {
  ex.exportState.textContent = text;
  ex.exportState.classList.toggle('is-done', Boolean(done));
}

function exportCopy(kind) {
  return kind === 'review'
    ? {
        title: '导出上周回顾',
        hint: '下面是按事实生成的回顾。导出前可以改字；改完保存，原始数据不受影响。',
      }
    : {
        title: '导出记录',
        hint: '下面是全部记录，按日期倒序。导出前可以改字；改完保存，原始数据不受影响。',
      };
}

async function openExport(kind) {
  if (!ex.exportSheet.hidden || exportUi.busy) return;
  const copy = exportCopy(kind);
  exportUi.kind = kind;
  exportUi.name = '';
  exportUi.busy = true;
  ex.exportTitle.textContent = copy.title;
  ex.exportHint.textContent = copy.hint;
  setExportState('', false);
  ex.exportText.value = '';
  ex.exportSheet.hidden = false;
  ex.exportClose.focus({ preventScroll: true });
  try {
    const preview = await invoke('export_preview', { kind });
    exportUi.name = preview.name;
    ex.exportText.value = preview.text;
    setExportState(`会保存到 ${preview.folder}`, false);
    ex.exportText.focus({ preventScroll: true });
    ex.exportText.setSelectionRange(0, 0);
  } catch (err) {
    setExportState(String(err), false);
  } finally {
    exportUi.busy = false;
  }
}

function closeExport() {
  if (ex.exportSheet.hidden) return;
  ex.exportSheet.hidden = true;
  ex.exportText.value = '';
  exportUi.name = '';
  setExportState('', false);
}

async function saveExport(event) {
  event.preventDefault();
  if (exportUi.busy) return;
  const text = ex.exportText.value;
  if (!text.trim()) {
    setExportState('内容还是空的，先写一点再保存。', false);
    return;
  }
  exportUi.busy = true;
  try {
    const saved = await invoke('export_save', {
      kind: exportUi.kind,
      name: exportUi.name,
      text,
    });
    setExportState(`已保存到 ${saved.path}`, true);
    toast('已导出');
  } catch (err) {
    setExportState(String(err), false);
  } finally {
    exportUi.busy = false;
  }
}

ex.exportForm.addEventListener('submit', saveExport);
ex.exportClose.addEventListener('click', closeExport);

ex.exportJson.addEventListener('click', async () => {
  if (exportUi.busy) return;
  exportUi.busy = true;
  try {
    const saved = await invoke('export_json');
    setExportState(`全部数据已保存到 ${saved.path}`, true);
    toast('全部数据已导出');
  } catch (err) {
    setExportState(String(err), false);
  } finally {
    exportUi.busy = false;
  }
});

ex.exportFolder.addEventListener('click', async () => {
  try {
    await invoke('export_open_folder');
  } catch (err) {
    setExportState(String(err), false);
  }
});

ex.exportSheet.addEventListener('pointerdown', (event) => {
  if (event.target === ex.exportSheet) closeExport();
});

window.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && !ex.exportSheet.hidden) closeExport();
});

document.getElementById('aboutExport').addEventListener('click', async () => {
  try {
    const saved = await invoke('export_json');
    toast(`全部数据已导出：${saved.path}`);
  } catch (err) {
    toast(String(err));
  }
});
