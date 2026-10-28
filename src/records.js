/* 记录视图：时间线、搜索、标签筛选、编辑与删除（删除后留 10 秒撤销）。
   分组键（localDate）、匹配规则、标签计数都只在 perch-core 的 records 里算一次（§2.5），
   这里把返回的快照摆进 DOM，不再判一遍。 */

const rec = {
  list: document.getElementById('recList'),
  scroll: document.getElementById('recScroll'),
  tags: document.getElementById('tagRow'),
  count: document.getElementById('recCount'),
  clear: document.getElementById('recClear'),
  search: document.getElementById('recSearch'),
  undoBar: document.getElementById('recUndo'),
  undoBtn: document.getElementById('recUndoBtn'),
  sheet: document.getElementById('noteSheet'),
  form: document.getElementById('noteForm'),
  title: document.getElementById('noteSheetTitle'),
  text: document.getElementById('fNoteText'),
  preview: document.getElementById('noteTagPreview'),
  save: document.getElementById('noteSheetSave'),
  state: null,
  query: '',
  tag: '',
  confirming: null,
  editing: null,
  undoId: null,
  undoTimer: 0,
};

// 撤销窗口按 §2.9 给到 10 秒，比一般 toast 长：手滑之后还能回头。
const UNDO_MS = 10000;

const hasFilter = () => rec.query !== '' || rec.tag !== '';

const findNote = (id) => {
  if (!rec.state) return null;
  for (const group of rec.state.days) {
    const hit = group.notes.find((n) => n.id === id);
    if (hit) return hit;
  }
  return null;
};

/* ---------- 渲染 ---------- */

const dayHeading = (localDate, today) =>
  `${relDay(localDate, today)} · ${weekdayOf(localDate)}`;

/* 命中区间由 Rust 给（字符下标），这里只负责包一层 <mark>：按码点切，
   不会把 emoji 的代理对切坏，也不在 JS 里再判一次大小写。 */
function markHits(text, hits) {
  if (!hits || !hits.length) return esc(text);
  const chars = [...text];
  let html = '';
  let cursor = 0;
  for (const [start, end] of hits) {
    if (start < cursor || end <= start || end > chars.length) continue;
    html += esc(chars.slice(cursor, start).join(''));
    html += `<mark>${esc(chars.slice(start, end).join(''))}</mark>`;
    cursor = end;
  }
  return html + esc(chars.slice(cursor).join(''));
}

const noteTagsHtml = (tags, hits) => (tags.length
  ? `<div class="note-tags">${tags.map((tag, index) => `<span class="chip${hits && hits[index] ? ' is-hit' : ''}">#${esc(tag)}</span>`).join('')}</div>`
  : '');

const noteRowHtml = (note) => `<div class="rec-item">
    <div class="rec-head">
      <span class="rec-time">${esc(note.timeOfDay)}</span>
      ${note.conflict ? '<span class="rec-copy" title="同步冲突副本">副本</span>' : ''}
      <span class="rec-acts">
        <button class="tool-btn" type="button" data-edit="${esc(note.id)}"
                aria-label="修改这条记录">${icon('edit')}</button>
        <button class="tool-btn" type="button" data-remove="${esc(note.id)}"
                aria-label="移除这条记录">${icon('trash')}</button>
      </span>
    </div>
    <p class="rec-text">${markHits(note.text, note.textHits)}</p>
    ${noteTagsHtml(note.tags, note.tagHits)}
  </div>`;

/* 「以前的一天」只是一段旧记录，不是待办：没有按钮，筛选时也不出现。
   年份不同的日期把年份写出来，避免「8 月 12 日」分不清是哪一年。 */
const memoryDate = (localDate, today) => {
  const [year, month, day] = parts(localDate);
  const sameYear = year === parts(today)[0];
  return `${sameYear ? '' : `${year}年`}${month}月${day}日 · ${weekdayOf(localDate)}`;
};

