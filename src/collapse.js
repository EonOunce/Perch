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
  'stripDay', 'stripWeek', 'stripCount', 'stripQuick', 'stripFold', 'stripRows',
]) {
  cfe[id] = document.getElementById(id);
}

/* ---------- 折叠 / 展开 ---------- */

/* 面板（`.sheet`）是贴在卡片底部的浮层，折叠那一档只有 169 / 46px 高，装不下它。
   面板开着的时候把窗口临时撑到今天的档（360×480），关掉回到原来那一档：
   折叠状态本身不写进设置，她下次看到的还是自己收起的那条。 */
const cfSheetsOpen = () => [...document.querySelectorAll('.sheet')].some((s) => !s.hidden);

function cfSlot() {
  if (!cf.collapsed) return views.current;
  if (cfSheetsOpen()) return 'today';
  return cf.rowsShown && cf.actions > 0 ? 'collapsed' : 'bar';
}

let cfLastSlot = 'today';
// 正在进行的换档。开机要等它落地再让窗口露面，否则会先按旧尺寸显示一下再收。
let cfSizing = Promise.resolve();

// 回值是「窗口尺寸这次到底变没变」：变了才要等网页重排完再量锚点。
function cfSize() {
  const slot = cfSlot();
  // 档位没变也要等在路上的那一次落地（开机要靠这个），但不算「这次变了」。
  if (slot === cfLastSlot) return cfSizing.then(() => false);
  cfLastSlot = slot;
  cfSizing = invoke('card_set_view', { view: slot }).catch((err) => {
    toast(String(err));
    return false;
  });
  return cfSizing;
}

// morphRect 直接逐帧改窗口尺寸，绕过了 card_set_view；换完档要登记一笔现在在哪一档，
// 否则 cfSize 以为还在旧档，下一次数据驱动的换档会被当成「没变化」跳过。
function cfNoteSlot(slot) {
  cfLastSlot = slot;
}

// 只改状态与 DOM，不碰窗口：开机要用它先画第一帧，换档则等内容退场后再用。
function cfShowCollapsed(on) {
  cf.collapsed = on;
  cfe.card.classList.toggle('is-collapsed', on);
  cfe.strip.hidden = !on;
  // 展开态的顶栏、标签与三个视图整块交给 CSS 收；焦点跟着回到看得见的那一块。
  (on ? cfe.expandBtn : cfe.collapseBtn).focus({ preventScroll: true });
}

let cfSeq = 0;
let cfQueue = Promise.resolve();

