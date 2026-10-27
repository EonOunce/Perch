/* 换标签与换档共用的节奏（DESIGN §8：过渡 ≤ 250ms，只动 transform/opacity）。
   手法是「窗口矩形连续插值」：被点的控件钉在屏幕原地不动，卡片围着它长大或缩小，
   跟系统最大化/还原同一个观感；内容各自的淡出淡入只用来盖住换布局那一下。
   减少动效时全部归零：一步换到位，不演任何补间。 */

const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)');

const MORPH_MS = 160;
const motionOut = () => (reduceMotion.matches ? 0 : 60);
const motionIn = () => (reduceMotion.matches ? 0 : 120);

// 窗口改完尺寸不等于已经画好：等两帧再量、再收尾。
const afterPaint = () => new Promise((resolve) =>
  requestAnimationFrame(() => requestAnimationFrame(resolve)));

const EASE_OUT = 'cubic-bezier(.22,.8,.24,1)';
const easeOut = (t) => 1 - (1 - t) ** 3;
const mix = (a, b, t) => a + (b - a) * t;

// 屏幕（客户区）坐标里的一个点，换算成 el 自己的 transform-origin：
// 淡出淡入的缩放都绕着被点的那一点发生，看起来才像从指尖收进去、从指尖长出来。
function originAt(el, point) {
  const r = el.getBoundingClientRect();
  return `${point.x - r.left}px ${point.y - r.top}px`;
}

const anchorPoint = (el) => {
  const r = el.getBoundingClientRect();
  return { x: r.left, y: r.top };
};

// 锚点要连窗口位置一起记：卡片自己在桌面上是被挪动的，
// 只比客户区坐标会漏掉那一笔，算出来的补偿方向甚至反掉。一律用物理屏幕坐标。
async function anchorScreen(el) {
  const outer = await window.__TAURI__.window.getCurrentWindow().outerPosition();
  const r = el.getBoundingClientRect();
  const s = window.devicePixelRatio || 1;
  return { x: outer.x + r.left * s, y: outer.y + r.top * s };
}

// 收尾补平：按实测位置把预测剩下的几像素挪回去。差得太小就不动，免得为 1px 挪一次窗口。
async function restoreAnchor(el, before) {
  const after = await anchorScreen(el);
  const dx = Math.round(before.x - after.x);
  const dy = Math.round(before.y - after.y);
  if (Math.abs(dx) < 2 && Math.abs(dy) < 2) return;
  await invoke('window_shift_by', { dx, dy });
}

/* ---------- 换档动画 ---------- */

// 被点的控件在新布局里怎么跟着宽度走，决定中间帧的横向预测量：
// 'stretch' 是被宽度撑开的（标签栏三格平分余量），偏移随宽度等比；
// 'left' 贴着卡片左边（顶栏品牌块、折叠态日期块），偏移根本不变；
// 'right' 贴着卡片右边（长条右上角箭头），偏移 = 宽度 - 到右边的距离。
// 纵向三种都不动（卡片内容从顶上排下来）。选错了最多是收尾多补几像素。
const anchorX = (r, w0, w, mode) => {
  if (mode === 'stretch') return r.left * (w / w0);
  if (mode === 'right') return r.left + (w - w0);
  return r.left;
};

// slot 是目标档（today/records/stats/collapsed/bar），anchorEl 是换完布局后仍在那儿的被点控件。
// 中间帧量不到控件在「新布局 + 当前尺寸」下的真实偏移（那要等网页重排，逐帧等就变成逐帧抖），
// 所以按上面那条规则预测，演完再按实测补一次；剩下的残差通常只有几像素。
// 空间不够时 Rust 把窗口夹回工作区：宁可锚点偏一点，不把卡片推出屏幕（§8，本人拍板「屏幕优先」）。
async function morphRect(slot, anchorEl, mode) {
  const [w1, h1] = await invoke('card_target_size', { view: slot });
  const s = window.devicePixelRatio || 1;
  const outer = await window.__TAURI__.window.getCurrentWindow().outerPosition();
  const r0 = anchorEl.getBoundingClientRect();
  const pin = { x: outer.x + r0.left * s, y: outer.y + r0.top * s };
  const w0 = window.innerWidth;
  const h0 = window.innerHeight;
  cfNoteSlot(slot);
  const setBounds = (w, h, ax, ay) => invoke('window_set_bounds', {
    x: Math.round(pin.x - ax * s),
    y: Math.round(pin.y - ay * s),
    w,
    h,
  });
  if (reduceMotion.matches || (w0 === w1 && h0 === h1)) {
    await setBounds(w1, h1, anchorX(r0, w0, w1, mode), r0.top);
    // 不演补间也要落回鼠标下面：等重排后按实测补一次（这一下是瞬时的，不是动画）。
    await afterPaint();
    await restoreAnchor(anchorEl, pin);
    return;
  }
  const start = performance.now();
  // 上一帧没落地就不催下一帧：命令排长队会让窗口按旧尺寸补画，看着就是抖。
  let queued = Promise.resolve();
  await new Promise((resolve) => {
    const frame = () => {
      const t = Math.min(1, (performance.now() - start) / MORPH_MS);
      const e = easeOut(t);
      const w = Math.round(mix(w0, w1, e));
      const h = Math.round(mix(h0, h1, e));
      queued = queued.then(() => setBounds(w, h, anchorX(r0, w0, w, mode), r0.top), () => {});
      if (t < 1) requestAnimationFrame(frame);
      else resolve(queued);
    };
    requestAnimationFrame(frame);
  });
  await afterPaint();
  await restoreAnchor(anchorEl, pin);
}

/* ---------- 内容淡出 / 淡入 ---------- */

// 退场：以点击点为中心略缩并淡出。返回 promise，调用方等它演完再换布局。
function sinkOut(els, ms, point) {
  const nodes = els.filter(Boolean);
  if (!ms || !nodes.length) return Promise.resolve();
  for (const el of nodes) el.style.transformOrigin = originAt(el, point);
  return Promise.all(nodes.map((el) => el.animate(
    [{ opacity: 1, transform: 'scale(1)' }, { opacity: 0, transform: 'scale(.97)' }],
    { duration: ms, easing: 'ease-in' },
  ).finished.catch(() => {}))).then(() => {
    for (const el of nodes) el.style.transformOrigin = '';
  });
}

// 换布局后先把进场那块藏成透明（同一个任务里设，才不会先露一帧新内容）。
const hideNow = (els) => {
  for (const el of els) if (el) el.style.opacity = '0';
};

// 进场：从透明+略缩播到原位；不等窗口演完，与长大/缩小同时进行。
// 演完把 opacity 交还样式表，别留内联样式。
function riseIn(els, ms, point) {
  for (const el of els.filter(Boolean)) {
    if (!ms) {
      el.style.opacity = '';
      continue;
    }
    el.style.transformOrigin = originAt(el, point);
    el.animate(
      [{ opacity: 0, transform: 'scale(.97)' }, { opacity: 1, transform: 'none' }],
      { duration: ms, easing: EASE_OUT },
    ).finished.then(() => {
      el.style.opacity = '';
      el.style.transformOrigin = '';
    }).catch(() => {
      el.style.opacity = '';
    });
  }
}
