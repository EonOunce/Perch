/* 今天视图：把 `view_state` 给的字段放进 DOM。
   日界、周归属、完成率、标签解析都只在 Rust 侧算一次（DESIGN §5.4、§5.5），
   这里出现的任何比较都只是排版，不是第二份口径。 */

const MOOD_BANDS = [
  { max: 19, label: '很低落', color: 'var(--mood-1)', mouth: 'M11 22c1.7-2.4 8.3-2.4 10 0' },
  { max: 39, label: '有点低', color: 'var(--mood-2)', mouth: 'M11.4 21.4c1.8-1.5 7.4-1.5 9.2 0' },
  { max: 59, label: '一般', color: 'var(--mood-3)', mouth: 'M11.3 21h9.4' },
  { max: 79, label: '不错', color: 'var(--mood-4)', mouth: 'M11.4 20.5c1.8 2 7.4 2 9.2 0' },
  { max: 100, label: '很好', color: 'var(--mood-5)', mouth: 'M10.8 20c2.1 3.8 8.3 3.8 10.4 0' },
];

const WEEKDAYS = ['周日', '周一', '周二', '周三', '周四', '周五', '周六'];

const ui = {
  view: null,
  busy: false,
  snoozed: false,
  sheetMode: 'weekly',
  sheetTiming: 'sameDay',
};

const el = {};
for (const id of [
  'card', 'scroller', 'dateTitle', 'weekLine', 'storageBanner', 'reviewZone',
  'addHabitTop', 'habitList', 'moodBox', 'moodFace', 'moodName', 'moodValue', 'moodSlider',
  'noteInput', 'tagPreview', 'noteSave', 'noteCount', 'noteList',
  'sheet', 'habitForm', 'sheetClose', 'fName', 'fDef', 'fUnit', 'fCount',
  'fTiming', 'fMode', 'fWeekly', 'fWeeklyWrap', 'toast',
]) {
  el[id] = document.getElementById(id);
}

const bandFor = (score) =>
  MOOD_BANDS.find((band) => score <= band.max) || MOOD_BANDS[MOOD_BANDS.length - 1];

const esc = (text) =>
  String(text).replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

/* ---------- 只用于显示：把后端给的日期串念成人话 ---------- */

const parts = (iso) => iso.slice(0, 10).split('-').map(Number);
const utcDay = (iso) => { const [y, m, d] = parts(iso); return Date.UTC(y, m - 1, d); };
const dayGap = (iso, base) => Math.round((utcDay(iso) - utcDay(base)) / 86400000);
const fmtDay = (iso) => { const [, m, d] = parts(iso); return `${m}月${d}日`; };
const weekdayOf = (iso) => WEEKDAYS[new Date(utcDay(iso)).getUTCDay()];
const relDay = (iso, today) => {
  const gap = dayGap(iso, today);
  return gap === 0 ? '今天' : gap === -1 ? '昨天' : gap === -2 ? '前天' : fmtDay(iso);
};

const icon = (name) => `<svg class="icon"><use href="#i-${name}"/></svg>`;

// perch-core 存的是色位名字，界面上一律换成同一组令牌（§2.4：系统按创建顺序分配）。
const PALETTE = {
  sage: ['var(--sage)', 'var(--sage-wash)'],
  clay: ['var(--clay)', 'var(--clay-wash)'],
  amber: ['var(--amber)', 'var(--amber-wash)'],
  mist: ['var(--mist)', 'var(--mist-wash)'],
};
const paletteOf = (name) => PALETTE[name] || ['var(--sage)', 'var(--sage-wash-soft)'];

let toastTimer = 0;
function toast(message) {
  el.toast.textContent = message;
  el.toast.hidden = false;
  requestAnimationFrame(() => el.toast.classList.add('is-on'));
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    el.toast.classList.remove('is-on');
    setTimeout(() => { el.toast.hidden = true; }, 200);
  }, 1800);
}

/* ---------- 写入 ---------- */

async function refresh() {
  try {
    paint(await window.__TAURI__.core.invoke('view_state'));
  } catch (err) {
    showStorageError(String(err));
  }
}

