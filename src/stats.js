/* 统计视图：习惯范围、范围筛选、完成率、单色相热力图、记录量、心情与每周回顾（DESIGN §6.1–§6.5）。
   这里一个公式都不算：范围怎么解释、分母到哪一天为止、格子什么档位、时段怎么切，
   全在 perch-core 的 stats 里判过一次（§5.4、§5.5），下面只是把已经判好的字段摆进 DOM。
   唯一出现在这里的日期比较是排版用的（沿用 app.js 那句「只用于显示」的口径）。 */

const st = {
  scope: document.getElementById('statScope'),
  range: document.getElementById('statRange'),
  caption: document.getElementById('statCaption'),
  custom: document.getElementById('statCustom'),
  from: document.getElementById('statFrom'),
  to: document.getElementById('statTo'),
  apply: document.getElementById('statApply'),
  error: document.getElementById('statError'),
  body: document.getElementById('statBody'),
  scroll: document.getElementById('statScroll'),
  view: null,
  kind: '30',
  scopeId: 'all',
  // 自定义那一行只是被拉开了，还没应用：应用之前屏上仍是上一次的有效范围。
  openCustom: false,
};

/* 一行能放格子的净宽度：520 的卡片减去左右内边距与行首习惯名（§6.2 每习惯一行）。 */
const HM_CONTENT = 402;
const MOOD_BOX = { left: 6, right: 464, top: 8, bottom: 96 };

const RANGE_UNIT = { dawn: '清晨 4–9', day: '白天 9–17', evening: '傍晚 17–22', night: '深夜 22–4' };
const LEVEL_WORD = { l2: '达标', l1: '做了一部分', l0: '没做', pending: '待确认' };

const cellSize = (days) => {
  const gap = days <= 31 ? 3 : 1;
  const size = Math.max(3, Math.min(44, Math.floor((HM_CONTENT - gap * (days - 1)) / days)));
  const height = days <= 10 ? 20 : days <= 31 ? 15 : 12;
  return `--hm-cell:${size}px;--hm-gap:${gap}px;--hm-h:${height}px`;
};

const pct = (rate) => (rate === null ? '—' : `${Math.round(rate * 100)}%`);

/* ---------- 首屏：一个数字 + 逐习惯各一行 ---------- */

const habitLine = (habit) => {
  const [accent] = paletteOf(habit.color);
  const week = habit.weekTarget === null
    // 只记录阶段没有目标可判，就只报这一周记了几次（§6.2）。
    ? `本周 ${habit.weekDone} ${habit.unit}`
    : `本周 ${habit.weekDone}/${habit.weekTarget} ${habit.unit}`;
  return `<div class="hero-row">
    <i class="scope-dot" style="--dot:${accent}"></i>
    <span class="hero-name">${esc(habit.name)}</span>
    <span class="hero-n">${habit.done}/${habit.total}</span>
    <span class="hero-week">${week}${habit.streakWeeks ? ` · 连着 ${habit.streakWeeks} 周` : ''}</span>
  </div>`;
};

function heroHtml(view) {
  if (!view.habits.length) {
    return `<section class="zone stat-empty">
      <p>还没有习惯。</p>
      <button class="save" type="button" data-act="add">添加第一个习惯</button>
    </section>`;
  }
  // 只有一个习惯时收成一行，不占两列（§6.2）。
  const single = view.habits.length === 1;
  return `<section class="zone stat-hero${single ? ' is-single' : ''}">
    <div class="hero-num">
      <strong>${pct(view.rate)}</strong>
      <span>${view.done}/${view.total}</span>
    </div>
    <div class="hero-rows">${view.habits.map(habitLine).join('')}</div>
  </section>`;
}

/* ---------- 热力图：单色相 small multiples，每习惯一行 ---------- */

const heatRowHtml = (row, days) => {
  const [accent] = paletteOf(row.color);
  const cells = row.cells
    .map((cell) => `<i class="hm-cell is-${cell.level}${cell.today ? ' is-today' : ''}"${
      cell.level === 'blank' ? '' : ` title="${esc(fmtDay(cell.date))} · ${LEVEL_WORD[cell.level]}"`
    }></i>`)
    .join('');
  return `<div class="hm-row" style="--hm:${accent}">
    <span class="hm-name">${esc(row.name)}</span>
    <div class="hm-line" style="${cellSize(days)}">${cells}</div>
  </div>`;
};

// 图例只有两项：三格色阶 + 完成，以及今天（§6.2）。其余解释在「关于栖迟 → 统计口径」。
const heatmapHtml = (view) => `<section class="zone stat-panel">
  <div class="zone-head"><h2>习惯热力图</h2></div>
  <div class="hm-legend">
    <span class="lg"><i class="hm-cell is-l2"></i><i class="hm-cell is-l1"></i><i class="hm-cell is-l0"></i>完成</span>
    <span class="lg"><i class="hm-cell is-ring"></i>今天</span>
  </div>
  <div class="hm-rows">${view.heatmap.rows.map((row) => heatRowHtml(row, view.range.days)).join('')}</div>
</section>`;

