/* 折叠态（DESIGN §2.1、§2.8）：横向日期栏 + 其下垂挂的竖向长条。
   长条只做确认、打卡、撤销三件事；改目标、写记录、看统计仍需展开卡片。
   窗口该变到哪一档由 card_set_view 决定，这里只报「现在折叠到哪一档」。 */

const cf = {
  collapsed: false,
  // 长条整体收起（右上角那根箭头只管这个，不管折叠）。
  rowsShown: true,
  // 这一帧算出来的「有几件事可做」，决定折叠档是带长条还是只剩日期栏。
  actions: 0,
  // 刚被点掉的那一行：只有它演一次「向下裁掉」，之后重画不再演。
  cropped: null,
};

const cfe = {};
for (const id of [
  'card', 'cardHead', 'collapseBtn', 'strip', 'stripBar', 'expandBtn',
  'stripDay', 'stripWeek', 'stripCount', 'stripFold', 'stripRows',
]) {
  cfe[id] = document.getElementById(id);
}

/* ---------- 折叠 / 展开 ---------- */

function cfSlot() {
  if (!cf.collapsed) return views.current;
  return cf.rowsShown && cf.actions > 0 ? 'collapsed' : 'bar';
}

let cfLastSlot = 'today';
// 正在进行的换档。开机要等它落地再让窗口露面，否则会先按旧尺寸显示一下再收。
let cfSizing = Promise.resolve();

function cfSize() {
  const slot = cfSlot();
  if (slot === cfLastSlot) return cfSizing;
  cfLastSlot = slot;
  cfSizing = invoke('card_set_view', { view: slot }).catch((err) => toast(String(err)));
  return cfSizing;
}

function cfSetCollapsed(on) {
  if (on === cf.collapsed) return;
  cf.collapsed = on;
  cfe.card.classList.toggle('is-collapsed', on);
  cfe.strip.hidden = !on;
  // 展开态的顶栏、标签与三个视图整块交给 CSS 收；焦点跟着回到看得见的那一块。
  (on ? cfe.expandBtn : cfe.collapseBtn).focus({ preventScroll: true });
  // 折叠状态自己记住（§2.8）；写失败顶多下次开机回到展开，不该拦住这次折叠。
  invoke('prefs_set', { collapsed: on }).catch((err) => toast(String(err)));
  paintStrip(ui.view);
}

/* ---------- 长条内容 ---------- */

const cfPendingRows = (view) =>
  view.pending
    .filter((item) => !item.isToday)
    .sort((a, b) => (a.localDate < b.localDate ? 1 : a.localDate > b.localDate ? -1 : 0));

const cfTapRows = (view) => view.habits.filter((h) => h.canTapToday && h.todayStatus !== 'done');

const cfDoneRows = (view) => view.habits.filter((h) => h.canTapToday && h.todayStatus === 'done');

const cfRow = (habit, inner) =>
  `<div class="strip-row" style="--habit-accent:${esc(paletteOf(habit.color)[0])}">
     <i class="dot"></i>${inner}
   </div>`;

function stripHtml(view) {
  const pending = cfPendingRows(view);
  const tap = cfTapRows(view);
  const done = cfDoneRows(view);
  cf.actions = pending.length + tap.length;

  const rows = [
    ...pending.map((item) => cfRow(item, `<span class="strip-name">${esc(item.habitName)}</span>
      <span class="strip-when">${esc(relDay(item.localDate, view.today))}</span>
      <button class="choice" type="button" data-confirm="${esc(item.habitId)}"
              data-date="${esc(item.localDate)}" data-status="done">完成</button>
      <button class="choice" type="button" data-confirm="${esc(item.habitId)}"
              data-date="${esc(item.localDate)}" data-status="notDone">没做</button>`)),
    ...tap.map((habit) => cfRow(habit, `<span class="strip-name">${esc(habit.name)}</span>
      <button class="check" type="button" data-tap="${esc(habit.id)}"
              aria-label="${esc(habit.name)}打卡">打卡</button>`)),
    // 点过的行向下裁成窄条：留结果、收动作，不是新的一行催促（§2.8）。
    ...done.map((habit) => `<div class="strip-row is-cropped${
      cf.cropped === habit.id ? ' is-cropping' : ''}"
      style="--habit-accent:${esc(paletteOf(habit.color)[0])}">
      <span class="strip-done">${esc(habit.name)}已打卡</span>
      <button class="text-btn" type="button" data-undo="${esc(habit.id)}">撤销</button>
    </div>`),
  ];
  return { html: rows.join(''), quiet: !cf.actions };
}

