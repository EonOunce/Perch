'use strict';

/* --------------------------------------------------------------------------
   Date helpers — the prototype fixes "today" at 2026-10-22 so the weekly
   stories stay consistent. Business dates use a local date key; timestamps
   are ISO strings, mirroring DESIGN.md §3.
   -------------------------------------------------------------------------- */

const DEMO_TODAY = new Date(2026, 9, 22, 12, 0, 0);
const TODAY_KEY = dateKey(DEMO_TODAY);
const YESTERDAY_KEY = dateKey(addDays(DEMO_TODAY, -1));
const READING_START_KEY = '2026-07-27';
const WALK_START_KEY = '2026-09-07';
const DEVICE_ID = 'dev_demo_1022';

function pad2(value) {
  return String(value).padStart(2, '0');
}

function dateKey(date) {
  return `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`;
}

function fromKey(key) {
  const [year, month, day] = key.split('-').map(Number);
  return new Date(year, month - 1, day, 12, 0, 0);
}

function addDays(date, amount) {
  const next = new Date(date.getFullYear(), date.getMonth(), date.getDate(), 12, 0, 0);
  next.setDate(next.getDate() + amount);
  return next;
}

function diffDays(from, to) {
  const a = Date.UTC(from.getFullYear(), from.getMonth(), from.getDate());
  const b = Date.UTC(to.getFullYear(), to.getMonth(), to.getDate());
  return Math.round((b - a) / 86400000);
}

function startOfWeek(date) {
  const base = new Date(date.getFullYear(), date.getMonth(), date.getDate(), 12, 0, 0);
  const offset = (base.getDay() + 6) % 7;
  return addDays(base, -offset);
}

function endOfWeek(date) {
  return addDays(startOfWeek(date), 6);
}

function weekdayCN(date) {
  return ['周日', '周一', '周二', '周三', '周四', '周五', '周六'][date.getDay()];
}

function fmtCN(value) {
  const date = typeof value === 'string' ? fromKey(value) : value;
  return `${date.getMonth() + 1}月${date.getDate()}日`;
}

function fmtCNFull(value) {
  const date = typeof value === 'string' ? fromKey(value) : value;
  return `${fmtCN(date)} ${weekdayCN(date)}`;
}

function timestamp(key, hour, minute) {
  const date = fromKey(key);
  date.setHours(hour, minute, 0, 0);
  return date.toISOString();
}

function timeLabel(isoString) {
  const date = new Date(isoString);
  return `${pad2(date.getHours())}:${pad2(date.getMinutes())}`;
}

function isoWeek(date) {
  const target = new Date(Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()));
  const dayNumber = target.getUTCDay() || 7;
  target.setUTCDate(target.getUTCDate() + 4 - dayNumber);
  const yearStart = new Date(Date.UTC(target.getUTCFullYear(), 0, 1));
  return Math.ceil((((target - yearStart) / 86400000) + 1) / 7);
}

function clamp(value, min, max) {
  return Math.min(Math.max(value, min), max);
}

function escapeHtml(value) {
  return String(value).replace(/[&<>"']/g, (character) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;',
  })[character]);
}

function escapeAttr(value) {
  return escapeHtml(value).replace(/`/g, '&#96;');
}

function parseTags(text) {
  const tags = [];
  const pattern = /#([\p{L}\p{N}_-]+)/gu;
  let match;
  while ((match = pattern.exec(text)) !== null) {
    const tag = match[1];
    if (!tags.includes(tag)) tags.push(tag);
  }
  return tags;
}

function renderRichText(text) {
  let html = '';
  let cursor = 0;
  const pattern = /#([\p{L}\p{N}_-]+)/gu;
  let match;
  while ((match = pattern.exec(text)) !== null) {
    html += escapeHtml(text.slice(cursor, match.index));
    const tag = match[1];
    html += `<button type="button" class="tag-inline" data-tag-jump="${escapeAttr(tag)}">#${escapeHtml(tag)}</button>`;
    cursor = match.index + match[0].length;
  }
  html += escapeHtml(text.slice(cursor));
  return html;
}

function mulberry32(seed) {
  let value = seed;
  return function random() {
    value |= 0;
    value = (value + 0x6D2B79F5) | 0;
    let result = Math.imul(value ^ (value >>> 15), 1 | value);
    result = (result + Math.imul(result ^ (result >>> 7), 61 | result)) ^ result;
    return ((result ^ (result >>> 14)) >>> 0) / 4294967296;
  };
}

/* --------------------------------------------------------------------------
   Demo database — same shape as DESIGN.md §3.2
   -------------------------------------------------------------------------- */

const db = {
  schemaVersion: 1,
  generatedAt: timestamp(TODAY_KEY, 9, 12),
  deviceId: DEVICE_ID,
  habits: [],
  checkinEvents: [],
  notes: [],
  dailyMoods: [],
  markers: [],
  tombstones: [],
};

const HABIT_COLORS = {
  sage: { accent: '#5b6f57', wash: '#e9eee1', dot: '#7e9278' },
  amber: { accent: '#7e6035', wash: '#f1e5d1', dot: '#bd975f' },
  clay: { accent: '#845c44', wash: '#f0e2d8', dot: '#b0866a' },
  mist: { accent: '#596c6f', wash: '#e5eceb', dot: '#8d9b9d' },
};

const HABIT_PALETTE = ['sage', 'clay', 'amber', 'mist'];
const HABIT_ICON_CYCLE = ['i-book', 'i-leaf', 'i-clock', 'i-tag', 'i-chart', 'i-pencil'];

// 图标与配色按顺序自动分配，不让用户挑：颜色只用于区分习惯，挑色不产生任何信息。
function habitIdentity(index) {
  return {
    color: HABIT_PALETTE[index % HABIT_PALETTE.length],
    icon: HABIT_ICON_CYCLE[index % HABIT_ICON_CYCLE.length],
  };
}

const readingHabit = {
  id: 'hbt_reading',
  name: '阅读',
  icon: 'i-book',
  color: 'sage',
  operationalDefinition: '睡前翻开书，连续读满 10 分钟；纸质或电子都可以。',
  unit: '晚',
  confirmationTiming: 'nextDay',
  targetMode: 'weekly',
  weeklyTarget: 5,
  ifThen: '如果洗完澡，就把书放到枕边。',
  reminderTime: '21:30',
  sortOrder: 10,
  archivedAt: null,
  statsVisible: true,
  goalHistory: [
    { effectiveFrom: '2026-07-27', targetMode: 'recordOnly', weeklyTarget: null },
    { effectiveFrom: '2026-08-03', targetMode: 'weekly', weeklyTarget: 5 },
  ],
  createdAt: timestamp('2026-07-27', 9, 0),
  updatedAt: timestamp('2026-10-06', 8, 10),
};

const walkHabit = {
  id: 'hbt_walk',
  name: '散步',
  icon: 'i-leaf',
  color: 'clay',
  operationalDefinition: '晚饭后出门走 10 分钟，慢走也算。',
  unit: '次',
  confirmationTiming: 'sameDay',
  targetMode: 'weekly',
  weeklyTarget: 4,
  ifThen: '如果吃完晚饭，就换上鞋出门。',
  reminderTime: '19:30',
  sortOrder: 20,
  archivedAt: null,
  statsVisible: true,
  goalHistory: [
    { effectiveFrom: '2026-09-07', targetMode: 'recordOnly', weeklyTarget: null },
    { effectiveFrom: '2026-09-14', targetMode: 'weekly', weeklyTarget: 4 },
  ],
  createdAt: timestamp('2026-09-07', 20, 30),
  updatedAt: timestamp('2026-10-05', 7, 20),
};

db.habits.push(readingHabit, walkHabit);

db.markers.push(
  {
    id: 'mrk_read_1', habitId: 'hbt_reading', localDate: '2026-08-03',
    label: '改成洗完澡就读', createdAt: timestamp('2026-08-03', 8, 0), updatedAt: timestamp('2026-08-03', 8, 0),
  },
  {
    id: 'mrk_read_2', habitId: 'hbt_reading', localDate: '2026-10-06',
    label: '换回纸质书', createdAt: timestamp('2026-10-06', 8, 0), updatedAt: timestamp('2026-10-06', 8, 0),
  },
  {
    id: 'mrk_walk_1', habitId: 'hbt_walk', localDate: '2026-09-14',
    label: '晚饭后立刻换鞋', createdAt: timestamp('2026-09-14', 19, 0), updatedAt: timestamp('2026-09-14', 19, 0),
  },
);

const random = mulberry32(20261022);

function pushSeedEvent(habit, localDate, status) {
  const nextDay = habit.confirmationTiming === 'nextDay';
  const confirmationKey = dateKey(addDays(fromKey(localDate), nextDay ? 1 : 0));
  if (confirmationKey > TODAY_KEY) return;
  const hour = nextDay ? 7 + Math.floor(random() * 2) : 19 + Math.floor(random() * 2);
  const minute = Math.floor(random() * 52);
  const confirmedAt = timestamp(confirmationKey, hour, minute);
  db.checkinEvents.push({
    id: `cke_${habit.id}_${localDate.replaceAll('-', '')}_${status}`,
    habitId: habit.id,
    localDate,
    status,
    confirmedAt,
    source: nextDay ? 'morningReview' : 'sameDayCheckin',
    supersedesId: null,
    recordedBy: DEVICE_ID,
    note: '',
    createdAt: confirmedAt,
  });
}

function seedEventsInRange(habit, weekStart, plan) {
  for (let day = 0; day < 7; day += 1) {
    const localDate = dateKey(addDays(weekStart, day));
    if (localDate < habitStart(habit) || localDate > YESTERDAY_KEY) continue;
    if (plan.done.includes(day)) pushSeedEvent(habit, localDate, 'done');
    if (plan.notDone.includes(day)) pushSeedEvent(habit, localDate, 'notDone');
  }
}

function seedReadingCheckins() {
  const firstWeek = startOfWeek(fromKey(READING_START_KEY));
  const lastWeek = startOfWeek(DEMO_TODAY);
  const totalWeeks = Math.round(diffDays(firstWeek, lastWeek) / 7) + 1;

  for (let weekIndex = 0; weekIndex < totalWeeks; weekIndex += 1) {
    const weekStart = addDays(firstWeek, weekIndex * 7);
    const weekKey = dateKey(weekStart);
    let plan;

    if (weekKey === dateKey(lastWeek)) {
      plan = { done: [0, 1], notDone: [] };
    } else if (weekIndex === 0) {
      plan = { done: [0, 2, 4], notDone: [1] };
    } else if (weekKey === '2026-08-24') {
      plan = { done: [0, 1, 2, 4], notDone: [3] };
    } else {
      const miss = (weekIndex + 3) % 7;
      const skip = (weekIndex * 2 + 1) % 7;
      plan = {
        done: [0, 1, 2, 3, 4, 5, 6].filter((day) => day !== miss && day !== skip),
        notDone: [miss],
      };
    }
    seedEventsInRange(readingHabit, weekStart, plan);
  }
}

function seedWalkCheckins() {
  const plans = {
    '2026-09-07': { done: [0, 2, 4], notDone: [1] },
    '2026-09-14': { done: [0, 1, 3, 5], notDone: [2] },
    '2026-09-21': { done: [0, 1, 2, 4], notDone: [5] },
    '2026-09-28': { done: [0, 1, 2, 4, 5], notDone: [3] },
    '2026-10-05': { done: [0, 2, 3, 4], notDone: [1] },
    '2026-10-12': { done: [0, 1, 2, 3, 4], notDone: [5] },
    '2026-10-19': { done: [0, 1, 2], notDone: [] },
  };

  const firstWeek = startOfWeek(fromKey(WALK_START_KEY));
  const lastWeek = startOfWeek(DEMO_TODAY);
  for (let weekStart = firstWeek; diffDays(weekStart, lastWeek) >= 0; weekStart = addDays(weekStart, 7)) {
    const plan = plans[dateKey(weekStart)];
    if (plan) seedEventsInRange(walkHabit, weekStart, plan);
  }
}

function addSeedNote(localDate, hour, minute, text, id) {
  const createdAt = timestamp(localDate, hour, minute);
  db.notes.push({
    id,
    createdAt,
    localDate,
    text,
    tags: parseTags(text),
    updatedAt: createdAt,
    conflictOf: null,
  });
}

function seedNotes() {
  const manualNotes = [
    ['2026-10-22', 7, 48, '早读前把书放到了枕边，昨晚读了两页。 #阅读 #自我照顾'],
    ['2026-10-22', 9, 12, '把钟面教具装进盒子了。 #备课'],
    ['2026-10-21', 20, 40, '晚饭后走了十分钟，风有点凉。 #自我照顾'],
    ['2026-10-21', 16, 35, '小可在钟面配对里坚持了 6 分钟，比上周多 2 分钟。 #课堂观察 #正向记录'],
    ['2026-10-21', 21, 6, '洗完澡真的把书放到枕边了。if–then 比「记得读书」好用。 #阅读 #碎碎念'],
    ['2026-10-20', 12, 20, '今天把「先…再…」提示卡放在桌角，孩子自己指了一次。 #课堂观察 #视觉支持'],
    ['2026-10-20', 19, 5, '绕着小区走了一圈，没有带手机。 #自我照顾'],
    ['2026-10-19', 8, 5, '这周只要求自己五个晚上，不追连续。 #阅读'],
    ['2026-10-18', 20, 40, '读到一句喜欢的话：慢一点，也是在走。 #碎碎念'],
    ['2026-10-17', 15, 30, '课后和妈妈沟通了代币的使用，强调只加不减。 #家校沟通'],
    ['2026-10-16', 11, 10, '把数学课的任务拆成两步，孩子完成后主动抬头看我。 #课堂观察 #正向记录'],
    ['2026-10-16', 21, 0, '今天有点累，但还是写下了三行。 #自我照顾'],
    ['2026-10-15', 9, 45, '教案改到第三版，最后决定保留最简单的那个活动。 #备课'],
    ['2026-10-14', 20, 15, '把明早要用的书和教具放在一起，给早上的自己少留一个问题。 #备课 #自我照顾'],
    ['2026-10-13', 19, 20, '晚饭后把书放到枕边，算是给明早的自己留个线索。 #阅读'],
    ['2026-10-12', 17, 40, '小可主动把积木递给同伴，记一次「主动发起互动」。 #课堂观察 #正向记录'],
  ];

  manualNotes.forEach((entry, index) => {
    const [localDate, hour, minute, text] = entry;
    addSeedNote(localDate, hour, minute, text, `note_manual_${index + 1}`);
  });

  addSeedNote('2026-07-30', 19, 20, '一条已经移除的旧记录。 #碎碎念', 'note_removed_demo');
  db.tombstones.push({
    entityType: 'note',
    entityId: 'note_removed_demo',
    deletedAt: timestamp('2026-08-01', 10, 5),
    deletedBy: DEVICE_ID,
  });

  const templates = [
    '今天把「先…再…」的提示卡放在桌角，孩子自己指了一次。 #课堂观察 #视觉支持',
    '教案改到第三版，还是决定把任务拆成两步。 #备课',
    '晚饭后散步十分钟，没有带手机。 #自我照顾',
    '读到一句喜欢的话：慢一点，也是在走。 #碎碎念',
    '今天没有特别的事，窗外的光很好。 #碎碎念',
    '把明天的教具装进盒子了，早上会轻松一点。 #备课',
    '小可主动把积木递给同伴，记一次「主动发起互动」。 #课堂观察 #正向记录',
    '有点累，但还是写下了三行。 #自我照顾 #碎碎念',
    '这周的目标是 5 晚，今晚算第 3 晚。 #阅读',
    '课后和妈妈沟通了代币的使用，强调只加不减。 #家校沟通',
    '把钟面模型擦干净放回盒子，明天可以直接用。 #备课',
    '午睡后情绪恢复得比昨天快，用计时器提示了两次。 #课堂观察',
    '不想写太多，只记一句：今天按时下班了。 #自我照顾',
    '把任务卡换成图片版，孩子看的时间更长了一点。 #视觉支持 #课堂观察',
    '睡前把书翻到上次停下的那一页。 #阅读',
  ];

  const firstGenerated = fromKey('2026-07-27');
  const lastGenerated = fromKey('2026-10-11');
  for (let date = firstGenerated; diffDays(date, lastGenerated) >= 0; date = addDays(date, 1)) {
    const key = dateKey(date);
    const roll = random();
    const noteCount = roll < 0.37 ? 0 : roll < 0.84 ? 1 : 2;
    for (let index = 0; index < noteCount; index += 1) {
      const template = templates[Math.floor(random() * templates.length)];
      const hour = 8 + Math.floor(random() * 13);
      const minute = 5 + Math.floor(random() * 50);
      addSeedNote(key, hour, minute, template, `note_gen_${key.replaceAll('-', '')}_${index}`);
    }
  }
}