/* ---------- 记录量：按日或按周 + 连续天数 + 时段 ---------- */

const bucketLabel = (bucket, perDay) =>
  perDay ? fmtDay(bucket.start) : `${fmtDay(bucket.start)}–${fmtDay(bucket.end)}`;

const timeRowHtml = (item, max) => `<div class="time-row">
  <span class="time-name">${RANGE_UNIT[item.key]}</span>
  <span class="time-track"><i style="width:${max ? Math.round((item.count / max) * 100) : 0}%"></i></span>
  <span class="time-n">${item.count}</span>
</div>`;

function volumeHtml(view) {
  const volume = view.volume;
  const max = Math.max(1, ...volume.buckets.map((bucket) => bucket.count));
  const timeMax = Math.max(1, ...volume.times.map((item) => item.count));
  const chart = volume.total
    ? `<div class="vol-line">${volume.buckets
        .map((bucket) => `<i class="vol-bar${bucket.count ? '' : ' is-zero'}" style="height:${
          bucket.count ? Math.max(8, Math.round((bucket.count / max) * 100)) : 3
        }%" title="${esc(bucketLabel(bucket, volume.perDay))} · ${bucket.count} 条"></i>`)
        .join('')}</div>
      <p class="vol-caption">${fmtDay(view.range.from)} – ${fmtDay(view.range.to)}</p>`
    : '<p class="note-empty">这个区间还没有记录。</p>';
  return `<section class="zone stat-panel">
    <div class="zone-head"><h2>记录量</h2><span class="zone-note">${volume.perDay ? '按天' : '按周'}</span></div>
    <p class="vol-total"><strong>${volume.total}</strong> 条 · 平均每${volume.perDay ? '天' : '周'} ${volume.average.toFixed(1)} 条${
      volume.streakDays ? ` · 连着 ${volume.streakDays} 天有写` : ''}</p>
    ${chart}
    ${volume.times.map((item) => timeRowHtml(item, timeMax)).join('')}
  </section>`;
}

/* 点一下标签就去记录视图看它——筛选的口径仍然只有 records 那一份（§2.5）。 */
const tagsHtml = (volume) => `<section class="zone stat-panel">
  <div class="zone-head"><h2>常常写到</h2></div>
  ${volume.tags.length
    ? `<div class="tag-row">${volume.tags
        .map((item) => `<button class="filter-chip" type="button" data-tag="${esc(item.tag)}">#${esc(item.tag)}<span class="count">${item.count}</span></button>`)
        .join('')}</div>`
    : '<p class="note-empty">这个区间还没有标签。</p>'}
</section>`;

/* ---------- 心情：折线（纵轴固定 0–100）+ 分数段 + 高低完成日对比 ---------- */

function moodSvg(points, view) {
  const span = Math.max(1, dayGap(view.range.to, view.range.from));
  const x = (date) => MOOD_BOX.left + (dayGap(date, view.range.from) / span) * (MOOD_BOX.right - MOOD_BOX.left);
  const y = (value) => MOOD_BOX.bottom - (Math.max(0, Math.min(100, value)) / 100) * (MOOD_BOX.bottom - MOOD_BOX.top);
  const dots = points.map((point) => `<circle cx="${x(point.date).toFixed(1)}" cy="${y(point.value).toFixed(1)}" r="2.4"/>`).join('');
  return `<svg class="mood-line" viewBox="0 0 470 104" aria-hidden="true">
    <polyline points="${points.map((point) => `${x(point.date).toFixed(1)},${y(point.value).toFixed(1)}`).join(' ')}"/>
    ${dots}
  </svg>`;
}

const compareHtml = (compare) => {
  if (!compare.enough) {
    return `<p class="mood-soft">高低完成日各还不够 4 天。样本还少，先看看描述，不急着下结论。</p>`;
  }
  const shown = (value) => (value === null ? '—' : Math.round(value));
  return `<p class="mood-cmp">「${esc(compare.habitName)}」达标的那些天，心情平均 ${
    shown(compare.highAverage)}（${compare.highDays} 天）；没做的那些天 ${
    shown(compare.lowAverage)}（${compare.lowDays} 天）。</p>
  <p class="mood-soft">相关不等于因果。</p>`;
};