function paintStrip(view) {
  if (!view) return;
  const { html, quiet } = stripHtml(view);
  cfe.stripDay.textContent = fmtDay(view.today);
  cfe.stripWeek.textContent = weekdayOf(view.today);
  // 有待确认才报个数；一件可做都没有时，这一格里换成那句「今天没有要记的」。
  // 只剩当天可打卡（没有历史待确认）时这一格留空，不报「0 项待确认」。
  cfe.stripCount.hidden = !quiet && view.pendingTotal === 0;
  cfe.stripCount.textContent = quiet ? '今天没有要记的' : `${view.pendingTotal} 项待确认`;
  cfe.stripFold.hidden = quiet;
  cfe.stripFold.classList.toggle('is-folded', !cf.rowsShown);
  cfe.stripFold.setAttribute('aria-label', cf.rowsShown ? '收起长条' : '展开长条');
  cfe.stripRows.hidden = quiet || !cf.rowsShown;
  const cropped = cf.cropped;
  cfe.stripRows.innerHTML = html;
  cf.cropped = null;
  if (cropped) {
    // 裁切动画只有 260ms，类名却会留到下一次重画；到点自己摘掉，别让它悬在那儿。
    setTimeout(() => {
      const node = cfe.stripRows.querySelector('.is-cropping');
      if (node) node.classList.remove('is-cropping');
    }, 300);
  }
  // 折叠按钮那条路径不走 paint()，淡出得自己刷新，否则沿用的是展开态那块滚动区的判断。
  updateFade();
  cfSize();
}

/* ---------- 拖动热区与 4px 阈值 ---------- */

const DRAG_SLOP = 4;
const cfDrag = { armed: false, dragged: false, x: 0, y: 0 };

function cfBindDrag(node) {
  node.addEventListener('pointerdown', (event) => {
    if (event.button !== 0) return;
    cfDrag.armed = true;
    cfDrag.x = event.clientX;
    cfDrag.y = event.clientY;
  });
  node.addEventListener('pointermove', async (event) => {
    if (!cfDrag.armed || cfDrag.dragged) return;
    if (Math.abs(event.clientX - cfDrag.x) + Math.abs(event.clientY - cfDrag.y) < DRAG_SLOP) return;
    cfDrag.armed = false;
    cfDrag.dragged = true;
    cfe.card.classList.add('is-dragging');
    try {
      // 原生拖动会吃掉随后的 pointerup，所以靠这个 promise 收尾。
      await window.__TAURI__.window.getCurrentWindow().startDragging();
    } finally {
      cfe.card.classList.remove('is-dragging');
      setTimeout(() => { cfDrag.dragged = false; }, 200);
    }
  });
  const disarm = () => { cfDrag.armed = false; };
  node.addEventListener('pointerup', disarm);
  node.addEventListener('pointercancel', disarm);
  node.addEventListener('pointerleave', disarm);
}

// 位移超过阈度的那一次不算点击（§2.1）。
const cfWasDragged = () => cfDrag.dragged;

/* ---------- 接线 ---------- */

cfe.collapseBtn.addEventListener('click', () => {
  if (cfWasDragged()) return;
  cfSetCollapsed(true);
});
cfe.expandBtn.addEventListener('click', () => {
  if (cfWasDragged()) return;
  cfSetCollapsed(false);
});
cfe.stripFold.addEventListener('click', () => {
  cf.rowsShown = !cf.rowsShown;
  invoke('prefs_set', { stripRows: cf.rowsShown }).catch((err) => toast(String(err)));
  paintStrip(ui.view);
});

cfe.stripRows.addEventListener('click', (event) => {
  const confirm = event.target.closest('[data-confirm]');
  if (confirm) {
    const done = confirm.dataset.status === 'done';
    call('checkin_set', {
      habitId: confirm.dataset.confirm,
      localDate: confirm.dataset.date,
      status: confirm.dataset.status,
    }).then((ok) => {
      if (ok) toast(done ? '已完成' : '记为没做');
    });
    return;
  }
  const tap = event.target.closest('[data-tap]');
  if (tap) {
    cf.cropped = tap.dataset.tap;
    call('checkin_set', {
      habitId: tap.dataset.tap,
      localDate: ui.view.today,
      status: 'done',
    }).then((ok) => {
      if (ok) toast(cfName(tap.dataset.tap));
    });
    return;
  }
  const undo = event.target.closest('[data-undo]');
  if (undo) {
    // 撤销写的是 void：读出来是未确认，不是「没做」（§3.3）。
    call('checkin_set', {
      habitId: undo.dataset.undo,
      localDate: ui.view.today,
      status: 'void',
    }).then((ok) => {
      if (ok) toast(`${cfName(undo.dataset.undo)}回到未确认`);
    });
  }
});

const cfName = (id) => {
  const habit = ui.view.habits.find((h) => h.id === id);
  return habit ? `${habit.name}已打卡` : '已打卡';
};

cfBindDrag(cfe.cardHead);
cfBindDrag(cfe.stripBar);

// 长条自己也是一块可翻的区域，淡出提示跟着它算（§8）。
cfe.stripRows.addEventListener('scroll', updateFade, { passive: true });

/* ---------- 开机 ---------- */

// 先读回记住的折叠与长条状态，再画第一帧，最后才让 Rust 把窗口露出来。
// 顺序反了，折叠用户每次开机都会先看见一眼展开的卡片。
(async () => {
  const prefs = await invoke('prefs_get').catch(() => null);
  if (prefs) {
    // 缺字段（老版本设置）按默认值来：长条默认摊开。
    cf.rowsShown = prefs.stripRows !== false;
    if (prefs.collapsed) cfSetCollapsed(true);
  }
  try {
    await refresh();
    // paint 里那次换档可能还在路上；等它落地，窗口就不会按旧尺寸先露一下。
    await cfSize();
  } finally {
    invoke('ui_ready').catch(() => {});
  }
})();