function seedMoods() {
  const readingMap = effectiveByDate(readingHabit.id);
  const walkMap = effectiveByDate(walkHabit.id);

  for (let date = fromKey(READING_START_KEY); diffDays(date, fromKey(YESTERDAY_KEY)) >= 0; date = addDays(date, 1)) {
    const key = dateKey(date);
    if (random() < 0.12) continue;
    let score = 52;
    const reading = statusOn(readingHabit.id, key, readingMap);
    if (reading === 'done') score += 14;
    else if (reading === 'notDone') score -= 7;
    const walk = statusOn(walkHabit.id, key, walkMap);
    if (walk === 'done') score += 6;
    else if (walk === 'notDone') score -= 3;
    score += [-9, -4, 0, 0, 3, 7][Math.floor(random() * 6)];
    if (random() < 0.04) score -= 9;
    db.dailyMoods.push({
      id: `mood_${key}`,
      localDate: key,
      score: clamp(Math.round(score), 6, 96),
      updatedAt: timestamp(key, 20 + Math.floor(random() * 2), 5 + Math.floor(random() * 50)),
    });
  }
}

/* --------------------------------------------------------------------------
   Lookup helpers
   -------------------------------------------------------------------------- */

function isTombstoned(entityType, entityId) {
  return db.tombstones.some((tombstone) => (
    tombstone.entityType === entityType && tombstone.entityId === entityId
  ));
}

function liveNotes() {
  return db.notes.filter((note) => !isTombstoned('note', note.id));
}

function liveMoods() {
  return db.dailyMoods.filter((mood) => !isTombstoned('dailyMood', mood.id));
}

function eventNewer(candidate, current) {
  const candidateTime = Date.parse(candidate.confirmedAt || candidate.createdAt || 0);
  const currentTime = Date.parse(current.confirmedAt || current.createdAt || 0);
  if (candidateTime !== currentTime) return candidateTime > currentTime;
  return String(candidate.id) > String(current.id);
}

function effectiveByDate(habitId) {
  const map = new Map();
  db.checkinEvents.forEach((event) => {
    if (event.habitId !== habitId) return;
    const current = map.get(event.localDate);
    if (!current || eventNewer(event, current)) map.set(event.localDate, event);
  });
  return map;
}

function statusOn(habitId, localDate, map = effectiveByDate(habitId)) {
  const event = map.get(localDate);
  if (!event || event.status === 'void') return null;
  return event.status;
}

function habitsSorted() {
  return [...db.habits]
    .filter((habit) => !isTombstoned('habit', habit.id) && !habit.archivedAt)
    .sort((a, b) => a.sortOrder - b.sortOrder || a.name.localeCompare(b.name, 'zh-Hans-CN'));
}

// 归档的习惯默认仍进统计，除非在归档区关掉「统计中显示」。
function habitsForStats() {
  return [...db.habits]
    .filter((habit) => !isTombstoned('habit', habit.id))
    .filter((habit) => !habit.archivedAt || habit.statsVisible)
    .sort((a, b) => a.sortOrder - b.sortOrder || a.name.localeCompare(b.name, 'zh-Hans-CN'));
}

function archivedHabits() {
  return [...db.habits]
    .filter((habit) => !isTombstoned('habit', habit.id) && habit.archivedAt)
    .sort((a, b) => String(b.archivedAt).localeCompare(String(a.archivedAt)));
}

function habitById(habitId) {
  const habit = db.habits.find((item) => item.id === habitId) || null;
  if (!habit || isTombstoned('habit', habit.id) || habit.archivedAt) return null;
  return habit;
}

function habitStart(habit) {
  return habit.goalHistory.reduce(
    (earliest, goal) => (goal.effectiveFrom < earliest ? goal.effectiveFrom : earliest),
    habit.goalHistory[0].effectiveFrom,
  );
}

function goalForWeek(habit, weekStart) {
  const key = dateKey(weekStart);
  let effective = habit.goalHistory[0];
  habit.goalHistory.forEach((goal) => {
    if (goal.effectiveFrom <= key) effective = goal;
  });
  return effective;
}

function pendingGoal(habit) {
  return habit.goalHistory
    .filter((goal) => goal.effectiveFrom > TODAY_KEY)
    .sort((a, b) => a.effectiveFrom.localeCompare(b.effectiveFrom))[0] || null;
}

function nextMondayKey() {
  return dateKey(addDays(startOfWeek(DEMO_TODAY), 7));
}

function currentWeekMondayKey() {
  return dateKey(startOfWeek(DEMO_TODAY));
}

function markersFor(habitId) {
  return db.markers
    .filter((marker) => marker.habitId === habitId && !isTombstoned('marker', marker.id))
    .sort((a, b) => (a.localDate < b.localDate ? -1 : a.localDate > b.localDate ? 1 : 0));
}

function markerOn(habitId, localDate) {
  return markersFor(habitId).find((marker) => marker.localDate === localDate) || null;
}

function currentWeekSummary(habit) {
  const weekStart = startOfWeek(DEMO_TODAY);
  const goal = goalForWeek(habit, weekStart);
  const map = effectiveByDate(habit.id);
  let done = 0;
  for (let offset = 0; offset < 7; offset += 1) {
    const key = dateKey(addDays(weekStart, offset));
    if (key > TODAY_KEY) break;
    if (statusOn(habit.id, key, map) === 'done') done += 1;
  }
  const hasTarget = goal.targetMode === 'weekly' || goal.targetMode === 'nightly';
  return { done, target: hasTarget ? goal.weeklyTarget : null };
}

function liveNoteById(id) {
  return liveNotes().find((note) => note.id === id) || null;
}

function pendingReviewHabits() {
  return habitsSorted().filter((habit) => (
    habit.confirmationTiming === 'nextDay' && !statusOn(habit.id, YESTERDAY_KEY)
  ));
}

function sameDayHabits() {
  return habitsSorted().filter((habit) => habit.confirmationTiming === 'sameDay');
}

function accentOf(habit) {
  return HABIT_COLORS[habit.color] || HABIT_COLORS.sage;
}

/* --------------------------------------------------------------------------
   UI state
   -------------------------------------------------------------------------- */

const state = {
  view: 'today',
  collapsed: false,
  hidden: false,
  reviewSnoozed: false,
  reviewEditing: false,
  range: '90',
  customStart: dateKey(addDays(DEMO_TODAY, -29)),
  customEnd: TODAY_KEY,
  pro: false,
  search: '',
  filterTag: null,
  editingId: null,
  localSequence: 0,
  demoClock: new Date(2026, 9, 22, 9, 41, 0),
  quickDraft: '',
  autostart: true,
  appOpen: false,
  appExplained: false,
  statsHabitId: 'all',
  habitForm: null,
  habitConfirm: null,
  markerError: '',
  justCropped: null,
};

const MOOD_BANDS = [
  { max: 19, label: '很低落', color: 'var(--mood-1)', mouth: 'M11 22c1.7-2.4 8.3-2.4 10 0' },
  { max: 39, label: '有点低', color: 'var(--mood-2)', mouth: 'M11.4 21.4c1.8-1.5 7.4-1.5 9.2 0' },
  { max: 59, label: '一般', color: 'var(--mood-3)', mouth: 'M11.3 21h9.4' },
  { max: 79, label: '不错', color: 'var(--mood-4)', mouth: 'M11.4 20.5c1.8 2 7.4 2 9.2 0' },
  { max: 100, label: '很好', color: 'var(--mood-5)', mouth: 'M10.8 20c2.1 3.8 8.3 3.8 10.4 0' },
];

function bandFor(score) {
  return MOOD_BANDS.find((band) => score <= band.max) || MOOD_BANDS[MOOD_BANDS.length - 1];
}

/* Seed once; all rendering below reads from this in-memory db. */
seedReadingCheckins();
seedWalkCheckins();
seedNotes();
seedMoods();

/* --------------------------------------------------------------------------
   DOM references
   -------------------------------------------------------------------------- */

const els = {
  desktop: document.getElementById('desktop'),
  card: document.getElementById('card'),
  cardHead: document.getElementById('cardHead'),
  aboutBtn: document.getElementById('aboutBtn'),
  brandToggle: document.getElementById('brandToggle'),
  miniBar: document.getElementById('miniBar'),
  miniBarHead: document.getElementById('miniBarHead'),
  miniDate: document.getElementById('miniDate'),
  miniStatus: document.getElementById('miniStatus'),
  miniExpand: document.getElementById('miniExpand'),
  miniQuick: document.getElementById('miniQuick'),
  miniFold: document.getElementById('miniFold'),
  strip: document.getElementById('strip'),
  tabs: document.getElementById('tabs'),
  viewToday: document.getElementById('viewToday'),
  viewRecords: document.getElementById('viewRecords'),
  viewStats: document.getElementById('viewStats'),
  dateEyebrow: document.getElementById('dateEyebrow'),
  dateTitle: document.getElementById('dateTitle'),
  reviewZone: document.getElementById('reviewZone'),
  manageHabits: document.getElementById('manageHabits'),
  todayHabit: document.getElementById('todayHabit'),
  moodScale: document.getElementById('moodScale'),
  moodFace: document.getElementById('moodFace'),
  moodName: document.getElementById('moodName'),
  moodValue: document.getElementById('moodValue'),
  moodSlider: document.getElementById('moodSlider'),
  inlineInput: document.getElementById('inlineInput'),
  inlineTagPreview: document.getElementById('inlineTagPreview'),
  inlineSave: document.getElementById('inlineSave'),
  todayCount: document.getElementById('todayCount'),
  todayNotes: document.getElementById('todayNotes'),
  goRecords: document.getElementById('goRecords'),
  recordSearch: document.getElementById('recordSearch'),
  recordsQuick: document.getElementById('recordsQuick'),
  tagFilter: document.getElementById('tagFilter'),
  resultCount: document.getElementById('resultCount'),
  recordsMeta: document.getElementById('recordsMeta'),
  clearFilters: document.getElementById('clearFilters'),
  recordsScroll: document.getElementById('recordsScroll'),
  recordList: document.getElementById('recordList'),
  habitScope: document.getElementById('habitScope'),
  rangeTabs: document.getElementById('rangeTabs'),
  proControl: document.getElementById('proControl'),
  rangeCaption: document.getElementById('rangeCaption'),
  proToggle: document.getElementById('proToggle'),
  customRange: document.getElementById('customRange'),
  rangeStart: document.getElementById('rangeStart'),
  rangeEnd: document.getElementById('rangeEnd'),
  applyRange: document.getElementById('applyRange'),
  rangeError: document.getElementById('rangeError'),
  habitSummary: document.getElementById('habitSummary'),
  proPanel: document.getElementById('proPanel'),
  heatmapPanel: document.getElementById('heatmapPanel'),
  volumePanel: document.getElementById('volumePanel'),
  moodPanel: document.getElementById('moodPanel'),
  tagsPanel: document.getElementById('tagsPanel'),
  weekReviewPanel: document.getElementById('weekReviewPanel'),
  statsScroll: document.getElementById('statsScroll'),
  appToggle: document.getElementById('appToggle'),
  appToggleText: document.getElementById('appToggleText'),
  quickToggle: document.getElementById('quickToggle'),
  resetDemo: document.getElementById('resetDemo'),
  deskApp: document.getElementById('deskApp'),
  appWindow: document.getElementById('appWindow'),
  closeApp: document.getElementById('closeApp'),
  trayPerch: document.getElementById('trayPerch'),
  quickLayer: document.getElementById('quickLayer'),
  quickInput: document.getElementById('quickInput'),
  quickPreview: document.getElementById('quickPreview'),
  quickSave: document.getElementById('quickSave'),
  editLayer: document.getElementById('editLayer'),
  editInput: document.getElementById('editInput'),
  editMeta: document.getElementById('editMeta'),
  editSave: document.getElementById('editSave'),
  habitLayer: document.getElementById('habitLayer'),
  habitTitle: document.getElementById('habitTitle'),
  habitManager: document.getElementById('habitManager'),
  aboutLayer: document.getElementById('aboutLayer'),
  aboutScroll: document.getElementById('aboutScroll'),
  toastRegion: document.getElementById('toastRegion'),
};

const STORAGE = {
  prefix: 'perch.mock.',
  position: 'position',
  collapsed: 'collapsed',
  autostart: 'autostart',
  morning: 'morningReveal',
};

const TASKBAR_HEIGHT = 42;
const VIEWS = ['today', 'records', 'stats'];

/* --------------------------------------------------------------------------
   Small shared helpers
   -------------------------------------------------------------------------- */

function icon(name, className = 'icon') {
  return `<svg class="${className}" aria-hidden="true"><use href="#${name}"/></svg>`;
}

function readStored(key, fallback) {
  try {
    const raw = localStorage.getItem(STORAGE.prefix + key);
    if (raw === null) return fallback;
    return JSON.parse(raw);
  } catch (error) {
    console.warn('读取演示状态失败：', error);
    return fallback;
  }
}

function writeStored(key, value) {
  try {
    localStorage.setItem(STORAGE.prefix + key, JSON.stringify(value));
  } catch (error) {
    console.warn('保存演示状态失败：', error);
  }
}

function clearStored() {
  try {
    Object.keys(localStorage)
      .filter((key) => key.startsWith(STORAGE.prefix))
      .forEach((key) => localStorage.removeItem(key));
  } catch (error) {
    console.warn('清理演示状态失败：', error);
  }
}

function makeId(prefix) {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return `${prefix}_${crypto.randomUUID()}`;
  }
  state.localSequence += 1;
  return `${prefix}_demo_${Date.now().toString(36)}_${state.localSequence}`;
}

function nextDemoIso() {
  state.localSequence += 1;
  return new Date(state.demoClock.getTime() + state.localSequence * 60000).toISOString();
}

function sortNotesDesc(a, b) {
  if (a.localDate !== b.localDate) return a.localDate < b.localDate ? 1 : -1;
  return Date.parse(b.createdAt) - Date.parse(a.createdAt);
}

function countTags(notes) {
  const counts = new Map();
  notes.forEach((note) => {
    note.tags.forEach((tag) => counts.set(tag, (counts.get(tag) || 0) + 1));
  });
  return [...counts.entries()]
    .map(([tag, count]) => ({ tag, count }))
    .sort((a, b) => b.count - a.count || a.tag.localeCompare(b.tag, 'zh-Hans-CN'));
}

function currentMood() {
  return liveMoods().find((mood) => mood.localDate === TODAY_KEY) || null;
}

function habitVars(habit) {
  const accent = accentOf(habit);
  return `--habit-accent:${accent.accent};--habit-wash:${accent.wash}`;
}

function timingLabel(habit) {
  return habit.confirmationTiming === 'nextDay' ? '次日确认' : '当天确认';
}

function goalSentence(habit, goal) {
  const hasTarget = goal.targetMode === 'weekly' || goal.targetMode === 'nightly';
  return hasTarget && goal.weeklyTarget ? `每周 ${goal.weeklyTarget} ${habit.unit}` : '只记录';
}

function weekCountText(habit, week) {
  return week.target === null
    ? `本周 ${week.done} ${habit.unit}`
    : `本周 ${week.done}/${week.target} ${habit.unit}`;
}

function moodFaceSvg(band) {
  return `<svg viewBox="0 0 32 32" aria-hidden="true" focusable="false">
    <circle class="mood-face-fill" cx="16" cy="16" r="12.4" stroke="currentColor" stroke-width="1.25"/>
    <circle cx="12.1" cy="13.4" r="1.05" fill="currentColor" stroke="none"/>
    <circle cx="19.9" cy="13.4" r="1.05" fill="currentColor" stroke="none"/>
    <path d="${band.mouth}" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round"/>
  </svg>`;
}