const memoryCardHtml = (memory, today) => `<section class="rec-memory" aria-label="以前的一天">
    <p class="rec-memory-head"><span>以前的一天${memory.conflict ? '<span class="rec-copy" title="同步冲突副本">副本</span>' : ''}</span><span class="rec-memory-date">${esc(memoryDate(memory.localDate, today))}</span></p>
    <p class="rec-memory-text">${esc(memory.text)}</p>
    ${noteTagsHtml(memory.tags)}
  </section>`;

// 移除的第一次点击只把这一条换成就地确认，不弹框、不挡别条（§2.5、§2.9）。
const noteConfirmHtml = (note) => `<div class="rec-item is-confirm">
    <p class="rec-ask">移除这条记录？可在 10 秒内撤销。</p>
    <span class="rec-acts">
      <button class="text-btn" type="button" data-cancel="1">取消</button>
      <button class="mini-save" type="button" data-remove="${esc(note.id)}">移除</button>
    </span>
  </div>`;

// 标签放不下时右缘淡出，让人知道还能横滑（§13）。
// 换标签与换尺寸都会改变可用宽度，所以淡出状态要跟着重算，不能只在渲染时判一次。
function clipTags() {
  rec.tags.classList.toggle('is-clipped', rec.tags.scrollWidth > rec.tags.clientWidth + 1);
}

function paintTags(list) {
  rec.tags.innerHTML = list
    .map((item) => `<button class="filter-chip${rec.tag === item.tag ? ' is-on' : ''}" type="button"
             data-tag="${esc(item.tag)}" aria-pressed="${rec.tag === item.tag}">#${esc(item.tag)}<span class="count">${item.count}</span></button>`)
    .join('');
  clipTags();
}

function paintCount(state) {
  rec.clear.hidden = !hasFilter();
  if (state.total === 0) {
    rec.count.textContent = '还没有记录。想到什么再写。';
    return;
  }
  rec.count.textContent = hasFilter()
    ? `${state.shown} / ${state.total} 条`
    : `全部记录 · ${state.total} 条`;
}

function paintRecords(state) {
  rec.state = state;
  showStorageError(state.storageError || '');
  paintTags(state.tags);
  paintCount(state);
  if (!state.days.length) {
    rec.list.innerHTML = `<p class="note-empty">${hasFilter()
      ? '没有符合的记录。换个词，或者清掉筛选。'
      : '还没有记录。想到什么再写。'}</p>`;
    return;
  }
  const memory = !hasFilter() && state.memory ? memoryCardHtml(state.memory, state.today) : '';
  rec.list.innerHTML = memory + state.days.map((group) => `<section class="rec-day">
      <h2 class="rec-day-head"><span>${esc(dayHeading(group.localDate, state.today))}</span><span class="rec-day-n">${group.notes.length} 条</span></h2>
      ${group.notes.map((note) => (rec.confirming === note.id ? noteConfirmHtml(note) : noteRowHtml(note))).join('')}
    </section>`).join('');
}

/* ---------- 读写 ---------- */

async function syncRecords() {
  try {
    paintRecords(await invoke('records_state', { query: rec.query, tag: rec.tag }));
  } catch (err) {
    toast(String(err));
  }
}

views.hooks.records = syncRecords;

// 记录里的写入返回的是那份时间线快照；今天视图另外刷新，两边不会各留一份旧数据。
async function recordsWrite(command, args) {
  if (ui.busy) return false;
  if (rec.state && rec.state.storageError) {
    toast(rec.state.storageError);
    return false;
  }
  ui.busy = true;
  try {
    paintRecords(await invoke(command, {
      ...args, query: rec.query, tag: rec.tag,
    }));
    rec.confirming = null;
    await refresh();
    return true;
  } catch (err) {
    toast(String(err));
    await Promise.all([syncRecords(), refresh()]);
    return false;
  } finally {
    ui.busy = false;
  }
}

/* ---------- 撤销 ---------- */

function openUndo(id) {
  rec.undoId = id;
  rec.undoBar.hidden = false;
  clearTimeout(rec.undoTimer);
  rec.undoTimer = setTimeout(closeUndo, UNDO_MS);
}

function closeUndo() {
  clearTimeout(rec.undoTimer);
  rec.undoId = null;
  rec.undoBar.hidden = true;
}