// 一次写入 = 一次落盘 + 一份新快照：命令返回的就是改完之后的界面数据。
async function call(command, args) {
  if (ui.busy) return false;
  if (ui.view && ui.view.storageError) {
    toast(ui.view.storageError);
    return false;
  }
  ui.busy = true;
  try {
    paint(await window.__TAURI__.core.invoke(command, args));
    return true;
  } catch (err) {
    toast(String(err));
    await refresh();
    return false;
  } finally {
    ui.busy = false;
  }
}

function showStorageError(message) {
  el.storageBanner.textContent = message;
  el.storageBanner.hidden = !message;
  document.body.classList.toggle('is-readonly', Boolean(message));
}

/* ---------- 渲染 ---------- */

function paint(view) {
  ui.view = view;
  showStorageError(view.storageError || '');

  el.dateTitle.textContent = `${fmtDay(view.today)} · ${weekdayOf(view.today)}`;
  el.weekLine.textContent = `本周 ${fmtDay(view.weekStart)}–${fmtDay(view.weekEnd)}`;

  renderReview(view);
  renderHabits(view);
  renderMood(view);
  renderNotes(view);
  el.addHabitTop.hidden = view.isEmpty;
  updateFade();
}

function renderReview(view) {
  const rows = view.pending
    .filter((item) => !item.isToday)
    .sort((a, b) => (a.localDate < b.localDate ? 1 : a.localDate > b.localDate ? -1 : 0));

  if (!rows.length) {
    ui.snoozed = false;
    el.reviewZone.hidden = true;
    el.reviewZone.innerHTML = '';
    return;
  }
  el.reviewZone.hidden = false;

  if (ui.snoozed) {
    el.reviewZone.innerHTML = `<div class="review-quiet">
      <span>${rows.length} 项待确认 · ${fmtDay(rows[rows.length - 1].localDate)} 起</span>
      <button class="text-btn" type="button" data-review="resume">现在回顾</button>
    </div>`;
    return;
  }

  const items = rows.map((item) => {
    const actions = item.confirmable
      ? `<button class="choice" type="button" data-status="done" data-habit="${esc(item.habitId)}" data-date="${esc(item.localDate)}">完成</button>
         <button class="choice" type="button" data-status="notDone" data-habit="${esc(item.habitId)}" data-date="${esc(item.localDate)}">没做</button>`
      : '';
    return `<div class="review-row" style="--habit-accent:${esc(paletteOf(item.color)[0])}">
      <i class="dot"></i>
      <span class="review-name">${esc(item.habitName)}</span>
      <span class="review-day">${esc(relDay(item.localDate, view.today))}</span>
      ${actions}
    </div>`;
  }).join('');

  el.reviewZone.innerHTML = `<div class="review-head">
      <p class="review-q">昨天完成了吗？</p>
      <button class="text-btn" type="button" data-review="later" style="margin-left:auto">稍后</button>
    </div>
    <div class="review-list">${items}</div>`;
}

function weekText(habit) {
  const hasTarget = habit.targetMode !== 'recordOnly' && habit.weeklyTarget !== null;
  const soon = dayGap(habit.targetFrom, ui.view.today) > 0 ? ` ${fmtDay(habit.targetFrom)} 起` : '';
  return hasTarget
    ? `本周 ${habit.weekDone}/${habit.weeklyTarget} 晚${soon}`
    : `本周 ${habit.weekDone} 天${soon}`;
}

function renderHabits(view) {
  if (view.isEmpty) {
    el.habitList.innerHTML = `<div class="habit-empty">
      <span>还没有习惯。</span>
      <button class="primary" type="button" id="addFirstHabit">${icon('plus')}<span>添加第一个习惯</span></button>
    </div>`;
    return;
  }
  el.habitList.innerHTML = view.habits.map((habit) => {
    const [accent, wash] = paletteOf(habit.color);
    const stateText = { done: '已完成', notDone: '没做', unconfirmed: '未确认' }[habit.todayStatus];
    const action = habit.canTapToday
      ? `<button class="check${habit.todayStatus === 'done' ? ' is-on' : ''}" type="button"
           data-checkin="${esc(habit.id)}"
           aria-label="${habit.todayStatus === 'done' ? '撤销' : ''}${esc(habit.name)}打卡">${
           habit.todayStatus === 'done' ? '已打卡' : '打卡'}</button>`
      : `<span class="habit-state">${esc(stateText)}</span>`;
    return `<div class="habit${habit.todayStatus === 'done' ? ' is-done' : ''}"
      style="--habit-accent:${esc(accent)};--habit-wash:${esc(wash)}">
      <span class="habit-icon">${icon(esc(habit.icon))}</span>
      <span class="habit-name">${esc(habit.name)}</span>
      ${action}
      <p class="habit-week">${esc(weekText(habit))}</p>
    </div>`;
  }).join('');
}