/* --------------------------------------------------------------------------
   Check-in writes
   -------------------------------------------------------------------------- */

function pushCheckin(habit, status, localDate, source) {
  const previous = effectiveByDate(habit.id).get(localDate) || null;
  if (previous && previous.status === status) return null;
  const confirmedAt = nextDemoIso();
  const event = {
    id: makeId('cke'),
    habitId: habit.id,
    localDate,
    status,
    confirmedAt,
    source,
    supersedesId: previous ? previous.id : null,
    recordedBy: DEVICE_ID,
    note: '',
    createdAt: confirmedAt,
  };
  db.checkinEvents.push(event);
  return event;
}

function saveReviewStatus(habitId, status) {
  const habit = habitById(habitId);
  if (!habit) return;
  pushCheckin(habit, status, YESTERDAY_KEY, 'morningReview');
  state.reviewSnoozed = false;
  state.reviewEditing = false;
  renderToday();
  if (state.view === 'stats') renderStats();
  showToast(`${habit.name}${status === 'done' ? '已完成' : '记为没做'}`);
}

function checkinToday(habitId) {
  const habit = habitById(habitId);
  if (!habit) return;
  pushCheckin(habit, 'done', TODAY_KEY, 'sameDayCheckin');
  state.justCropped = habitId;
  renderToday();
  if (state.view === 'stats') renderStats();
  showToast(`${habit.name}已完成`);
}

function undoCheckin(habitId, localDate) {
  const habit = habitById(habitId);
  if (!habit) return;
  pushCheckin(habit, 'void', localDate, 'undo');
  renderToday();
  if (state.view === 'stats') renderStats();
  showToast(`${habit.name}已撤回`);
}

/* --------------------------------------------------------------------------
   Today — date, morning review, habits, mood, notes
   -------------------------------------------------------------------------- */

function renderDateBlock() {
  els.dateEyebrow.textContent = `${weekdayCN(DEMO_TODAY)} · 第 ${isoWeek(DEMO_TODAY)} 周`;
  els.dateTitle.textContent = fmtCN(DEMO_TODAY);
}

function reviewRowMarkup(habit, selected) {
  return `<div class="review-row">
    <span class="review-row-copy">
      <span class="review-habit-name">${escapeHtml(habit.name)}</span>
      <span class="review-sub">${fmtCNFull(fromKey(YESTERDAY_KEY))}</span>
    </span>
    <span class="review-row-actions">
      <button class="choice is-done" type="button" data-review-action="done" data-review-habit="${habit.id}" aria-pressed="${selected === 'done'}">${icon('i-check')}<span>完成</span></button>
      <button class="choice is-notdone" type="button" data-review-action="notDone" data-review-habit="${habit.id}" aria-pressed="${selected === 'notDone'}"><span>没做</span></button>
    </span>
  </div>`;
}

function reviewResultsMarkup(habits) {
  const items = habits.map((habit) => {
    const status = statusOn(habit.id, YESTERDAY_KEY);
    return `<span class="review-result"><i class="result-dot ${status === 'done' ? 'is-done' : 'is-notdone'}"></i>${escapeHtml(habit.name)} · ${status === 'done' ? '已完成' : '没做'}</span>`;
  }).join('');
  return `<div class="review-results">${items}</div>`;
}

function renderReview() {
  const nextDay = habitsSorted().filter((habit) => habit.confirmationTiming === 'nextDay');
  if (!nextDay.length) {
    els.reviewZone.innerHTML = '';
    return;
  }
  const resolved = nextDay.filter((habit) => statusOn(habit.id, YESTERDAY_KEY));
  const pending = nextDay.filter((habit) => !statusOn(habit.id, YESTERDAY_KEY));

  if (pending.length && state.reviewSnoozed && !state.reviewEditing) {
    els.reviewZone.innerHTML = `<div class="review-card">
      <div class="review-snoozed-row">
        <span class="review-icon">${icon('i-clock')}</span>
        <div class="review-snoozed-copy">
          <span>${pending.length} 项待确认</span>
          <span>${fmtCNFull(fromKey(YESTERDAY_KEY))}</span>
        </div>
        <button class="text-btn" type="button" data-review-action="resume">现在回顾</button>
      </div>
    </div>`;
    return;
  }

  if (pending.length || state.reviewEditing) {
    const rows = (state.reviewEditing ? nextDay : pending)
      .map((habit) => reviewRowMarkup(habit, statusOn(habit.id, YESTERDAY_KEY)))
      .join('');
    els.reviewZone.innerHTML = `<div class="review-card">
      <div class="review-head">
        <p class="review-question">昨天完成了吗？</p>
        <button class="text-btn" type="button" data-review-action="${state.reviewEditing ? 'cancel' : 'later'}">${state.reviewEditing ? '收起' : '稍后'}</button>
      </div>
      <div class="review-list">${rows}</div>
      ${state.reviewEditing && resolved.length ? reviewResultsMarkup(resolved) : ''}
    </div>`;
    return;
  }

  els.reviewZone.innerHTML = '';
}

function habitCardMarkup(habit) {
  const week = currentWeekSummary(habit);
  const map = effectiveByDate(habit.id);
  const todayStatus = statusOn(habit.id, TODAY_KEY, map);
  const nextDay = habit.confirmationTiming === 'nextDay';
  const yesterday = nextDay ? statusOn(habit.id, YESTERDAY_KEY, map) : null;

  let action;
  if (habit.confirmationTiming === 'sameDay') {
    action = todayStatus === 'done'
      ? `<button class="checkin-btn is-done" type="button" data-checkin-undo="${habit.id}" aria-label="撤销${escapeAttr(habit.name)}打卡">已打卡</button>`
      : `<button class="checkin-btn" type="button" data-checkin="${habit.id}">打卡</button>`;
  } else if (yesterday) {
    action = `<button class="text-btn" type="button" data-review-change="${habit.id}">改昨天</button>`;
  } else if (pendingReviewHabits().some((item) => item.id === habit.id)) {
    // 上面的回顾卡已经在问同一件事，这里不再放第二个入口
    action = '';
  } else {
    action = '<span class="habit-state">待确认</span>';
  }

  const yesterdayNote = yesterday === 'done' ? ' · 昨天已确认'
    : yesterday === 'notDone' ? ' · 昨天没做' : '';
  const weekLabel = `${weekCountText(habit, week)}${yesterdayNote}`;

  return `<div class="habit-card" style="${habitVars(habit)}" data-habit="${habit.id}">
    <div class="habit-top">
      <span class="habit-icon">${icon(habit.icon)}</span>
      <span class="habit-name">${escapeHtml(habit.name)}</span>
      ${action}
      <button class="habit-edit" type="button" data-habit-edit="${habit.id}" aria-label="修改${escapeAttr(habit.name)}">${icon('i-pencil')}</button>
    </div>
    <p class="week-label">${escapeHtml(weekLabel)}</p>
  </div>`;
}

function renderHabit() {
  const habits = habitsSorted();
  if (!habits.length) {
    els.todayHabit.innerHTML = `<div class="habit-empty">
      ${icon('i-leaf')}
      <span>还没有习惯。</span>
      <button class="section-action" type="button" data-habit-form="new">${icon('i-plus')}<span>新增习惯</span></button>
    </div>`;
    return;
  }
  els.todayHabit.innerHTML = habits.map(habitCardMarkup).join('');
}

let lastPaintedBand = null;

function paintMood(value, mode) {
  const band = bandFor(value);
  const unset = mode === 'idle';
  els.moodScale.style.setProperty('--mood-band-color', band.color);
  els.moodFace.innerHTML = moodFaceSvg(band);
  els.moodFace.style.color = unset ? '' : band.color;
  els.moodFace.classList.toggle('is-unset', unset);
  if (mode === 'drag' && band.label !== lastPaintedBand) {
    els.moodFace.classList.remove('is-live');
    void els.moodFace.offsetWidth;
    els.moodFace.classList.add('is-live');
  }
  lastPaintedBand = unset ? null : band.label;
  els.moodName.textContent = unset ? '今天还没记' : band.label;
  els.moodValue.textContent = unset ? '—' : `${value}%`;
  els.moodSlider.setAttribute('aria-valuetext', `${value}% · ${band.label}`);
}

function renderMood() {
  const mood = currentMood();
  const value = mood ? mood.score : clamp(Math.round(Number(els.moodSlider.value) || 50), 0, 100);
  if (mood) els.moodSlider.value = String(mood.score);
  paintMood(mood ? mood.score : value, mood ? 'saved' : 'idle');
}

function commitMood(rawValue) {
  const value = clamp(Math.round(Number(rawValue) || 0), 0, 100);
  const existing = currentMood();
  if (existing) {
    existing.score = value;
    existing.updatedAt = nextDemoIso();
  } else {
    db.dailyMoods.push({
      id: makeId('mood'),
      localDate: TODAY_KEY,
      score: value,
      updatedAt: nextDemoIso(),
    });
  }
  paintMood(value, 'saved');
  if (state.view === 'stats') renderStats();
}

function notesForToday() {
  return liveNotes()
    .filter((note) => note.localDate === TODAY_KEY)
    .sort(sortNotesDesc);
}

function renderTodayNotes() {
  const notes = notesForToday();
  els.todayCount.textContent = `${notes.length} 条`;
  if (!notes.length) {
    els.todayNotes.innerHTML = `<div class="empty-soft">
      ${icon('i-leaf')}
      <span>今天还没有记录。</span>
    </div>`;
    return;
  }
  els.todayNotes.innerHTML = notes.map((note) => `<div class="today-note">
    <div class="today-note-meta">
      <span>${timeLabel(note.createdAt)}</span>
      ${note.conflictOf ? '<span class="conflict-badge">同步冲突副本</span>' : ''}
    </div>
    <p class="today-note-text">${renderRichText(note.text)}</p>
  </div>`).join('');
}

/* --------------------------------------------------------------------------
   Folded strip
   -------------------------------------------------------------------------- */

function renderMini() {
  const pending = state.reviewSnoozed ? [] : pendingReviewHabits();
  const openCheckins = sameDayHabits().filter((habit) => !statusOn(habit.id, TODAY_KEY));
  els.miniDate.textContent = `${fmtCN(DEMO_TODAY)} · ${weekdayCN(DEMO_TODAY)}`;
  if (pending.length) els.miniStatus.textContent = `${pending.length} 项待确认`;
  else if (openCheckins.length) els.miniStatus.textContent = `${openCheckins.length} 项今天可记`;
  else els.miniStatus.textContent = '今天已记录';
}

function stripPendingRow(habit) {
  return `<div class="strip-row" style="${habitVars(habit)}">
    <span class="strip-icon">${icon(habit.icon)}</span>
    <span class="strip-name">${escapeHtml(habit.name)}</span>
    <span class="strip-when">昨天</span>
    <span class="strip-actions">
      <button class="strip-btn" type="button" data-strip-action="done" data-strip-habit="${habit.id}">完成</button>
      <button class="strip-btn" type="button" data-strip-action="notDone" data-strip-habit="${habit.id}">没做</button>
    </span>
  </div>`;
}

function stripCheckinRow(habit) {
  return `<button class="strip-row is-action" type="button" style="${habitVars(habit)}" data-strip-checkin="${habit.id}">
    <span class="strip-icon">${icon(habit.icon)}</span>
    <span class="strip-name">${escapeHtml(habit.name)}</span>
    <span class="strip-when">今天</span>
    <span class="strip-cta">打卡</span>
  </button>`;
}

function stripSliver(habit) {
  return `<div class="strip-row is-cropped${state.justCropped === habit.id ? ' is-cropping' : ''}" style="${habitVars(habit)}">
    <span class="strip-name">${escapeHtml(habit.name)}已打卡</span>
    <button class="strip-undo" type="button" data-strip-action="undo" data-strip-habit="${habit.id}" data-strip-date="${TODAY_KEY}" aria-label="撤销${escapeAttr(habit.name)}打卡">${icon('i-undo')}</button>
  </div>`;
}

function renderStrip() {
  const pending = state.reviewSnoozed ? [] : pendingReviewHabits();
  const snoozed = state.reviewSnoozed ? pendingReviewHabits() : [];
  const openCheckins = sameDayHabits().filter((habit) => !statusOn(habit.id, TODAY_KEY));
  const doneToday = sameDayHabits().filter((habit) => statusOn(habit.id, TODAY_KEY) === 'done');

  const rows = [
    ...pending.map(stripPendingRow),
    ...openCheckins.map(stripCheckinRow),
    ...doneToday.map(stripSliver),
  ];

  if (!rows.length) {
    rows.push(snoozed.length
      ? `<div class="strip-row is-empty"><span>昨天 ${snoozed.length} 项待确认</span><button class="strip-btn" type="button" data-strip-action="resume">现在回顾</button></div>`
      : '<div class="strip-row is-empty">今天没有要记的</div>');
  }

  els.strip.innerHTML = rows.join('');
  state.justCropped = null;
}

function renderToday() {
  renderReview();
  renderHabit();
  renderMini();
  renderStrip();
}

function handleReviewAction(action, habitId) {
  if (action === 'done' || action === 'notDone') {
    saveReviewStatus(habitId, action);
    return;
  }
  if (action === 'later') {
    state.reviewSnoozed = true;
    renderToday();
    return;
  }
  if (action === 'resume') {
    state.reviewSnoozed = false;
    renderToday();
    return;
  }
  if (action === 'change') {
    state.reviewEditing = true;
    renderReview();
    return;
  }
  if (action === 'cancel') {
    state.reviewEditing = false;
    renderReview();
  }
}

function handleStripClick(event) {
  const checkin = event.target.closest('[data-strip-checkin]');
  if (checkin) {
    checkinToday(checkin.dataset.stripCheckin);
    return;
  }
  const action = event.target.closest('[data-strip-action]');
  if (!action) return;
  const kind = action.dataset.stripAction;
  if (kind === 'resume') {
    state.reviewSnoozed = false;
    renderToday();
    return;
  }
  const habitId = action.dataset.stripHabit;
  if (kind === 'done' || kind === 'notDone') saveReviewStatus(habitId, kind);
  else if (kind === 'undo') undoCheckin(habitId, action.dataset.stripDate);
}

function handleHabitZoneClick(event) {
  const edit = event.target.closest('[data-habit-edit]');
  if (edit) {
    openHabitForm('edit', edit.dataset.habitEdit);
    return;
  }
  const add = event.target.closest('[data-habit-form="new"]');
  if (add) {
    openHabitForm('new');
    return;
  }
  const checkin = event.target.closest('[data-checkin]');
  if (checkin) {
    checkinToday(checkin.dataset.checkin);
    return;
  }
  const undo = event.target.closest('[data-checkin-undo]');
  if (undo) {
    undoCheckin(undo.dataset.checkinUndo, TODAY_KEY);
    return;
  }
  const change = event.target.closest('[data-review-change]');
  if (change) {
    state.reviewEditing = true;
    renderReview();
    els.reviewZone.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }
}

/* --------------------------------------------------------------------------
   Habit manager — add and edit habits; target changes take effect next Monday
   -------------------------------------------------------------------------- */

function closeHabitLayer() {
  els.habitLayer.hidden = true;
  state.habitForm = null;
}

function backToManagerList() {
  state.habitForm = null;
  state.habitConfirm = null;
  els.habitTitle.textContent = '管理习惯';
  renderHabitManager();
}

function openHabitManager() {
  state.habitForm = null;
  state.habitConfirm = null;
  els.habitTitle.textContent = '管理习惯';
  renderHabitManager();
  els.habitLayer.hidden = false;
}