/* ---------- 那张表单（记一条与编辑共用） ---------- */

function openNoteSheet(note) {
  rec.editing = note ? { id: note.id } : null;
  rec.title.textContent = note ? '编辑记录' : '记一条';
  rec.text.value = note ? note.text : '';
  previewTags(rec.text.value, rec.preview);
  rec.save.disabled = rec.text.value.trim() === '';
  rec.sheet.hidden = false;
  rec.text.focus();
}

function closeNoteSheet() {
  rec.sheet.hidden = true;
  rec.editing = null;
  rec.text.value = '';
  previewTags('', rec.preview);
}

function syncNotePreview() {
  previewTags(rec.text.value, rec.preview);
  rec.save.disabled = rec.text.value.trim() === '';
}

async function submitNoteSheet(event) {
  event.preventDefault();
  const text = rec.text.value.trim();
  if (!text) {
    toast('写一句再保存');
    rec.text.focus();
    return;
  }
  const editing = rec.editing;
  if (editing) {
    if (await recordsWrite('note_update', { noteId: editing.id, text })) {
      closeNoteSheet();
      toast('已改好');
    }
    return;
  }
  // 新增走今天视图那条命令：归属哪一天、解析出哪些标签都由 Rust 侧算。
  if (await call('note_add', { text })) {
    closeNoteSheet();
    await syncRecords();
    toast('已记下');
  }
}

/* ---------- 事件 ---------- */

document.getElementById('recAdd').addEventListener('click', () => openNoteSheet(null));
document.getElementById('recExport').addEventListener('click', () => openExport('notes'));

rec.list.addEventListener('click', (event) => {
  const hit = (name) => event.target.closest(`[data-${name}]`);

  if (hit('cancel')) {
    rec.confirming = null;
    paintRecords(rec.state);
    return;
  }
  const edit = hit('edit');
  if (edit) {
    const note = findNote(edit.dataset.edit);
    if (note) openNoteSheet(note);
    return;
  }
  const remove = hit('remove');
  if (!remove) return;
  const id = remove.dataset.remove;
  if (rec.confirming !== id) {
    rec.confirming = id;
    paintRecords(rec.state);
    return;
  }
  closeUndo();
  recordsWrite('note_remove', { noteId: id })
    .then((ok) => ok && openUndo(id));
});

rec.tags.addEventListener('click', (event) => {
  const chip = event.target.closest('[data-tag]');
  if (!chip) return;
  rec.tag = rec.tag === chip.dataset.tag ? '' : chip.dataset.tag;
  syncRecords();
});

rec.clear.addEventListener('click', () => {
  rec.query = '';
  rec.tag = '';
  rec.search.value = '';
  syncRecords();
});

let searchTimer = 0;
rec.search.addEventListener('input', () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => {
    rec.query = rec.search.value.trim();
    syncRecords();
  }, 250);
});
rec.search.addEventListener('keydown', (event) => {
  if (event.key === 'Enter') {
    clearTimeout(searchTimer);
    rec.query = rec.search.value.trim();
    syncRecords();
  }
});

rec.undoBtn.addEventListener('click', () => {
  const id = rec.undoId;
  if (!id) return;
  closeUndo();
  recordsWrite('note_restore', { noteId: id })
    .then((ok) => ok && toast('已经撤销'));
});

document.getElementById('noteSheetClose').addEventListener('click', closeNoteSheet);
document.getElementById('noteCancel').addEventListener('click', closeNoteSheet);
rec.form.addEventListener('submit', submitNoteSheet);
rec.text.addEventListener('input', syncNotePreview);
rec.text.addEventListener('keydown', (event) => {
  if (event.key === 'Enter' && !event.shiftKey) {
    event.preventDefault();
    submitNoteSheet(event);
  }
});

rec.scroll.addEventListener('scroll', updateFade, { passive: true });
window.addEventListener('resize', clipTags);

document.addEventListener('keydown', (event) => {
  // 习惯表单在上面时由 app.js 处理，这里不抢同一次 Esc。
  if (event.key === 'Escape' && !rec.sheet.hidden && el.sheet.hidden) closeNoteSheet();
});