function moodHtml(view) {
  const mood = view.mood;
  const points = mood.useWeekly ? mood.weeks : mood.days;
  const chart = mood.enough
    ? `${moodSvg(points, view)}<p class="vol-caption">纵轴 0–100 · ${mood.useWeekly ? '每点一周的均值' : '每点一天'}</p>`
    : `<p class="mood-soft">有评分的日子还不到两个，先不画线。${mood.count ? `目前 ${mood.count} 天。` : ''}</p>`;
  const bands = mood.bands
    .map((band, index) => {
      const meta = MOOD_BANDS.find((item) => item.max === band.max) || MOOD_BANDS[index];
      return `<span class="band"><i style="background:${meta.color}"></i>${meta.label}<b>${band.count}</b></span>`;
    })
    .join('');
  return `<section class="zone stat-panel">
    <div class="zone-head"><h2>心情趋势</h2><span class="zone-note">${
      mood.average === null ? '还没有评分' : `平均 ${Math.round(mood.average)}`}</span></div>
    ${chart}
    <div class="band-row">${bands}</div>
    ${view.compare ? compareHtml(view.compare) : ''}
  </section>`;
}

/* ---------- 每周回顾：固定看最近一个完整的周一–周日（§6.5） ---------- */

const reviewHtml = (review) => `<section class="zone stat-panel">
  <div class="zone-head"><h2>每周回顾</h2><span class="zone-note">${
    fmtDay(review.weekStart)} – ${fmtDay(review.weekEnd)}</span></div>
  ${review.lines.length
    ? review.lines.map((line) => `<p class="rv-line">${esc(line)}</p>`).join('')
    : '<p class="note-empty">那一周还没有记录。</p>'}
</section>`;

/* ---------- 摆进 DOM ---------- */

function paintTools(view) {
  // 候选列全量：用 view.habits 会在选中单习惯后把其余习惯从选择器里抹掉，切不回去。
  st.scope.innerHTML = `<button class="filter-chip${view.scope === 'all' ? ' is-on' : ''}" type="button"
      data-scope="all" aria-pressed="${view.scope === 'all'}">全部</button>${view.scopeOptions
    .map((habit) => `<button class="filter-chip${view.scope === habit.id ? ' is-on' : ''}" type="button"
        data-scope="${esc(habit.id)}" aria-pressed="${view.scope === habit.id}"><i class="scope-dot" style="--dot:${
        paletteOf(habit.color)[0]}"></i>${esc(habit.name)}</button>`)
    .join('')}`;

  for (const button of st.range.children)
    button.classList.toggle('is-on', button.dataset.range === view.range.kind);

  st.caption.textContent = `${fmtDay(view.range.from)} – ${fmtDay(view.range.to)} · ${view.range.days} 天`;
  st.scopeId = view.scope;
}

function paintStats(snap) {
  showStorageError(snap.storageError || '');
  st.error.hidden = !snap.rangeError;
  st.error.textContent = snap.rangeError || '';
  // 非法范围只换掉那一句就地提示，屏上仍是上一次的有效快照（§6.1）。
  if (!snap.stats) return;
  st.view = snap.stats;
  st.kind = snap.stats.range.kind;
  paintTools(st.view);
  st.body.innerHTML = [
    heroHtml(st.view),
    st.view.heatmap.rows.length ? heatmapHtml(st.view) : '',
    volumeHtml(st.view),
    tagsHtml(st.view.volume),
    moodHtml(st.view),
    reviewHtml(st.view.review),
  ].join('');
  updateFade();
}

async function syncStats() {
  const args = { range: st.kind };
  if (st.kind === 'custom') {
    args.from = st.from.value;
    args.to = st.to.value;
  }
  if (st.scopeId !== 'all') args.habit = st.scopeId;
  try {
    paintStats(await invoke('stats_state', args));
  } catch (err) {
    toast(String(err));
  }
}

views.hooks.stats = syncStats;

function openCustomRow(view) {
  st.openCustom = true;
  st.custom.hidden = false;
  if (!st.from.value) st.from.value = view ? view.range.from : '';
  if (!st.to.value) st.to.value = view ? view.range.to : '';
  st.from.focus();
}

/* ---------- 事件 ---------- */

st.scope.addEventListener('click', (event) => {
  const chip = event.target.closest('[data-scope]');
  if (!chip) return;
  st.scopeId = chip.dataset.scope;
  syncStats();
});

st.range.addEventListener('click', (event) => {
  const button = event.target.closest('[data-range]');
  if (!button) return;
  if (button.dataset.range === 'custom') {
    openCustomRow(st.view);
    return;
  }
  st.openCustom = false;
  st.custom.hidden = true;
  st.kind = button.dataset.range;
  syncStats();
});

st.apply.addEventListener('click', () => {
  st.kind = 'custom';
  syncStats();
});
for (const input of [st.from, st.to])
  input.addEventListener('keydown', (event) => {
    if (event.key === 'Enter') {
      event.preventDefault();
      st.kind = 'custom';
      syncStats();
    }
  });

st.body.addEventListener('click', (event) => {
  const tag = event.target.closest('[data-tag]');
  if (tag) {
    rec.tag = tag.dataset.tag;
    rec.query = '';
    rec.search.value = '';
    setView('records');
    return;
  }
  if (event.target.closest('[data-act="add"]')) openPanel();
});

st.scroll.addEventListener('scroll', updateFade, { passive: true });