function openHabitForm(mode, habitId) {
  const habit = mode === 'edit' ? habitById(habitId) : null;
  if (mode === 'edit' && !habit) return;

  const current = habit ? goalForWeek(habit, startOfWeek(DEMO_TODAY)) : null;
  const upcoming = habit ? (pendingGoal(habit) || current) : null;

  state.habitForm = {
    mode,
    habitId: habit ? habit.id : null,
    error: '',
    draft: {
      name: habit ? habit.name : '',
      unit: habit ? habit.unit : '次',
      target: upcoming ? String(upcoming.weeklyTarget || 0) : '3',
      timing: habit ? habit.confirmationTiming : 'sameDay',
      definition: habit ? habit.operationalDefinition : '',
      ifThen: habit ? habit.ifThen : '',
    },
  };

  els.habitTitle.textContent = habit ? `修改「${habit.name}」` : '新增习惯';
  renderHabitManager();
  els.habitLayer.hidden = false;
  window.setTimeout(() => {
    const input = document.getElementById('hfName');
    if (input) input.focus();
  }, 40);
}

function managerRowMarkup(habit) {
  const goal = goalForWeek(habit, startOfWeek(DEMO_TODAY));
  const upcoming = pendingGoal(habit);

  if (state.habitConfirm === habit.id) {
    return `<div class="manager-row is-confirm" style="${habitVars(habit)}">
      <span class="habit-icon">${icon(habit.icon)}</span>
      <span class="manager-copy">
        <strong>移除「${escapeHtml(habit.name)}」？</strong>
        <span>它和它的打卡会从所有地方消失，无法恢复。</span>
      </span>
      <span class="manager-actions">
        <button class="mini-btn" type="button" data-habit-confirm="cancel">取消</button>
        <button class="mini-btn is-danger" type="button" data-habit-remove="${habit.id}">移除</button>
      </span>
    </div>`;
  }

  return `<div class="manager-row" style="${habitVars(habit)}">
    <span class="habit-icon">${icon(habit.icon)}</span>
    <span class="manager-copy">
      <strong>${escapeHtml(habit.name)}</strong>
      <span>${timingLabel(habit)} · ${goalSentence(habit, goal)}</span>
      ${upcoming ? `<span class="manager-pending">${fmtCN(upcoming.effectiveFrom)} 起 ${goalSentence(habit, upcoming)}</span>` : ''}
    </span>
    <span class="manager-actions">
      <button class="manager-edit" type="button" data-habit-edit="${habit.id}" aria-label="修改${escapeAttr(habit.name)}" title="修改">${icon('i-pencil')}</button>
      <button class="manager-edit" type="button" data-habit-archive="${habit.id}" aria-label="归档${escapeAttr(habit.name)}" title="归档">${icon('i-archive')}</button>
      <button class="manager-edit is-remove" type="button" data-habit-confirm="${habit.id}" aria-label="移除${escapeAttr(habit.name)}" title="移除">${icon('i-trash')}</button>
    </span>
  </div>`;
}

function archivedRowMarkup(habit) {
  const archivedKey = String(habit.archivedAt).slice(0, 10);
  return `<div class="manager-row is-archived" style="${habitVars(habit)}">
    <span class="habit-icon">${icon(habit.icon)}</span>
    <span class="manager-copy">
      <strong>${escapeHtml(habit.name)}</strong>
      <span>${fmtCN(archivedKey)} 归档 · 只读</span>
    </span>
    <span class="manager-actions">
      <label class="switch-label" title="统计中显示">
        <span>统计</span>
        <input type="checkbox" data-habit-stats="${habit.id}" ${habit.statsVisible ? 'checked' : ''}>
        <span class="switch" aria-hidden="true"><span class="switch-knob"></span></span>
      </label>
      <button class="manager-edit" type="button" data-habit-restore="${habit.id}" aria-label="恢复${escapeAttr(habit.name)}" title="恢复">${icon('i-undo')}</button>
      <button class="manager-edit is-remove" type="button" data-habit-confirm="${habit.id}" aria-label="移除${escapeAttr(habit.name)}" title="移除">${icon('i-trash')}</button>
    </span>
  </div>`;
}

function habitFormMarkup() {
  const draft = state.habitForm.draft;
  const timingButton = (value, label) => `<button type="button" data-hf-timing="${value}"
    class="${draft.timing === value ? 'is-active' : ''}" aria-pressed="${draft.timing === value}">${label}</button>`;

  return `<form class="habit-form" novalidate>
    <div class="field-row">
      <label class="field"><span>名称</span><input id="hfName" value="${escapeAttr(draft.name)}" maxlength="12" autocomplete="off" placeholder="阅读"></label>
      <label class="field"><span>单位</span><input id="hfUnit" value="${escapeAttr(draft.unit)}" maxlength="3" autocomplete="off" placeholder="晚 / 次"></label>
    </div>
    <div class="field-row is-uneven">
      <label class="field"><span>每周目标 · 0 为只记录</span><input id="hfTarget" type="number" min="0" max="7" step="1" value="${escapeAttr(draft.target)}"></label>
      <span class="field"><span>确认时机</span><span class="segmented" role="group">
        ${timingButton('nextDay', '次日')}
        ${timingButton('sameDay', '当天')}
      </span></span>
    </div>
    <label class="field"><span>一句定义</span><textarea id="hfDef" rows="2" placeholder="怎样算完成一次">${escapeHtml(draft.definition)}</textarea></label>
    <label class="field"><span>if–then</span><input id="hfIfThen" value="${escapeAttr(draft.ifThen)}" placeholder="如果……就……"></label>
    <p class="form-error" id="hfError" aria-live="polite">${escapeHtml(state.habitForm.error)}</p>
    <div class="form-actions">
      <button type="button" class="ghost-btn" data-habit-form="cancel">取消</button>
      <button type="submit" class="primary-btn">保存</button>
    </div>
  </form>`;
}

function renderHabitManager() {
  if (state.habitForm) {
    els.habitManager.innerHTML = habitFormMarkup();
    return;
  }
  const habits = habitsSorted();
  const archived = archivedHabits();
  els.habitManager.innerHTML = `<div class="manager-list">
    ${habits.length ? habits.map(managerRowMarkup).join('') : '<p class="manager-sub">还没有习惯。</p>'}
    <button class="manager-add" type="button" data-habit-form="new">${icon('i-plus')}<span>新增习惯</span></button>
    ${archived.length ? `<div class="manager-archive">
      <span class="manager-archive-title">归档</span>
      ${archived.map(archivedRowMarkup).join('')}
    </div>` : ''}
  </div>`;
}

function refreshAfterHabitChange() {
  if (state.statsHabitId !== 'all' && !habitById(state.statsHabitId)) state.statsHabitId = 'all';
  renderToday();
  if (state.view === 'stats') renderStats();
  renderHabitManager();
}

function archiveHabit(habitId) {
  const habit = db.habits.find((item) => item.id === habitId);
  if (!habit || habit.archivedAt) return;
  habit.archivedAt = nextDemoIso();
  habit.statsVisible = true;
  habit.updatedAt = habit.archivedAt;
  refreshAfterHabitChange();
  showToast(`已归档「${habit.name}」`);
}

function restoreHabit(habitId) {
  const habit = db.habits.find((item) => item.id === habitId);
  if (!habit || !habit.archivedAt) return;
  habit.archivedAt = null;
  habit.updatedAt = nextDemoIso();
  refreshAfterHabitChange();
  showToast(`已恢复「${habit.name}」`);
}

function setHabitStatsVisible(habitId, visible) {
  const habit = db.habits.find((item) => item.id === habitId);
  if (!habit) return;
  habit.statsVisible = visible;
  habit.updatedAt = nextDemoIso();
  if (state.view === 'stats') renderStats();
}

function removeHabit(habitId) {
  const habit = db.habits.find((item) => item.id === habitId);
  if (!habit || isTombstoned('habit', habitId)) return;
  db.tombstones.push({
    entityType: 'habit',
    entityId: habitId,
    deletedAt: nextDemoIso(),
    deletedBy: DEVICE_ID,
  });
  state.habitConfirm = null;
  refreshAfterHabitChange();
  showToast(`已移除「${habit.name}」`);
}

function syncChoice(selector, activeButton) {
  els.habitManager.querySelectorAll(selector).forEach((button) => {
    const active = button === activeButton;
    button.classList.toggle('is-active', active);
    button.setAttribute('aria-pressed', String(active));
  });
}

function readHabitForm() {
  const value = (id) => {
    const node = document.getElementById(id);
    return node ? node.value.trim() : '';
  };
  const draft = state.habitForm.draft;
  return {
    name: value('hfName'),
    unit: value('hfUnit'),
    target: value('hfTarget'),
    definition: value('hfDef'),
    ifThen: value('hfIfThen'),
    timing: draft.timing,
  };
}

function validateHabitForm(values) {
  if (!values.name) return '请填写习惯名称。';
  if (!values.unit) return '请填写单位，例如「晚」或「次」。';
  const target = Number(values.target);
  if (!values.target || !Number.isInteger(target) || target < 0 || target > 7) {
    return '每周目标填 0–7 的整数，0 表示只记录。';
  }
  return '';
}

function saveHabitForm() {
  const values = readHabitForm();
  const error = validateHabitForm(values);
  if (error) {
    state.habitForm.error = error;
    renderHabitManager();
    return;
  }

  const target = Number(values.target);
  const targetMode = target === 0 ? 'recordOnly' : 'weekly';
  const weeklyTarget = targetMode === 'weekly' ? target : null;
  const mode = state.habitForm.mode;
  const stamp = nextDemoIso();

  if (mode === 'new') {
    const identity = habitIdentity(db.habits.length);
    db.habits.push({
      id: makeId('hbt'),
      name: values.name,
      icon: identity.icon,
      color: identity.color,
      operationalDefinition: values.definition,
      unit: values.unit,
      confirmationTiming: values.timing,
      targetMode,
      weeklyTarget,
      ifThen: values.ifThen,
      reminderTime: values.timing === 'nextDay' ? '07:30' : '20:00',
      sortOrder: habitsSorted().reduce((max, item) => Math.max(max, item.sortOrder), 0) + 10,
      archivedAt: null,
      statsVisible: true,
      goalHistory: [{ effectiveFrom: currentWeekMondayKey(), targetMode, weeklyTarget }],
      createdAt: stamp,
      updatedAt: stamp,
    });
  } else {
    const habit = habitById(state.habitForm.habitId);
    if (!habit) return;
    const current = goalForWeek(habit, startOfWeek(DEMO_TODAY));
    habit.name = values.name;
    habit.unit = values.unit;
    habit.confirmationTiming = values.timing;
    habit.operationalDefinition = values.definition;
    habit.ifThen = values.ifThen;
    habit.updatedAt = stamp;

    const changed = current.targetMode !== targetMode || (current.weeklyTarget || 0) !== target;
    const upcoming = pendingGoal(habit);
    if (upcoming) habit.goalHistory = habit.goalHistory.filter((goal) => goal !== upcoming);
    if (changed) habit.goalHistory.push({ effectiveFrom: nextMondayKey(), targetMode, weeklyTarget });
  }

  if (state.statsHabitId !== 'all' && !habitById(state.statsHabitId)) state.statsHabitId = 'all';
  state.habitForm = null;
  els.habitLayer.hidden = true;
  renderToday();
  if (state.view === 'stats') renderStats();

  if (mode === 'new') showToast(`已新增「${values.name}」`);
  else showToast(values.target === '0' ? '已保存 · 只记录' : `已保存 · 目标 ${fmtCN(nextMondayKey())} 起生效`);
}

/* --------------------------------------------------------------------------
   About 栖迟
   -------------------------------------------------------------------------- */

let aboutReturnFocus = null;

function openAbout(section) {
  aboutReturnFocus = document.activeElement;
  els.aboutLayer.hidden = false;
  const target = section ? document.getElementById(`about-${section}`) : null;
  if (!target) {
    els.aboutScroll.scrollTop = 0;
    return;
  }
  window.setTimeout(() => {
    const offset = target.getBoundingClientRect().top
      - els.aboutScroll.getBoundingClientRect().top
      + els.aboutScroll.scrollTop
      - 8;
    els.aboutScroll.scrollTop = Math.max(0, offset);
  }, 40);
}

function closeAbout() {
  els.aboutLayer.hidden = true;
  if (aboutReturnFocus && document.contains(aboutReturnFocus)) aboutReturnFocus.focus();
}

/* --------------------------------------------------------------------------
   Records — timeline, search, tag filter, edit and remove
   -------------------------------------------------------------------------- */

function renderTagFilter() {
  const allNotes = liveNotes();
  const counts = countTags(allNotes);
  const total = allNotes.length;
  els.tagFilter.innerHTML = [
    `<button class="filter-chip ${state.filterTag ? '' : 'is-active'}" type="button" data-tag="">全部<span class="count">${total}</span></button>`,
    ...counts.map(({ tag, count }) => `<button class="filter-chip ${state.filterTag === tag ? 'is-active' : ''}"
      type="button" data-tag="${escapeAttr(tag)}">#${escapeHtml(tag)}<span class="count">${count}</span></button>`),
  ].join('');
  els.tagFilter.classList.toggle('is-clipped', els.tagFilter.scrollWidth > els.tagFilter.clientWidth + 4);
}

function noteMatches(note) {
  const query = state.search.trim().toLowerCase();
  if (query) {
    const haystack = `${note.text} ${note.tags.map((tag) => `#${tag}`).join(' ')}`.toLowerCase();
    if (!haystack.includes(query)) return false;
  }
  if (state.filterTag && !note.tags.includes(state.filterTag)) return false;
  return true;
}

function dateHeading(key) {
  if (key === TODAY_KEY) return `今天 · ${fmtCNFull(fromKey(key))}`;
  if (key === YESTERDAY_KEY) return `昨天 · ${fmtCNFull(fromKey(key))}`;
  const date = fromKey(key);
  const year = date.getFullYear() === DEMO_TODAY.getFullYear() ? '' : `${date.getFullYear()}年`;
  return `${year}${fmtCNFull(date)}`;
}

function renderRecords() {
  renderTagFilter();
  const filtering = Boolean(state.search.trim() || state.filterTag);
  const searching = Boolean(state.search.trim());
  const notes = liveNotes().filter(noteMatches).sort(sortNotesDesc);
  els.clearFilters.hidden = !filtering;
  els.recordsMeta.hidden = !filtering;
  els.resultCount.hidden = !searching;
  els.resultCount.textContent = `搜索到 ${notes.length} 条`;

  if (!notes.length) {
    els.recordList.innerHTML = `<div class="empty-soft records-empty">
      ${icon(filtering ? 'i-search' : 'i-leaf')}
      <span>${filtering ? '没有找到匹配的记录。' : '还没有记录。'}</span>
    </div>`;
    return;
  }

  const groups = new Map();
  notes.forEach((note) => {
    if (!groups.has(note.localDate)) groups.set(note.localDate, []);
    groups.get(note.localDate).push(note);
  });

  els.recordList.innerHTML = [...groups.entries()].map(([key, groupNotes]) => `<section class="record-group">
    <h2 class="record-date"><span>${dateHeading(key)}</span><span class="date-count">${groupNotes.length} 条</span></h2>
    ${groupNotes.map((note) => `<article class="record-item">
      <div class="record-item-head">
        <span class="record-time">${timeLabel(note.createdAt)}</span>
        ${note.conflictOf ? '<span class="conflict-badge">同步冲突副本</span>' : ''}
      </div>
      <p class="record-text">${renderRichText(note.text)}</p>
      <div class="record-tools">
        <button class="tool-btn" type="button" data-edit="${escapeAttr(note.id)}" aria-label="编辑记录">${icon('i-pencil')}</button>
        <button class="tool-btn is-remove" type="button" data-delete="${escapeAttr(note.id)}" aria-label="删除记录">${icon('i-trash')}</button>
      </div>
    </article>`).join('')}
  </section>`).join('');
}

function goRecordsWithTag(tag) {
  state.filterTag = tag;
  state.search = '';
  els.recordSearch.value = '';
  switchView('records');
  els.recordsScroll.scrollTop = 0;
}

function addNote(rawText) {
  const text = rawText.trim();
  if (!text) return null;
  const createdAt = nextDemoIso();
  const note = {
    id: makeId('note'),
    createdAt,
    localDate: TODAY_KEY,
    text,
    tags: parseTags(text),
    updatedAt: createdAt,
    conflictOf: null,
  };
  db.notes.push(note);
  return note;
}