function cfSetCollapsed(on, clicked) {
  const mine = ++cfSeq;
  cfQueue = cfQueue.catch(() => {}).then(async () => {
    // 过渡途中又点了一下（或点了相反方向）：只走最后一次，别来回换档。
    if (mine !== cfSeq || on === cf.collapsed) return;
    const expandedSurfaces = () => [
      ...document.querySelectorAll('.card-head, .tabs, .card > .banner, .scroller'),
    ];
    const outgoing = on ? expandedSurfaces() : [cfe.strip];
    const incoming = on ? [cfe.strip] : expandedSurfaces();
    // 收起/展开是两套布局：看得见的那块先朝点击点退掉，摆好新布局后再逐帧换窗口尺寸。
    const fromEl = clicked || (on ? cfe.collapseBtn : cfe.expandBtn);
    const toEl = on ? cfe.expandBtn : cfe.collapseBtn;
    const point = anchorPoint(fromEl);
    await sinkOut(outgoing, motionOut(), point);
    cfShowCollapsed(on);
    // 折叠状态自己记住（§2.8）；写失败顶多下次开机回到展开，不该拦住这次折叠。
    invoke('prefs_set', { collapsed: on }).catch((err) => toast(String(err)));
    renderStrip(ui.view);
    hideNow(incoming);
    // 折叠与展开点的是同一处：顶栏品牌块 / 日期块，都贴着卡片左边，横向偏移不用预测。
    const morph = morphRect(cfSlot(), toEl);
    riseIn(incoming, motionIn(), point);
    await morph;
    updateFade();
  }).catch((err) => {
    // 卡在其中一步时不要把内容永远留在透明态；报一声，用户还能继续操作。
    for (const el of document.querySelectorAll('.card-head, .tabs, .scroller, .strip')) {
      el.style.opacity = '';
    }
    toast(String(err));
  });
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

// 只画长条与日期栏，不碰窗口：换档动画要先把新布局摆好，再逐帧改窗口矩形。
function renderStrip(view) {
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
}

// 数据驱动的那条路（打卡后长条行数变了、开机画第一帧）：换档一步到位，不演动画。
// 没有「被点的控件」要钉住，动画在这里只会凭空抖一下。
function paintStrip(view) {
  if (!view) return Promise.resolve(false);
  renderStrip(view);
  return cfSize();
}

/* ---------- 拖动热区与 4px 阈值 ---------- */

const DRAG_SLOP = 4;
const cfDrag = { armed: false, dragged: false, x: 0, y: 0 };

function cfBindDrag(node) {
  node.addEventListener('pointerdown', (event) => {
    if (event.button !== 0) return;
    // 顶栏只有品牌块（和折叠态的日期块）既要点又要拖；其余按钮按下去就是按下去，
    // 不然在 ⓘ 上稍微挪一下就把整张卡拖走了。
    const hit = event.target.closest('button');
    if (hit && !hit.classList.contains('brand') && !hit.classList.contains('strip-date')) return;
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
  cfSetCollapsed(true, cfe.collapseBtn);
});
cfe.expandBtn.addEventListener('click', () => {
  if (cfWasDragged()) return;
  cfSetCollapsed(false, cfe.expandBtn);
});
cfe.stripFold.addEventListener('click', () => cfSetRowsShown(!cf.rowsShown, cfe.stripFold));
// 折叠态也要能记一条（§2.8）：小窗是另一个窗口，卡片的档位与层级都不动。
cfe.stripQuick.addEventListener('click', () => invoke('quick_toggle').catch(toast));

let cfRowsSeq = 0;
let cfRowsQueue = Promise.resolve();

// 长条收起/摊开：行先朝点击点收进去（或从点击点长出来），窗口围着那颗箭头逐帧换档（§8）。
function cfSetRowsShown(on, clicked) {
  const mine = ++cfRowsSeq;
  cfRowsQueue = cfRowsQueue.catch(() => {}).then(async () => {
    if (mine !== cfRowsSeq || on === cf.rowsShown) return;
    const anchorEl = clicked || cfe.stripFold;
    const point = anchorPoint(anchorEl);
    if (!on) await sinkOut([cfe.stripRows], motionOut(), point);
    cf.rowsShown = on;
    invoke('prefs_set', { stripRows: on }).catch((err) => toast(String(err)));
    renderStrip(ui.view);
    if (on) hideNow([cfe.stripRows]);
    // 箭头在日期栏最右边；这两档同宽（360），换成别的档也按「到右边的距离不变」算。
    const morph = morphRect(cfSlot(), cfe.stripFold, 'right');
    if (on) riseIn([cfe.stripRows], motionIn(), point);
    await morph;
    updateFade();
  }).catch((err) => {
    cfe.stripRows.style.opacity = '';
    toast(String(err));
  });
}

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

/* 面板的开关发生在各自的文件里（about / sync / export / restore / 表单），
   这里只盯 `hidden` 这一个属性的变化：第一块面板打开时撑开窗口，最后一块关掉时收回去。 */
const cfSheetWatch = new MutationObserver(() => {
  cfe.card.classList.toggle('has-sheet', cfSheetsOpen());
  cfSize();
});
for (const sheet of document.querySelectorAll('.sheet')) {
  cfSheetWatch.observe(sheet, { attributes: true, attributeFilter: ['hidden'] });
}

/* ---------- 开机 ---------- */

// 先读回记住的折叠与长条状态，再画第一帧，最后才让 Rust 把窗口露出来。
// 顺序反了，折叠用户每次开机都会先看见一眼展开的卡片。
(async () => {
  const prefs = await invoke('prefs_get').catch(() => null);
  if (prefs) {
    // 缺字段（老版本设置）按默认值来：长条默认摊开。
    cf.rowsShown = prefs.stripRows !== false;
    // 开机不演过渡：先把状态摆好，下一行 refresh 画出来的就是最终那一帧。
    if (prefs.collapsed) cfShowCollapsed(true);
  }
  try {
    await refresh();
    // paint 里那次换档可能还在路上；等它落地，窗口就不会按旧尺寸先露一下。
    await cfSize();
  } finally {
    invoke('ui_ready').catch(() => {});
  }
})();