function renderMood(view) {
  const score = view.mood === null || view.mood === undefined ? null : view.mood;
  if (score !== null) el.moodSlider.value = String(score);
  paintMood(score === null ? Number(el.moodSlider.value) : score, score === null ? 'idle' : 'saved');
}

function faceSvg(band, unset) {
  return `<svg viewBox="0 0 32 32" aria-hidden="true" focusable="false">
    <circle class="mood-fill" cx="16" cy="16" r="12.4" stroke="currentColor" stroke-width="1.25"/>
    <circle cx="12.1" cy="13.4" r="1.05" fill="currentColor" stroke="none"/>
    <circle cx="19.9" cy="13.4" r="1.05" fill="currentColor" stroke="none"/>
    <path d="${unset ? 'M11.3 21h9.4' : band.mouth}" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round"/>
  </svg>`;
}

function paintMood(value, mode) {
  const band = bandFor(value);
  const unset = mode === 'idle';
  el.moodBox.style.setProperty('--mood-band-color', band.color);
  el.moodFace.innerHTML = faceSvg(band, unset);
  el.moodFace.classList.toggle('is-unset', unset);
  el.moodName.textContent = unset ? '今天还没记' : band.label;
  el.moodValue.textContent = unset ? '—' : `${value}%`;
  el.moodSlider.setAttribute('aria-valuetext', `${value}% · ${band.label}`);
}

function renderNotes(view) {
  const notes = view.notes;
  el.noteCount.hidden = notes.length === 0;
  el.noteCount.textContent = `${notes.length} 条`;
  if (!notes.length) {
    el.noteList.innerHTML = '<p class="note-empty">今天还没有记录。想到什么再写。</p>';
    return;
  }
  el.noteList.innerHTML = notes.map((note) => `<div class="note-row">
    <span class="note-time">${esc(note.createdAt.slice(11, 16))}</span>
    <div>
      <p class="note-text">${esc(note.text)}</p>
      ${note.tags.length
        ? `<div class="note-tags">${note.tags.map((t) => `<span class="chip">#${esc(t)}</span>`).join('')}</div>`
        : ''}
    </div>
  </div>`).join('');
}

function updateFade() {
  const left = el.scroller.scrollHeight - el.scroller.scrollTop - el.scroller.clientHeight;
  el.card.classList.toggle('is-scrollable', left > 4);
}

/* ---------- 表单 ---------- */

function openSheet() {
  el.sheet.hidden = false;
  el.fName.focus();
}

function closeSheet() {
  el.sheet.hidden = true;
  el.habitForm.reset();
  ui.sheetTiming = 'sameDay';
  ui.sheetMode = 'weekly';
  setSeg(el.fTiming, ui.sheetTiming);
  setSeg(el.fMode, ui.sheetMode);
  syncSheet();
}

function setSeg(group, value) {
  for (const button of group.children) {
    button.classList.toggle('is-on', button.dataset.value === value);
  }
}

function syncSheet() {
  el.fWeeklyWrap.hidden = ui.sheetMode === 'recordOnly';
}

function numberValue(input) {
  const raw = input.value.trim();
  if (raw === '') return undefined;
  return Math.max(1, Math.round(Number(raw)) || 1);
}