function afterNoteChange() {
  renderTodayNotes();
  if (state.view === 'records') renderRecords();
  if (state.view === 'stats') renderStats();
}

function openEdit(noteId) {
  const note = liveNoteById(noteId);
  if (!note) return;
  state.editingId = noteId;
  els.editInput.value = note.text;
  els.editMeta.textContent = `${fmtCNFull(fromKey(note.localDate))} · ${timeLabel(note.createdAt)}`;
  els.editSave.disabled = !note.text.trim();
  els.editLayer.hidden = false;
  window.setTimeout(() => {
    els.editInput.focus();
    els.editInput.setSelectionRange(els.editInput.value.length, els.editInput.value.length);
  }, 30);
}

function closeEdit() {
  els.editLayer.hidden = true;
  state.editingId = null;
}

function saveEdit() {
  const note = state.editingId ? liveNoteById(state.editingId) : null;
  const text = els.editInput.value.trim();
  if (!note || !text) return;
  note.text = text;
  note.tags = parseTags(text);
  note.updatedAt = nextDemoIso();
  closeEdit();
  afterNoteChange();
  showToast('记录已更新');
}

function removeNote(noteId) {
  const note = liveNoteById(noteId);
  if (!note) return;
  const tombstone = {
    entityType: 'note',
    entityId: note.id,
    deletedAt: nextDemoIso(),
    deletedBy: DEVICE_ID,
  };
  db.tombstones.push(tombstone);
  if (state.editingId === note.id) closeEdit();
  afterNoteChange();
  showToast('已移除这条记录', {
    actionLabel: '撤销',
    duration: 5200,
    onAction: () => {
      const index = db.tombstones.indexOf(tombstone);
      if (index >= 0) db.tombstones.splice(index, 1);
      afterNoteChange();
      showToast('记录已恢复');
    },
  });
}

/* --------------------------------------------------------------------------
   Toasts
   -------------------------------------------------------------------------- */

function showToast(message, options = {}) {
  const toast = document.createElement('div');
  toast.className = 'toast';

  const mark = document.createElement('span');
  mark.innerHTML = icon('i-leaf');
  toast.appendChild(mark);

  const copy = document.createElement('span');
  copy.textContent = message;
  toast.appendChild(copy);

  let dismissed = false;
  let timer = null;
  const dismiss = () => {
    if (dismissed) return;
    dismissed = true;
    window.clearTimeout(timer);
    toast.classList.add('is-leaving');
    window.setTimeout(() => toast.remove(), 220);
  };

  if (options.actionLabel && typeof options.onAction === 'function') {
    const action = document.createElement('button');
    action.type = 'button';
    action.className = 'toast-action';
    action.textContent = options.actionLabel;
    action.addEventListener('click', () => {
      dismiss();
      options.onAction();
    });
    toast.appendChild(action);
  }

  els.toastRegion.appendChild(toast);
  while (els.toastRegion.children.length > 3) {
    els.toastRegion.firstElementChild.remove();
  }

  timer = window.setTimeout(dismiss, options.duration || 4200);
  toast.addEventListener('mouseenter', () => window.clearTimeout(timer));
  toast.addEventListener('mouseleave', () => {
    if (dismissed) return;
    window.clearTimeout(timer);
    timer = window.setTimeout(dismiss, 1800);
  });
  return dismiss;
}

/* --------------------------------------------------------------------------
   Stats — range, completion, heatmap, volume, mood, tags, weekly review
   -------------------------------------------------------------------------- */

function rangeSpec() {
  const today = DEMO_TODAY;
  if (state.range === '7') {
    const startDate = addDays(today, -6);
    return { startDate, endDate: today, startKey: dateKey(startDate), endKey: TODAY_KEY, days: 7 };
  }
  if (state.range === '30') {
    const startDate = addDays(today, -29);
    return { startDate, endDate: today, startKey: dateKey(startDate), endKey: TODAY_KEY, days: 30 };
  }
  if (state.range === 'month') {
    const startDate = new Date(today.getFullYear(), today.getMonth(), 1, 12, 0, 0);
    return {
      startDate,
      endDate: today,
      startKey: dateKey(startDate),
      endKey: TODAY_KEY,
      days: diffDays(startDate, today) + 1,
    };
  }
  if (state.range === 'lastMonth') {
    const startDate = new Date(today.getFullYear(), today.getMonth() - 1, 1, 12, 0, 0);
    const endDate = new Date(today.getFullYear(), today.getMonth(), 0, 12, 0, 0);
    return {
      startDate,
      endDate,
      startKey: dateKey(startDate),
      endKey: dateKey(endDate),
      days: diffDays(startDate, endDate) + 1,
    };
  }
  if (state.range === 'custom') {
    const startDate = fromKey(state.customStart);
    const endDate = fromKey(state.customEnd);
    return {
      startDate,
      endDate,
      startKey: state.customStart,
      endKey: state.customEnd,
      days: diffDays(startDate, endDate) + 1,
    };
  }
  const startDate = addDays(today, -89);
  return { startDate, endDate: today, startKey: dateKey(startDate), endKey: TODAY_KEY, days: 90 };
}

function mean(values) {
  if (!values.length) return null;
  return values.reduce((sum, value) => sum + value, 0) / values.length;
}

function standardDeviation(values) {
  if (values.length < 2) return null;
  const avg = mean(values);
  const variance = values.reduce((sum, value) => sum + ((value - avg) ** 2), 0) / values.length;
  return Math.sqrt(variance);
}

function habitAggregate(habit, spec) {
  const map = effectiveByDate(habit.id);
  const startKey = spec.startKey > habitStart(habit) ? spec.startKey : habitStart(habit);
  const endKey = spec.endKey < YESTERDAY_KEY ? spec.endKey : YESTERDAY_KEY;
  let done = 0;
  let eligible = 0;

  if (startKey <= endKey) {
    const start = fromKey(startKey);
    eligible = diffDays(start, fromKey(endKey)) + 1;
    for (let offset = 0; offset < eligible; offset += 1) {
      if (statusOn(habit.id, dateKey(addDays(start, offset)), map) === 'done') done += 1;
    }
  }

  return { habit, map, done, eligible };
}

function computeStats(spec) {
  const allPerHabit = habitsForStats().map((habit) => habitAggregate(habit, spec));
  const scopeHabit = state.statsHabitId === 'all' ? null : habitById(state.statsHabitId);
  const perHabit = scopeHabit
    ? allPerHabit.filter((item) => item.habit.id === scopeHabit.id)
    : allPerHabit;

  const totals = perHabit.reduce((acc, item) => ({
    done: acc.done + item.done,
    eligible: acc.eligible + item.eligible,
  }), { done: 0, eligible: 0 });

  const notes = liveNotes()
    .filter((note) => note.localDate >= spec.startKey && note.localDate <= spec.endKey)
    .sort(sortNotesDesc);
  const moods = liveMoods()
    .filter((mood) => mood.localDate >= spec.startKey && mood.localDate <= spec.endKey)
    .sort((a, b) => a.localDate.localeCompare(b.localDate));

  const timeBuckets = { morning: 0, day: 0, evening: 0, night: 0 };
  notes.forEach((note) => {
    const hour = new Date(note.createdAt).getHours();
    if (hour >= 5 && hour < 12) timeBuckets.morning += 1;
    else if (hour >= 12 && hour < 18) timeBuckets.day += 1;
    else if (hour >= 18 && hour < 23) timeBuckets.evening += 1;
    else timeBuckets.night += 1;
  });

  return {
    spec,
    scopeHabit,
    scopeAll: !scopeHabit,
    perHabit,
    allPerHabit,
    totals,
    rate: totals.eligible ? Math.round((totals.done / totals.eligible) * 100) : null,
    notes,
    moods,
    avgMood: mean(moods.map((mood) => mood.score)),
    topTags: countTags(notes),
    timeBuckets,
  };
}

function renderScope() {
  const habits = habitsSorted();
  const chip = (value, label, dotStyle) => `<button class="scope-chip ${state.statsHabitId === value ? 'is-active' : ''}"
    type="button" data-scope="${value}" aria-pressed="${state.statsHabitId === value}">${dotStyle ? `<span class="scope-dot" style="${dotStyle}"></span>` : ''}${escapeHtml(label)}</button>`;

  els.habitScope.innerHTML = [
    chip('all', '全部', ''),
    ...habits.map((habit) => chip(habit.id, habit.name, `--habit-accent:${accentOf(habit).accent}`)),
  ].join('');
}

function heroHabitRow(item) {
  const { habit, done, eligible } = item;
  return `<span class="hero-habit-row" style="${habitVars(habit)}">
    <span class="hero-habit-icon">${icon(habit.icon)}</span>
    <span class="hero-habit-name">${escapeHtml(habit.name)}</span>
    <span class="hero-habit-unit">${eligible ? `${done}/${eligible}` : '—'}</span>
  </span>`;
}

function renderHero(stats) {
  const rateMarkup = stats.rate === null ? '—' : `${stats.rate}<span>%</span>`;
  const countMarkup = stats.totals.eligible ? `${stats.totals.done}/${stats.totals.eligible}` : '—';

  let right = '';
  if (!stats.perHabit.length) right = '<div class="hero-right"><span class="hero-empty">还没有习惯</span></div>';
  else if (stats.perHabit.length > 1) {
    right = `<div class="hero-right hero-habit-list">
      ${stats.perHabit.map((item) => heroHabitRow(item)).join('')}
    </div>`;
  }

  els.habitSummary.innerHTML = `<div class="panel-head">
    <h3>完成率</h3>
  </div>
  <div class="hero-layout${right ? '' : ' is-solo'}">
    <div class="hero-left">
      <div class="hero-rate">${rateMarkup}</div>
      <p class="hero-count">${countMarkup}</p>
    </div>
    ${right}
  </div>`;
}

function weeklyHistory(habit) {
  const map = effectiveByDate(habit.id);
  const firstWeek = startOfWeek(fromKey(habitStart(habit)));
  const lastWeek = startOfWeek(DEMO_TODAY);
  const weeks = [];
  const yesterday = fromKey(YESTERDAY_KEY);

  for (let weekStart = firstWeek; diffDays(weekStart, lastWeek) >= 0; weekStart = addDays(weekStart, 7)) {
    const startKey = dateKey(weekStart);
    const weekEnd = addDays(weekStart, 6);
    const countedEnd = weekEnd > yesterday ? yesterday : weekEnd;
    const elapsed = Math.max(0, diffDays(weekStart, countedEnd) + 1);
    let done = 0;
    for (let offset = 0; offset < elapsed; offset += 1) {
      if (statusOn(habit.id, dateKey(addDays(weekStart, offset)), map) === 'done') done += 1;
    }
    weeks.push({
      startDate: weekStart,
      startKey,
      elapsed,
      done,
      rate: elapsed ? (done / elapsed) * 100 : 0,
    });
  }
  return weeks;
}

// 分段由标记切出：每个标记是一条竖线，两段均值直接可比。没有标记就没有分段。
function segmentsFor(habit, weeks) {
  if (!weeks.length) return [];
  const markers = markersFor(habit.id).filter((marker) => marker.localDate > weeks[0].startKey);
  const cuts = [weeks[0].startKey, ...markers.map((marker) => marker.localDate)];
  return cuts
    .map((from, index) => {
      const to = cuts[index + 1] || null;
      const rows = weeks.filter((week) => week.startKey >= from && (to === null || week.startKey < to));
      return {
        from,
        to,
        label: index === 0 ? '还没标记之前' : markers[index - 1].label,
        markerId: index === 0 ? null : markers[index - 1].id,
        markerDate: index === 0 ? null : markers[index - 1].localDate,
        rates: rows.map((row) => row.rate),
        count: rows.length,
      };
    })
    .filter((segment) => segment.count > 0);
}

function proChartSvg(weeks, habit, segments) {
  const width = 480;
  const height = 112;
  const left = 8;
  const right = 472;
  const top = 10;
  const bottom = 100;
  const span = right - left;
  const xAt = (index) => left + (weeks.length <= 1 ? span / 2 : (index / (weeks.length - 1)) * span);
  const yAt = (rate) => bottom - (rate / 100) * (bottom - top);
  const accent = accentOf(habit);
  const columnWidth = weeks.length > 1 ? span / (weeks.length - 1) : span;
  const indexAtKey = (key) => {
    const found = weeks.findIndex((week) => week.startKey >= key);
    return found < 0 ? null : found;
  };

  const bands = segments
    .map((segment, index) => {
      if (index % 2 === 0) return '';
      const from = indexAtKey(segment.from);
      const to = segment.to ? indexAtKey(segment.to) : weeks.length;
      if (from === null || to === null) return '';
      const bandLeft = clamp(xAt(from) - columnWidth / 2, left, right);
      const bandRight = clamp(xAt(to - 1) + columnWidth / 2, left, right);
      return `<rect class="seg-band" x="${bandLeft.toFixed(1)}" y="${top}" width="${(bandRight - bandLeft).toFixed(1)}" height="${bottom - top}"/>`;
    })
    .join('');

  const markerLines = segments
    .filter((segment) => segment.markerDate)
    .map((segment) => {
      const index = indexAtKey(segment.markerDate);
      if (index === null) return '';
      const x = xAt(index).toFixed(1);
      return `<line class="marker-line" x1="${x}" y1="${top - 4}" x2="${x}" y2="${bottom}" stroke-width="1"/>`;
    })
    .join('');

  const goal = goalForWeek(habit, startOfWeek(DEMO_TODAY));
  const hasTarget = (goal.targetMode === 'weekly' || goal.targetMode === 'nightly') && goal.weeklyTarget;
  const targetY = hasTarget ? yAt((goal.weeklyTarget / 7) * 100) : null;
  const points = weeks.map((week, index) => `${xAt(index).toFixed(1)},${yAt(week.rate).toFixed(1)}`);
  const line = points.map((point, index) => `${index === 0 ? 'M' : 'L'}${point}`).join(' ');
  const area = `${line} L${xAt(weeks.length - 1).toFixed(1)},${bottom} L${xAt(0).toFixed(1)},${bottom} Z`;
  const dots = weeks.map((week, index) => `<circle class="line-dot" cx="${xAt(index).toFixed(1)}" cy="${yAt(week.rate).toFixed(1)}" r="${index === weeks.length - 1 ? 3 : 2.2}" stroke="${accent.accent}" stroke-width="1.5"/>`).join('');

  const grid = [0, 50, 100].map((tick) => {
    const y = yAt(tick).toFixed(1);
    return `<line class="grid-line" x1="${left}" y1="${y}" x2="${right}" y2="${y}" stroke-width="1"/>`;
  }).join('');

  return `<svg class="line-chart" viewBox="0 0 ${width} ${height}" role="img" aria-label="${escapeAttr(`${habit.name}每周完成率与标记分段`)}">
    ${bands}
    ${grid}
    ${targetY === null ? '' : `<line class="target-line" x1="${left}" y1="${targetY.toFixed(1)}" x2="${right}" y2="${targetY.toFixed(1)}" stroke-width="1" stroke-dasharray="3 4"/>`}
    <path d="${area}" fill="${accent.wash}" opacity=".55"/>
    <path d="${line}" fill="none" stroke="${accent.accent}" stroke-width="1.6" stroke-linejoin="round" stroke-linecap="round"/>
    ${dots}
    ${markerLines}
  </svg>`;
}

function addMarker(habitId, localDate, label) {
  const stamp = nextDemoIso();
  db.markers.push({
    id: makeId('mrk'),
    habitId,
    localDate,
    label,
    createdAt: stamp,
    updatedAt: stamp,
  });
}

function removeMarker(markerId) {
  const marker = db.markers.find((item) => item.id === markerId);
  if (!marker || isTombstoned('marker', markerId)) return;
  db.tombstones.push({
    entityType: 'marker',
    entityId: markerId,
    deletedAt: nextDemoIso(),
    deletedBy: DEVICE_ID,
  });
}

function handleMarkerSubmit(event) {
  event.preventDefault();
  const habit = habitById(state.statsHabitId);
  if (!habit) return;
  const localDate = document.getElementById('mkDate').value.trim();
  const label = document.getElementById('mkLabel').value.trim();

  if (!/^\d{4}-\d{2}-\d{2}$/.test(localDate)) state.markerError = '请选择日期。';
  else if (!label) state.markerError = '写一句话，说明从这天起改了什么。';
  else if (localDate < habitStart(habit)) state.markerError = `这个习惯最早到 ${fmtCN(habitStart(habit))}。`;
  else if (localDate > TODAY_KEY) state.markerError = '不能标到未来。';
  else if (markerOn(habit.id, localDate)) state.markerError = '这天已经有标记了。';
  else {
    state.markerError = '';
    addMarker(habit.id, localDate, label);
  }
  renderStats();
  if (!state.markerError) showToast('已标记');
}

function renderPro(stats) {
  if (stats.scopeAll || !state.pro) {
    els.proPanel.hidden = true;
    els.proPanel.innerHTML = '';
    return;
  }

  const habit = stats.scopeHabit;
  const weeks = weeklyHistory(habit);
  const segments = segmentsFor(habit, weeks);
  const volatility = standardDeviation(weeks.map((week) => week.rate));
  const last = segments.length ? segments[segments.length - 1] : null;
  const prev = segments.length > 1 ? segments[segments.length - 2] : null;
  const lastMean = last ? mean(last.rates) : null;
  const prevMean = prev ? mean(prev.rates) : null;
  const diff = lastMean !== null && prevMean !== null ? lastMean - prevMean : null;
  const diffLabel = diff === null
    ? '只有一个分段，先标记一个变化日期'
    : `较上一段 ${diff >= 0 ? '+' : '−'}${Math.abs(diff).toFixed(1)} pp`;
  const formatPercent = (value) => (value === null ? '—' : `${Math.round(value)}%`);
  const goal = goalForWeek(habit, startOfWeek(DEMO_TODAY));

  const segmentRows = segments.map((segment) => {
    const endKey = segment.to ? dateKey(addDays(fromKey(segment.to), -1)) : YESTERDAY_KEY;
    return `<div class="pro-segment">
      <div class="seg-copy">
        <span class="seg-label">${escapeHtml(segment.label)}</span>
        <span class="seg-range">${fmtCN(segment.from)} – ${fmtCN(endKey)} · ${segment.count} 周</span>
      </div>
      <strong class="seg-mean">${formatPercent(mean(segment.rates))}</strong>
      ${segment.markerId ? `<button class="seg-del" type="button" data-marker-del="${segment.markerId}" aria-label="删除标记 ${escapeAttr(segment.label)}">${icon('i-trash')}</button>` : ''}
    </div>`;
  }).join('');

  els.proPanel.innerHTML = `<div class="panel-head">
    <div>
      <span class="panel-kicker">专业视图 · 分段对比</span>
      <h3>${escapeHtml(habit.name)}</h3>
    </div>
    <span class="pro-badge">描述性</span>
  </div>
  <div class="pro-chart-wrap">${weeks.length ? proChartSvg(weeks, habit, segments) : ''}</div>
  ${weeks.length ? `<p class="chart-axis">每点一周 · 纵轴为该周完成率，横线是 0 / 50 / 100% · ${fmtCN(weeks[0].startKey)} 至 ${fmtCN(weeks[weeks.length - 1].startKey)}</p>` : ''}
  ${goal.weeklyTarget || segments.some((s) => s.markerId) ? `<div class="chart-legend">
      ${goal.weeklyTarget ? `<span><i class="legend-dash"></i>目标 ${goal.weeklyTarget} ${habit.unit}/周</span>` : ''}
      ${segments.some((s) => s.markerId) ? '<span><i class="legend-vline"></i>标记起点</span>' : ''}
    </div>` : ''}
  <div class="pro-segments">${segmentRows}</div>
  <form class="marker-form" id="markerForm" novalidate>
    <label class="field"><span>日期</span><input type="date" id="mkDate" min="${habitStart(habit)}" max="${TODAY_KEY}" value="${TODAY_KEY}"></label>
    <label class="field"><span>从这天起改成了什么</span><input type="text" id="mkLabel" maxlength="20" autocomplete="off" placeholder="换回纸质书"></label>
    <button type="submit" class="ghost-btn">标记</button>
  </form>
  <p class="form-error" id="mkError" aria-live="polite">${escapeHtml(state.markerError || '')}</p>
  <div class="pro-trend">
    <span>最近一段均值 <strong>${formatPercent(lastMean)}</strong></span>
    <span class="tiny-chip">${escapeHtml(diffLabel)}</span>
    <span class="tiny-chip">波动 ${volatility === null ? '—' : `${volatility.toFixed(0)} pp`}</span>
  </div>`;

  els.proPanel.hidden = false;
}

function heatLevel(done, confirmed, total) {
  if (!confirmed) return 'is-pending';
  if (done === total) return 'is-l2';
  if (done) return 'is-l1';
  return 'is-l0';
}

function statusWord(status) {
  if (status === 'done') return '已完成';
  if (status === 'notDone') return '没做';
  return '未确认';
}

function heatCell(key, spec, stats, options = {}) {
  const date = fromKey(key);
  const classes = ['hm-cell'];
  let description = '';

  if (options.outside || key < spec.startKey || key > spec.endKey) {
    classes.push('is-outside');
    description = '不在当前范围';
  } else if (key > TODAY_KEY) {
    classes.push('is-future');
    description = '还没到';
  } else if (stats.scopeAll) {
    const active = stats.allPerHabit.filter((item) => key >= habitStart(item.habit));
    if (!active.length) {
      classes.push('is-before');
      description = '习惯还没开始';
    } else {
      const statuses = active.map((item) => statusOn(item.habit.id, key, item.map));
      const done = statuses.filter((status) => status === 'done').length;
      const confirmed = statuses.filter(Boolean).length;
      classes.push(heatLevel(done, confirmed, active.length));
      if (key === TODAY_KEY) classes.push('is-today');
      description = `${done}/${active.length} 完成 · ${active
        .map((item, index) => `${item.habit.name} ${statusWord(statuses[index])}`).join(' · ')}`;
    }
  } else {
    const habit = stats.scopeHabit;
    const map = effectiveByDate(habit.id);
    const status = statusOn(habit.id, key, map);
    if (key < habitStart(habit)) {
      classes.push('is-before');
      description = '习惯还没开始';
    } else {
      if (status === 'done') classes.push('is-l2');
      else if (status === 'notDone') classes.push('is-l0');
      else classes.push(goalForWeek(habit, startOfWeek(date)).targetMode === 'recordOnly' ? 'is-before' : 'is-pending');
      if (key === TODAY_KEY) classes.push('is-today');
      description = statusWord(status);
    }
  }

  return `<span class="${classes.join(' ')}" title="${escapeAttr(`${fmtCNFull(date)} · ${description}`)}"></span>`;
}

function heatColumns(spec) {
  const last = fromKey(spec.endKey);
  if (spec.days <= 35) {
    const columns = [];
    for (let date = fromKey(spec.startKey); diffDays(date, last) >= 0; date = addDays(date, 1)) {
      columns.push({ startKey: dateKey(date), endKey: dateKey(date) });
    }
    return columns;
  }
  const columns = [];
  for (let week = startOfWeek(fromKey(spec.startKey)); diffDays(week, last) >= 0; week = addDays(week, 7)) {
    const weekEnd = addDays(week, 6);
    columns.push({
      startKey: dateKey(week) < spec.startKey ? spec.startKey : dateKey(week),
      endKey: dateKey(weekEnd) > spec.endKey ? spec.endKey : dateKey(weekEnd),
    });
  }
  return columns;
}

function habitMatrixRow(item, columns) {
  const { habit, map } = item;
  const cells = columns.map((column) => {
    let done = 0;
    let confirmed = 0;
    let eligible = 0;
    for (let date = fromKey(column.startKey); diffDays(date, fromKey(column.endKey)) >= 0; date = addDays(date, 1)) {
      const key = dateKey(date);
      if (key < habitStart(habit) || key > TODAY_KEY) continue;
      eligible += 1;
      const status = statusOn(habit.id, key, map);
      if (status) confirmed += 1;
      if (status === 'done') done += 1;
    }
    const cls = eligible ? heatLevel(done, confirmed, eligible) : 'is-before';
    const text = eligible ? `${done}/${eligible} ${habit.unit}` : '还没开始';
    return `<span class="hm-mcell ${cls}" title="${escapeAttr(`${fmtCN(fromKey(column.startKey))} – ${fmtCN(fromKey(column.endKey))} · ${text}`)}"></span>`;
  }).join('');

  return `<div class="hm-mrow" style="${habitVars(habit)}">
    <span class="hm-mlabel">${escapeHtml(habit.name)}</span>
    <span class="hm-mcells">${cells}</span>
  </div>`;
}

function habitMatrixMarkup(stats, spec) {
  const columns = heatColumns(spec);
  const first = fromKey(columns[0].startKey);
  const last = fromKey(columns[columns.length - 1].endKey);
  return `<div class="hm-matrix">
    <div class="hm-mrow is-axis">
      <span class="hm-mlabel"></span>
      <span class="hm-maxis"><span>${fmtCN(first)}</span><span>${fmtCN(last)}</span></span>
    </div>
    ${stats.allPerHabit.map((item) => habitMatrixRow(item, columns)).join('')}
  </div>`;
}

function heatmapLegend() {
  const ramp = `<span class="legend-item"><i class="legend-swatch l0"></i><i class="legend-swatch l1"></i><i class="legend-swatch l2"></i>完成</span>`;
  return `<div class="heatmap-legend">
    ${ramp}
    <span class="legend-item"><i class="legend-swatch today"></i>今天</span>
  </div>`;
}

function renderHeatmap(stats) {
  const spec = stats.spec;
  let body = '';

  if (spec.days <= 8) {
    const cells = [];
    for (let date = fromKey(spec.startKey); diffDays(date, fromKey(spec.endKey)) >= 0; date = addDays(date, 1)) {
      const key = dateKey(date);
      cells.push(`<div class="hm-strip-day">${heatCell(key, spec, stats)}
        <span class="hm-strip-label">${weekdayCN(date)}</span>
      </div>`);
    }
    body = `<div class="hm-strip">${cells.join('')}</div>`;
  } else if (spec.days <= 35) {
    const cells = [];
    const gridStart = startOfWeek(fromKey(spec.startKey));
    const gridEnd = endOfWeek(fromKey(spec.endKey));
    for (let date = gridStart; diffDays(date, gridEnd) >= 0; date = addDays(date, 1)) {
      const key = dateKey(date);
      cells.push(heatCell(key, spec, stats, { outside: key < spec.startKey || key > spec.endKey }));
    }
    body = `<div class="hm-calendar-head">
      ${['一', '二', '三', '四', '五', '六', '日'].map((day) => `<span>${day}</span>`).join('')}
    </div>
    <div class="hm-calendar">${cells.join('')}</div>`;
  } else {
    const cells = [];
    const gridStart = startOfWeek(fromKey(spec.startKey));
    const gridEnd = endOfWeek(fromKey(spec.endKey));
    for (let date = gridStart; diffDays(date, gridEnd) >= 0; date = addDays(date, 1)) {
      const key = dateKey(date);
      cells.push(heatCell(key, spec, stats, { outside: key < spec.startKey || key > spec.endKey }));
    }
    body = `<div class="heatmap-weekly">
      <div class="hm-weekdays">${['一', '二', '三', '四', '五', '六', '日'].map((day) => `<span>${day}</span>`).join('')}</div>
      <div class="hm-weeks">${cells.join('')}</div>
    </div>`;
  }

  els.heatmapPanel.style.setProperty(
    '--hm-hue',
    stats.scopeHabit ? accentOf(stats.scopeHabit).accent : 'var(--sage-deep)',
  );

  els.heatmapPanel.innerHTML = `<div class="panel-head">
    <h3>习惯热力图</h3>
    <span class="aside">${spec.days} 天</span>
  </div>
  ${heatmapLegend()}
  <div class="heatmap-wrap">${body}</div>
  ${stats.scopeAll && stats.allPerHabit.length ? habitMatrixMarkup(stats, spec) : ''}`;
}

function volumeBuckets(stats) {
  const { spec, notes } = stats;
  const daily = spec.days <= 31;
  const buckets = [];
  if (daily) {
    for (let date = fromKey(spec.startKey); diffDays(date, fromKey(spec.endKey)) >= 0; date = addDays(date, 1)) {
      const key = dateKey(date);
      const count = notes.filter((note) => note.localDate === key).length;
      buckets.push({ label: fmtCN(date), title: `${fmtCNFull(date)} · ${count} 条`, count });
    }
  } else {
    let cursor = fromKey(spec.startKey);
    while (diffDays(cursor, fromKey(spec.endKey)) >= 0) {
      const sliceEnd = addDays(cursor, 6);
      const cappedEnd = dateKey(sliceEnd) < spec.endKey ? sliceEnd : fromKey(spec.endKey);
      const startKey = dateKey(cursor);
      const endKey = dateKey(cappedEnd);
      const count = notes.filter((note) => note.localDate >= startKey && note.localDate <= endKey).length;
      buckets.push({ label: fmtCN(cursor), title: `${fmtCN(cursor)}–${fmtCN(cappedEnd)} · ${count} 条`, count });
      cursor = addDays(cursor, 7);
    }
  }
  return { daily, buckets };
}

function renderVolume(stats) {
  const { daily, buckets } = volumeBuckets(stats);
  const total = stats.notes.length;
  const max = Math.max(1, ...buckets.map((bucket) => bucket.count));
  const average = daily ? total / stats.spec.days : total / (stats.spec.days / 7);
  const bars = buckets.map((bucket) => {
    const height = bucket.count ? Math.max(6, Math.round((bucket.count / max) * 100)) : 4;
    return `<span class="volume-col" title="${escapeAttr(bucket.title)}">
      <span class="volume-bar ${bucket.count ? '' : 'is-zero'}" style="height:${height}%"></span>
    </span>`;
  }).join('');

  const labelIndexes = buckets.length <= 1 ? [0] : [0, Math.floor((buckets.length - 1) / 2), buckets.length - 1];
  const labels = [...new Set(labelIndexes)]
    .map((index) => `<span>${buckets[index].label}</span>`).join('');

  els.volumePanel.innerHTML = `<div class="panel-head">
    <h3>记录量</h3>
    <span class="aside">${daily ? '按天' : '按周'}</span>
  </div>
  <div class="volume-summary"><strong>${total}</strong><span>条记录 · 平均每${daily ? '天' : '周'} ${total ? average.toFixed(1) : '0'} 条</span></div>
  ${total ? `<div class="volume-chart">${bars}</div>
  <div class="chart-labels">${labels}</div>` : `<div class="empty-soft">${icon('i-leaf')}<span>这个区间还没有记录。</span></div>`}`;
}

function moodChart(stats) {
  const { moods } = stats;
  const width = 480;
  const height = 108;
  const left = 8;
  const right = 472;
  const top = 10;
  const bottom = 96;
  const span = right - left;
  const rangeStart = fromKey(stats.spec.startKey);
  const totalDays = stats.spec.days;
  const xFor = (key) => {
    if (totalDays <= 1) return left + span / 2;
    return left + (diffDays(rangeStart, fromKey(key)) / (totalDays - 1)) * span;
  };
  const yFor = (score) => bottom - (clamp(score, 0, 100) / 100) * (bottom - top);

  const pointData = moods.map((mood) => ({ mood, x: xFor(mood.localDate), y: yFor(mood.score) }));

  let dailyPath = '';
  let previous = null;
  pointData.forEach((point) => {
    const command = previous && diffDays(fromKey(previous.mood.localDate), fromKey(point.mood.localDate)) <= 2 ? 'L' : 'M';
    dailyPath += `${command}${point.x.toFixed(1)},${point.y.toFixed(1)} `;
    previous = point;
  });

  let movingPath = '';
  pointData.forEach((point, index) => {
    const windowPoints = pointData.filter((candidate) => {
      const gap = diffDays(fromKey(candidate.mood.localDate), fromKey(point.mood.localDate));
      return gap >= 0 && gap <= 6;
    });
    const avg = mean(windowPoints.map((candidate) => candidate.mood.score));
    movingPath += `${index === 0 ? 'M' : 'L'}${point.x.toFixed(1)},${yFor(avg).toFixed(1)} `;
  });

  const grid = [0, 50, 100].map((tick) => {
    const y = yFor(tick).toFixed(1);
    return `<line class="grid-line" x1="${left}" y1="${y}" x2="${right}" y2="${y}" stroke-width="1"/>`;
  }).join('');

  const dots = pointData.map((point) => `<circle class="mood-dot" cx="${point.x.toFixed(1)}" cy="${point.y.toFixed(1)}" r="2"/>`).join('');
  return `<svg class="line-chart" viewBox="0 0 ${width} ${height}" role="img" aria-label="区间内每日心情趋势">
    ${grid}
    <path class="mood-avg" d="${movingPath}" fill="none" stroke-width="1.4" stroke-dasharray="3 4"/>
    <path class="mood-daily" d="${dailyPath}" fill="none" stroke-width="1.5" stroke-linejoin="round" stroke-linecap="round"/>
    ${dots}
  </svg>`;
}