async function submitSheet(event) {
  event.preventDefault();
  const name = el.fName.value.trim();
  if (!name) {
    toast('给这个习惯起个名字');
    el.fName.focus();
    return;
  }
  const definition = el.fDef.value.trim();
  const unit = el.fUnit.value.trim();
  const saved = await call('habit_add', {
    form: {
      name,
      operationalDefinition: definition === '' ? undefined : definition,
      unit: unit === '' ? undefined : unit,
      targetCount: numberValue(el.fCount),
      confirmationTiming: ui.sheetTiming,
      targetMode: ui.sheetMode,
      weeklyTarget: numberValue(el.fWeekly),
    },
  });
  if (saved) {
    closeSheet();
    toast('已经加上');
  }
}

/* ---------- 碎碎念 ---------- */

// 预览只是即时回显；真正入库的标签由 perch-core 的 parse_tags 决定。
function previewTags(text) {
  const out = [];
  for (const word of text.split(/\s+/)) {
    if (word.startsWith('#') && word.length > 1 && !out.includes(word)) out.push(word);
  }
  el.tagPreview.innerHTML = out.map((t) => `<span class="chip">${esc(t)}</span>`).join('');
}

async function saveNote() {
  const text = el.noteInput.value.trim();
  if (!text) return;
  if (await call('note_add', { text })) {
    el.noteInput.value = '';
    previewTags('');
    el.noteSave.disabled = true;
    toast('已记下');
  }
}

/* ---------- 事件 ---------- */

el.reviewZone.addEventListener('click', (event) => {
  const review = event.target.closest('[data-review]');
  if (review) {
    ui.snoozed = review.dataset.review === 'later';
    renderReview(ui.view);
    return;
  }
  const choice = event.target.closest('[data-habit]');
  if (choice) {
    call('checkin_set', {
      habitId: choice.dataset.habit,
      localDate: choice.dataset.date,
      status: choice.dataset.status,
    }).then((ok) => {
      if (ok) toast(choice.dataset.status === 'done' ? '已完成' : '记为没做');
    });
  }
});

el.habitList.addEventListener('click', (event) => {
  if (event.target.closest('#addFirstHabit')) {
    openSheet();
    return;
  }
  const tap = event.target.closest('[data-checkin]');
  if (!tap) return;
  const habit = ui.view.habits.find((h) => h.id === tap.dataset.checkin);
  if (!habit) return;
  const undo = habit.todayStatus === 'done';
  call('checkin_set', {
    habitId: habit.id,
    localDate: ui.view.today,
    status: undo ? 'void' : 'done',
  }).then((ok) => {
    if (ok) toast(`${habit.name}${undo ? '回到未确认' : '已打卡'}`);
  });
});

el.addHabitTop.addEventListener('click', openSheet);
el.sheetClose.addEventListener('click', closeSheet);
el.habitForm.addEventListener('submit', submitSheet);
el.fTiming.addEventListener('click', (event) => {
  const button = event.target.closest('button');
  if (!button) return;
  ui.sheetTiming = button.dataset.value;
  setSeg(el.fTiming, ui.sheetTiming);
});
el.fMode.addEventListener('click', (event) => {
  const button = event.target.closest('button');
  if (!button) return;
  ui.sheetMode = button.dataset.value;
  setSeg(el.fMode, ui.sheetMode);
  syncSheet();
});

document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && !el.sheet.hidden) closeSheet();
});

// 松手才写入：拖动过程中的预览不产生任何事件（§2.6）。
el.moodSlider.addEventListener('input', () => {
  paintMood(Number(el.moodSlider.value), 'drag');
});
el.moodSlider.addEventListener('change', () => {
  const score = Number(el.moodSlider.value);
  call('mood_set', { localDate: ui.view.today, score }).then((ok) => {
    if (ok) toast('已记下');
  });
});

el.noteInput.addEventListener('input', () => {
  previewTags(el.noteInput.value);
  el.noteSave.disabled = el.noteInput.value.trim() === '';
});
el.noteInput.addEventListener('keydown', (event) => {
  if (event.key === 'Enter' && !event.shiftKey) {
    event.preventDefault();
    saveNote();
  }
});
el.noteSave.addEventListener('click', saveNote);

el.scroller.addEventListener('scroll', updateFade, { passive: true });

window.addEventListener('error', (event) => toast(`界面出错：${event.message}`));

refresh();