function moodGroup(status) {
  if (status === 'done') return 'done';
  if (status === 'notDone') return 'notDone';
  return null;
}

function moodGroupsForDay(stats, key) {
  if (stats.scopeAll) {
    const statuses = stats.allPerHabit
      .filter((item) => key >= habitStart(item.habit) && key <= YESTERDAY_KEY)
      .map((item) => statusOn(item.habit.id, key, item.map));
    if (statuses.includes('done')) return 'done';
    if (statuses.includes('notDone')) return 'notDone';
    return null;
  }
  return moodGroup(statusOn(stats.scopeHabit.id, key, effectiveByDate(stats.scopeHabit.id)));
}

function renderMoodPanel(stats) {
  const n = stats.moods.length;
  const avg = stats.avgMood;

  const doneScores = [];
  const lowScores = [];
  stats.moods.forEach((mood) => {
    const group = moodGroupsForDay(stats, mood.localDate);
    if (group === 'done') doneScores.push(mood.score);
    else if (group === 'notDone') lowScores.push(mood.score);
  });
  const doneAvg = mean(doneScores);
  const lowAvg = mean(lowScores);
  const canCompare = doneScores.length >= 4 && lowScores.length >= 4;

  const compareHtml = canCompare
    ? `<div class="mood-compare">
        <div class="mood-compare-title">完成日与没做日<span>描述性</span></div>
        <div class="compare-row">
          <span>完成日</span>
          <span class="compare-track"><span class="compare-fill" style="width:${doneAvg}%"></span></span>
          <span class="compare-value">${doneAvg.toFixed(0)}%</span>
        </div>
        <div class="compare-row">
          <span>没做日</span>
          <span class="compare-track"><span class="compare-fill is-low" style="width:${lowAvg}%"></span></span>
          <span class="compare-value">${lowAvg.toFixed(0)}%</span>
        </div>
        <p class="compare-note">样本：完成 ${doneScores.length} 天 / 没做 ${lowScores.length} 天。</p>
      </div>`
    : '';

  const chartHtml = n >= 2
    ? `${moodChart(stats)}
      <div class="chart-legend">
        <span><i class="legend-line"></i>每日心情</span>
        <span><i class="legend-dash"></i>7 日平均</span>
      </div>`
    : `<div class="empty-soft">${icon('i-leaf')}<span>这个区间的心情记录还不足以画趋势。</span></div>`;

  els.moodPanel.innerHTML = `<div class="panel-head">
    <h3>心情趋势</h3>
    <div class="mood-headline"><strong>${avg === null ? '—' : Math.round(avg)}</strong><span>% · ${n} 天</span></div>
  </div>
  ${chartHtml}
  ${compareHtml}`;
}

function renderTags(stats) {
  const tags = stats.topTags.slice(0, 5);
  const max = tags.length ? tags[0].count : 1;
  const bars = tags.map((tag) => `<div class="tag-bar-row">
    <span class="tag-bar-label"><span class="hash">#</span>${escapeHtml(tag.tag)}</span>
    <span class="tag-bar-track"><span class="tag-bar-fill" style="width:${Math.max(6, (tag.count / max) * 100)}%"></span></span>
    <span class="tag-bar-count">${tag.count}</span>
  </div>`).join('');

  const buckets = stats.timeBuckets;
  const total = stats.notes.length;
  const order = [
    { key: 'morning', label: '早晨 5–11' },
    { key: 'day', label: '白天 12–17' },
    { key: 'evening', label: '傍晚 18–22' },
    { key: 'night', label: '深夜 23–4' },
  ];
  const segments = order
    .filter((item) => buckets[item.key] > 0)
    .map((item) => `<span class="time-seg ${item.key}" style="width:${(buckets[item.key] / Math.max(1, total)) * 100}%"></span>`)
    .join('');
  const legend = order.map((item) => `<span><i class="time-dot ${item.key}"></i>${item.label} · ${buckets[item.key]}</span>`).join('');

  els.tagsPanel.innerHTML = `<div class="panel-head">
    <h3>常常写到</h3>
    <span class="aside">${total} 条记录里</span>
  </div>
  ${tags.length ? `<div class="tag-bars">${bars}</div>` : `<div class="empty-soft">${icon('i-tag')}<span>这个区间还没有带标签的记录。</span></div>`}
  ${total ? `<div class="time-block">
    <span class="time-block-title">记录的时段分布</span>
    <div class="time-strip">${segments}</div>
    <div class="time-legend">${legend}</div>
  </div>` : ''}`;
}

function lastWeekReview() {
  const weekStart = addDays(startOfWeek(DEMO_TODAY), -7);
  const weekEnd = addDays(weekStart, 6);
  const startKey = dateKey(weekStart);
  const endKey = dateKey(weekEnd);
  const habits = state.statsHabitId === 'all'
    ? habitsSorted()
    : [habitById(state.statsHabitId)].filter(Boolean);
  const unit = habits.length === 1 ? habits[0].unit : '项';

  const rows = habits.map((habit) => {
    const map = effectiveByDate(habit.id);
    const goal = goalForWeek(habit, weekStart);
    const hasTarget = (goal.targetMode === 'weekly' || goal.targetMode === 'nightly') && goal.weeklyTarget;
    let done = 0;
    for (let offset = 0; offset < 7; offset += 1) {
      if (statusOn(habit.id, dateKey(addDays(weekStart, offset)), map) === 'done') done += 1;
    }
    return {
      habit,
      done,
      target: hasTarget ? goal.weeklyTarget : null,
      met: hasTarget ? done >= goal.weeklyTarget : null,
    };
  });

  const notes = liveNotes().filter((note) => note.localDate >= startKey && note.localDate <= endKey);
  const moodScores = liveMoods()
    .filter((mood) => mood.localDate >= startKey && mood.localDate <= endKey)
    .map((mood) => mood.score);
  const previousStart = dateKey(addDays(weekStart, -7));
  const previousScores = liveMoods()
    .filter((mood) => mood.localDate >= previousStart && mood.localDate < startKey)
    .map((mood) => mood.score);

  return {
    weekStart,
    weekEnd,
    startKey,
    endKey,
    rows,
    unit,
    notesCount: notes.length,
    topTag: countTags(notes)[0] || null,
    moodAvg: mean(moodScores),
    moodCount: moodScores.length,
    previousMoodAvg: mean(previousScores),
  };
}

function weekReviewText(review) {
  if (!review.rows.length) return '还没有习惯可以回顾。';

  const habitPart = `${review.rows.map((row) => (row.target === null
    ? `${row.habit.name} ${row.done} ${row.habit.unit}`
    : `${row.habit.name} ${row.done}/${row.target} ${row.habit.unit}`)).join('；')}。`;

  const notePart = review.notesCount
    ? `写下 ${review.notesCount} 条记录${review.topTag ? `，最常写到 #${review.topTag.tag}（${review.topTag.count} 次）` : ''}。`
    : '这一周没有记录。';

  let moodPart = '';
  if (review.moodAvg !== null) {
    moodPart = `心情平均 ${Math.round(review.moodAvg)}%（${review.moodCount} 天有记录）`;
    if (review.previousMoodAvg !== null) {
      const diff = review.moodAvg - review.previousMoodAvg;
      moodPart += Math.abs(diff) >= 1
        ? `，比再前一周${diff > 0 ? '高' : '低'} ${Math.abs(diff).toFixed(0)} 个百分点`
        : '，与再前一周相近';
    }
    moodPart += '。';
  }

  return `${habitPart}${notePart}${moodPart}`;
}

function renderWeekReview() {
  const review = lastWeekReview();
  els.weekReviewPanel.innerHTML = `<div class="week-review-title">
    <div>
      <h3>上周回顾</h3>
      <p>${fmtCN(review.weekStart)} – ${fmtCN(review.weekEnd)}</p>
    </div>
  </div>
  <p class="week-review-body">${escapeHtml(weekReviewText(review))}</p>
  <div class="week-review-foot">
    <button class="export-btn" type="button" data-week-export>${icon('i-export')}导出 Markdown</button>
  </div>`;
}

function exportWeekReview() {
  const review = lastWeekReview();
  const lines = [
    '# 栖迟 · 上周回顾',
    '',
    `区间：${fmtCN(review.weekStart)} – ${fmtCN(review.weekEnd)}`,
    `范围：${state.statsHabitId === 'all' ? '全部习惯' : review.rows[0].habit.name}`,
    '',
    '## 数据摘要',
    '',
    ...review.rows.map((row) => `- ${row.habit.name}：已确认完成 ${row.done} ${row.habit.unit}${row.target === null ? '（只记录）' : `，当时目标 ${row.target} ${row.habit.unit}`}`),
    `- 记录：${review.notesCount} 条`,
    review.topTag ? `- 最常写到的标签：#${review.topTag.tag}（${review.topTag.count} 次）` : '- 最常写到的标签：无',
    review.moodAvg !== null ? `- 心情平均：${Math.round(review.moodAvg)}%（${review.moodCount} 天有记录）` : '- 心情平均：无足够记录',
    '',
    '## 回顾',
    '',
    weekReviewText(review),
    '',
    '> 由栖迟按固定规则生成，内容只有完成、目标差距、记录量、标签与心情。',
    '',
  ];
  const blob = new Blob([lines.join('\n')], { type: 'text/markdown;charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = `栖迟-上周回顾-${review.startKey}.md`;
  document.body.appendChild(link);
  link.click();
  link.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 1200);
  showToast('已导出上周回顾（Markdown）');
}

function renderStats() {
  const spec = rangeSpec();
  const stats = computeStats(spec);
  renderScope();
  els.rangeTabs.querySelectorAll('.range-tab').forEach((button) => {
    const active = button.dataset.range === state.range;
    button.classList.toggle('is-active', active);
    button.setAttribute('aria-pressed', String(active));
  });
  els.rangeCaption.textContent = `${fmtCN(spec.startDate)} – ${fmtCN(spec.endDate)}`;
  els.customRange.hidden = state.range !== 'custom';
  els.proToggle.checked = state.pro;
  els.proToggle.disabled = stats.scopeAll;
  els.proControl.classList.toggle('is-disabled', stats.scopeAll);

  renderHero(stats);
  renderPro(stats);
  renderHeatmap(stats);
  renderVolume(stats);
  renderMoodPanel(stats);
  renderTags(stats);
  renderWeekReview();
}

/* --------------------------------------------------------------------------
   View switching, folding, dragging
   -------------------------------------------------------------------------- */

function switchView(view) {
  if (!VIEWS.includes(view)) return;
  state.view = view;
  els.card.dataset.view = view;
  els.tabs.querySelectorAll('.tab').forEach((tab) => {
    const active = tab.dataset.view === view;
    tab.classList.toggle('is-active', active);
    tab.setAttribute('aria-selected', String(active));
  });
  els.viewToday.hidden = view !== 'today';
  els.viewRecords.hidden = view !== 'records';
  els.viewStats.hidden = view !== 'stats';
  if (view === 'records') renderRecords();
  if (view === 'stats') renderStats();
  syncScrollFade();
}

function setCollapsed(collapsed) {
  state.collapsed = collapsed;
  els.card.classList.toggle('is-collapsed', collapsed);
  els.miniBar.hidden = !collapsed;
  writeStored(STORAGE.collapsed, collapsed);
  if (collapsed) {
    renderMini();
    renderStrip();
  }
  syncScrollFade();
  window.requestAnimationFrame(clampCardPosition);
}

function setStripVisible(visible) {
  els.strip.hidden = !visible;
  const label = visible ? '收起长条' : '展开长条';
  els.miniFold.title = label;
  els.miniFold.setAttribute('aria-label', label);
  els.miniFold.innerHTML = icon(visible ? 'i-chevron-up' : 'i-chevron-down');
  window.requestAnimationFrame(clampCardPosition);
}

function setHidden(hidden) {
  state.hidden = hidden;
  els.card.classList.toggle('is-hidden', hidden);
  if (hidden) els.card.classList.remove('is-raised');
  els.trayPerch.setAttribute('aria-pressed', String(!hidden));
}

function raiseCard(force) {
  const raised = force === undefined ? !els.card.classList.contains('is-raised') : force;
  els.card.classList.toggle('is-raised', raised);
  return raised;
}

function positionOfCard() {
  const rect = els.card.getBoundingClientRect();
  const desk = els.desktop.getBoundingClientRect();
  return { x: rect.left - desk.left, y: rect.top - desk.top };
}

function applyPosition(position) {
  els.card.style.left = `${Math.round(position.x)}px`;
  els.card.style.top = `${Math.round(position.y)}px`;
  els.card.style.right = 'auto';
  els.card.style.bottom = 'auto';
}

function clampPosition(position) {
  const desk = els.desktop.getBoundingClientRect();
  const rect = els.card.getBoundingClientRect();
  const width = rect.width || 340;
  const height = Math.min(rect.height || 300, desk.height - TASKBAR_HEIGHT - 16);
  return {
    x: clamp(position.x, 8, Math.max(8, desk.width - width - 8)),
    y: clamp(position.y, 8, Math.max(8, desk.height - height - TASKBAR_HEIGHT - 8)),
  };
}

function clampCardPosition() {
  if (!els.card.style.left) return;
  applyPosition(clampPosition(positionOfCard()));
}

let drag = null;
let justDragged = false;
const DRAG_THRESHOLD = 4;

function beginDrag(event) {
  if (event.button !== 0) return;
  justDragged = false;
  const onToggle = event.target.closest('#brandToggle, #miniExpand');
  if (!onToggle && event.target.closest('button, input, textarea, a, label')) return;
  const rect = els.card.getBoundingClientRect();
  drag = {
    offsetX: event.clientX - rect.left,
    offsetY: event.clientY - rect.top,
    startX: event.clientX,
    startY: event.clientY,
    armed: !onToggle,
    moved: false,
  };
  if (drag.armed) {
    els.card.classList.add('is-dragging');
    event.preventDefault();
  }
}

function moveDrag(event) {
  if (!drag) return;
  if (!drag.armed) {
    if (Math.abs(event.clientX - drag.startX) < DRAG_THRESHOLD
      && Math.abs(event.clientY - drag.startY) < DRAG_THRESHOLD) return;
    drag.armed = true;
    els.card.classList.add('is-dragging');
    event.preventDefault();
  }
  const desk = els.desktop.getBoundingClientRect();
  const next = clampPosition({
    x: event.clientX - desk.left - drag.offsetX,
    y: event.clientY - desk.top - drag.offsetY,
  });
  applyPosition(next);
  drag.moved = true;
}

function endDrag() {
  if (!drag) return;
  justDragged = drag.moved;
  if (drag.moved) writeStored(STORAGE.position, positionOfCard());
  els.card.classList.remove('is-dragging');
  drag = null;
}

/* --------------------------------------------------------------------------
   Tray menu
   -------------------------------------------------------------------------- */

let trayMenu = null;

function closeTrayMenu() {
  if (!trayMenu) return;
  trayMenu.remove();
  trayMenu = null;
}

function trayMenuMarkup() {
  const raised = els.card.classList.contains('is-raised');
  const row = (action, label, hint, options = {}) => `<button type="button" role="menuitem" data-tray="${action}"
    class="${options.quiet ? 'is-quiet' : ''}"><span>${label}</span>
    <span class="${options.check ? 'menu-check' : 'menu-hint'}">${options.check ? (options.on ? '✓' : '') : hint}</span></button>`;

  return `<div class="tray-menu-head">栖迟 · 桌面层</div>
    ${row('visible', state.hidden ? '显示卡片' : '隐藏卡片', state.hidden ? '已隐藏' : '贴于桌面')}
    ${row('collapsed', state.collapsed ? '展开卡片' : '折叠卡片', state.collapsed ? '已折叠' : '完整视图')}
    ${row('quick', '快速记录', 'Ctrl+Alt+N')}
    <div class="menu-sep"></div>
    ${row('raise', '临时提到前台', '', { check: true, on: raised })}
    ${row('autostart', '开机自启', '', { check: true, on: state.autostart })}
    ${row('about', '关于栖迟', '说明与口径')}
    <div class="menu-sep"></div>
    ${row('quit', '退出栖迟', '演示', { quiet: true })}`;
}

function renderTrayMenu() {
  if (trayMenu) trayMenu.innerHTML = trayMenuMarkup();
}

function toggleTrayMenu() {
  if (trayMenu) {
    closeTrayMenu();
    return;
  }
  trayMenu = document.createElement('div');
  trayMenu.className = 'tray-menu';
  trayMenu.setAttribute('role', 'menu');
  trayMenu.innerHTML = trayMenuMarkup();
  trayMenu.addEventListener('click', (event) => {
    const item = event.target.closest('[data-tray]');
    if (!item) return;
    handleTrayAction(item.dataset.tray);
  });
  els.desktop.appendChild(trayMenu);
}

function handleTrayAction(action) {
  if (action === 'visible') {
    setHidden(!state.hidden);
    renderTrayMenu();
    return;
  }
  if (action === 'collapsed') {
    setCollapsed(!state.collapsed);
    renderTrayMenu();
    return;
  }
  if (action === 'quick') {
    closeTrayMenu();
    openQuick();
    return;
  }
  if (action === 'raise') {
    raiseCard();
    renderTrayMenu();
    return;
  }
  if (action === 'autostart') {
    state.autostart = !state.autostart;
    writeStored(STORAGE.autostart, state.autostart);
    renderTrayMenu();
    return;
  }
  if (action === 'about') {
    closeTrayMenu();
    openAbout(null);
    return;
  }
  closeTrayMenu();
  showToast('演示原型不会真的退出：关掉这个页面就好。');
}

/* --------------------------------------------------------------------------
   Quick capture and inline composer
   -------------------------------------------------------------------------- */

function previewTags(host, text) {
  const tags = parseTags(text);
  host.innerHTML = tags.map((tag) => `<span class="tag-inline">#${escapeHtml(tag)}</span>`).join('');
}

function openQuick() {
  els.quickInput.value = state.quickDraft;
  previewTags(els.quickPreview, els.quickInput.value);
  els.quickSave.disabled = !els.quickInput.value.trim();
  els.quickLayer.hidden = false;
  window.setTimeout(() => els.quickInput.focus(), 30);
}

function closeQuick() {
  state.quickDraft = els.quickInput.value;
  els.quickLayer.hidden = true;
}

function saveQuick() {
  const text = els.quickInput.value.trim();
  if (!text) return;
  addNote(text);
  els.quickInput.value = '';
  state.quickDraft = '';
  previewTags(els.quickPreview, '');
  els.quickSave.disabled = true;
  afterNoteChange();
  showToast('已记下');
}

function syncInlineComposer() {
  previewTags(els.inlineTagPreview, els.inlineInput.value);
  els.inlineSave.disabled = !els.inlineInput.value.trim();
}

function saveInlineNote() {
  const text = els.inlineInput.value.trim();
  if (!text) return;
  addNote(text);
  els.inlineInput.value = '';
  syncInlineComposer();
  afterNoteChange();
  showToast('已记下');
}

/* --------------------------------------------------------------------------
   Simulated other application window
   -------------------------------------------------------------------------- */

function setAppOpen(open) {
  state.appOpen = open;
  els.appWindow.hidden = !open;
  els.appToggle.setAttribute('aria-pressed', String(open));
  els.appToggleText.textContent = open ? '关掉模拟应用' : '模拟应用盖住';
  if (!open) els.card.classList.remove('is-raised');
  if (open && !state.appExplained) {
    state.appExplained = true;
    showToast('卡片贴在桌面层，所以这个窗口会盖住它。', {
      actionLabel: '临时提到前台',
      duration: 6400,
      onAction: () => raiseCard(true),
    });
  }
}

/* --------------------------------------------------------------------------
   Range controls and morning nudge
   -------------------------------------------------------------------------- */

function selectRange(range) {
  state.range = range;
  renderStats();
  if (range === 'custom') {
    els.rangeStart.focus();
  }
}

function applyCustomRange() {
  const start = els.rangeStart.value;
  const end = els.rangeEnd.value;
  if (!start || !end) {
    els.rangeError.textContent = '请选择起止日期。';
    return;
  }
  if (start > end) {
    els.rangeError.textContent = '开始日期不能晚于结束日期。';
    return;
  }
  if (start < READING_START_KEY) {
    els.rangeError.textContent = `最早到 ${fmtCN(fromKey(READING_START_KEY))}，再往前没有数据。`;
    return;
  }
  state.customStart = start;
  state.customEnd = end;
  state.range = 'custom';
  els.rangeError.textContent = '';
  renderStats();
}

function maybeMorningNudge() {
  const pending = pendingReviewHabits();
  if (!pending.length) return;
  if (readStored(STORAGE.morning, null) === TODAY_KEY) return;
  writeStored(STORAGE.morning, TODAY_KEY);
  showToast(`早安。昨天还有 ${pending.length} 个习惯待确认。`, {
    duration: 8000,
    actionLabel: '现在回顾',
    onAction: () => {
      state.reviewSnoozed = false;
      state.reviewEditing = false;
      if (state.collapsed) setCollapsed(false);
      switchView('today');
      renderToday();
      els.reviewZone.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    },
  });
}

function resetDemo() {
  clearStored();
  window.location.reload();
}

/* --------------------------------------------------------------------------
   Event wiring
   -------------------------------------------------------------------------- */

function handleHabitManagerClick(event) {
  const timing = event.target.closest('[data-hf-timing]');
  if (timing) {
    state.habitForm.draft.timing = timing.dataset.hfTiming;
    syncChoice('[data-hf-timing]', timing);
    return;
  }
  if (event.target.closest('[data-habit-form="cancel"]')) {
    backToManagerList();
    return;
  }
  if (event.target.closest('[data-habit-confirm="cancel"]')) {
    state.habitConfirm = null;
    renderHabitManager();
    return;
  }
  const remove = event.target.closest('[data-habit-remove]');
  if (remove) {
    removeHabit(remove.dataset.habitRemove);
    return;
  }
  const confirm = event.target.closest('[data-habit-confirm]');
  if (confirm) {
    state.habitConfirm = confirm.dataset.habitConfirm;
    renderHabitManager();
    return;
  }
  const archive = event.target.closest('[data-habit-archive]');
  if (archive) {
    archiveHabit(archive.dataset.habitArchive);
    return;
  }
  const restore = event.target.closest('[data-habit-restore]');
  if (restore) {
    restoreHabit(restore.dataset.habitRestore);
    return;
  }
  if (event.target.closest('[data-habit-form="new"]')) {
    openHabitForm('new');
    return;
  }
  const edit = event.target.closest('[data-habit-edit]');
  if (edit) openHabitForm('edit', edit.dataset.habitEdit);
}

function handleStatsClick(event) {
  const markerDel = event.target.closest('[data-marker-del]');
  if (markerDel) {
    removeMarker(markerDel.dataset.markerDel);
    state.markerError = '';
    renderStats();
    showToast('已删除标记');
    return;
  }
  const scope = event.target.closest('[data-scope]');
  if (scope) {
    if (state.statsHabitId === scope.dataset.scope) return;
    state.statsHabitId = scope.dataset.scope;
    if (state.statsHabitId === 'all') state.pro = false;
    renderStats();
    return;
  }
  const rangeTab = event.target.closest('.range-tab');
  if (rangeTab) {
    selectRange(rangeTab.dataset.range);
    return;
  }
  if (event.target.closest('[data-week-export]')) exportWeekReview();
}

function handleRecordListClick(event) {
  const edit = event.target.closest('[data-edit]');
  if (edit) {
    openEdit(edit.dataset.edit);
    return;
  }
  const remove = event.target.closest('[data-delete]');
  if (remove) removeNote(remove.dataset.delete);
}

function handleGlobalClick(event) {
  const tagJump = event.target.closest('[data-tag-jump]');
  if (tagJump) {
    goRecordsWithTag(tagJump.dataset.tagJump);
    return;
  }
  if (trayMenu && !event.target.closest('.tray-menu') && !event.target.closest('#trayPerch')) {
    closeTrayMenu();
  }
}

function handleKeydown(event) {
  if (event.ctrlKey && event.altKey && (event.key === 'n' || event.key === 'N')) {
    event.preventDefault();
    if (!els.quickLayer.hidden) closeQuick();
    else openQuick();
    return;
  }
  if (event.key !== 'Escape') return;
  if (!els.aboutLayer.hidden) {
    closeAbout();
    return;
  }
  if (!els.habitLayer.hidden) {
    if (state.habitForm) backToManagerList();
    else closeHabitLayer();
    return;
  }
  if (!els.quickLayer.hidden) {
    closeQuick();
    return;
  }
  if (!els.editLayer.hidden) {
    closeEdit();
    return;
  }
  if (trayMenu) {
    closeTrayMenu();
    return;
  }
  if (state.appOpen) setAppOpen(false);
}

function bindEvents() {
  els.brandToggle.addEventListener('click', () => {
    if (justDragged) return;
    setCollapsed(true);
  });
  els.aboutBtn.addEventListener('click', () => openAbout(null));
  els.miniExpand.addEventListener('click', () => {
    if (justDragged) return;
    setCollapsed(false);
  });
  els.miniFold.addEventListener('click', () => setStripVisible(els.strip.hidden));
  els.miniQuick.addEventListener('click', openQuick);
  els.recordsQuick.addEventListener('click', openQuick);
  els.card.addEventListener('scroll', syncScrollFade, true);
  new MutationObserver(syncScrollFade).observe(els.card, { childList: true, subtree: true, characterData: true });

  els.tabs.addEventListener('click', (event) => {
    const tab = event.target.closest('.tab');
    if (tab) switchView(tab.dataset.view);
  });

  els.reviewZone.addEventListener('click', (event) => {
    const button = event.target.closest('[data-review-action]');
    if (button) handleReviewAction(button.dataset.reviewAction, button.dataset.reviewHabit);
  });
  els.todayHabit.addEventListener('click', handleHabitZoneClick);
  els.manageHabits.addEventListener('click', openHabitManager);
  els.strip.addEventListener('click', handleStripClick);

  els.moodSlider.addEventListener('input', (event) => paintMood(Number(event.target.value), 'drag'));
  els.moodSlider.addEventListener('change', (event) => commitMood(event.target.value));

  els.inlineInput.addEventListener('input', syncInlineComposer);
  els.inlineInput.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      saveInlineNote();
    }
  });
  els.inlineSave.addEventListener('click', saveInlineNote);
  els.goRecords.addEventListener('click', () => {
    state.filterTag = null;
    state.search = '';
    els.recordSearch.value = '';
    switchView('records');
  });

  els.recordSearch.addEventListener('input', (event) => {
    state.search = event.target.value;
    renderRecords();
  });
  els.tagFilter.addEventListener('click', (event) => {
    const chip = event.target.closest('[data-tag]');
    if (!chip) return;
    state.filterTag = chip.dataset.tag || null;
    renderRecords();
  });
  els.clearFilters.addEventListener('click', () => {
    state.search = '';
    state.filterTag = null;
    els.recordSearch.value = '';
    renderRecords();
  });
  els.recordList.addEventListener('click', handleRecordListClick);

  els.habitManager.addEventListener('click', handleHabitManagerClick);
  els.habitManager.addEventListener('change', (event) => {
    const toggle = event.target.closest('[data-habit-stats]');
    if (toggle) setHabitStatsVisible(toggle.dataset.habitStats, toggle.checked);
  });
  els.habitManager.addEventListener('submit', (event) => {
    event.preventDefault();
    saveHabitForm();
  });
  document.querySelectorAll('[data-close-habits]').forEach((node) => {
    node.addEventListener('click', closeHabitLayer);
  });
  document.querySelectorAll('[data-close-about]').forEach((node) => {
    node.addEventListener('click', closeAbout);
  });

  els.habitScope.addEventListener('click', handleStatsClick);
  els.rangeTabs.addEventListener('click', handleStatsClick);
  els.weekReviewPanel.addEventListener('click', handleStatsClick);
  els.proPanel.addEventListener('click', handleStatsClick);
  els.proPanel.addEventListener('submit', handleMarkerSubmit);
  els.proToggle.addEventListener('change', (event) => {
    state.pro = event.target.checked;
    if (state.pro && state.statsHabitId === 'all') {
      const first = habitsSorted()[0];
      if (first) state.statsHabitId = first.id;
    }
    renderStats();
  });
  els.applyRange.addEventListener('click', applyCustomRange);

  document.querySelectorAll('[data-close-quick]').forEach((node) => {
    node.addEventListener('click', closeQuick);
  });
  els.quickInput.addEventListener('input', (event) => {
    state.quickDraft = event.target.value;
    previewTags(els.quickPreview, event.target.value);
    els.quickSave.disabled = !event.target.value.trim();
  });
  els.quickInput.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      saveQuick();
    }
  });
  els.quickSave.addEventListener('click', saveQuick);

  document.querySelectorAll('[data-close-edit]').forEach((node) => {
    node.addEventListener('click', closeEdit);
  });
  els.editInput.addEventListener('input', (event) => {
    els.editSave.disabled = !event.target.value.trim();
  });
  els.editSave.addEventListener('click', saveEdit);

  els.trayPerch.addEventListener('click', (event) => {
    event.stopPropagation();
    toggleTrayMenu();
  });
  els.appToggle.addEventListener('click', () => setAppOpen(!state.appOpen));
  els.quickToggle.addEventListener('click', openQuick);
  els.resetDemo.addEventListener('click', resetDemo);
  els.deskApp.addEventListener('click', () => setAppOpen(true));
  els.closeApp.addEventListener('click', () => setAppOpen(false));

  els.cardHead.addEventListener('pointerdown', beginDrag);
  els.miniBarHead.addEventListener('pointerdown', beginDrag);
  window.addEventListener('pointermove', moveDrag);
  window.addEventListener('pointerup', endDrag);
  window.addEventListener('pointercancel', endDrag);
  window.addEventListener('resize', clampCardPosition);
  document.addEventListener('click', handleGlobalClick);
  window.addEventListener('keydown', handleKeydown);
}

/* --------------------------------------------------------------------------
   Boot
   -------------------------------------------------------------------------- */

/* 底部淡出：当前视图还能往下翻时，给一条「下面还有」的信号 */
function syncScrollFade() {
  const sc = els.card.querySelector('.view:not([hidden]) .scroll');
  const more = Boolean(sc) && !state.collapsed && sc.scrollTop + sc.clientHeight < sc.scrollHeight - 8;
  els.card.classList.toggle('has-more', more);
}

function init() {
  els.rangeStart.value = state.customStart;
  els.rangeEnd.value = state.customEnd;
  state.autostart = readStored(STORAGE.autostart, true);

  const storedPosition = readStored(STORAGE.position, null);
  if (storedPosition) applyPosition(clampPosition(storedPosition));

  renderDateBlock();
  renderToday();
  renderMood();
  renderTodayNotes();
  syncInlineComposer();
  switchView('today');
  renderStats();
  setCollapsed(readStored(STORAGE.collapsed, false));
  setStripVisible(true);
  bindEvents();
  syncScrollFade();
  maybeMorningNudge();
}

init();




