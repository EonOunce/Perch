/* 标签切换（DESIGN §2.1、§2.2）。
   这里只说「现在看哪个标签」：窗口该变多大由 Rust 的 card_set_view 决定，
   尺寸表如果在 JS 里再抄一份，就会出现「标签是记录、窗口还是今天的尺寸」这类对不上的状态。 */

const views = {
  current: 'today',
  hooks: {},
  seq: 0,
  queue: Promise.resolve(),
};

const TAB_BUTTONS = { today: 'tabToday', records: 'tabRecords', stats: 'tabStats' };

// 每个视图各自的容器；今天那块本身就是滚动区，另两块各带一段工具区。
const VIEW_CONTAINERS = { today: 'scroller', records: 'viewRecords', stats: 'viewStats' };

function paintTabs(name) {
  for (const [key, id] of Object.entries(TAB_BUTTONS)) {
    const button = document.getElementById(id);
    button.classList.toggle('is-on', key === name);
    button.setAttribute('aria-selected', String(key === name));
  }
}

function showView(name) {
  for (const [key, id] of Object.entries(VIEW_CONTAINERS))
    document.getElementById(id).hidden = key !== name;
}

function setView(name) {
  if (!TAB_BUTTONS[name]) return;
  // 标签自身立刻响应；内容先退场，窗口换好尺寸再淡入（§8）。
  paintTabs(name);
  const mine = ++views.seq;
  views.queue = views.queue.catch(() => {}).then(async () => {
    if (mine !== views.seq || name === views.current) return;
    const card = document.getElementById('card');
    const ms = motionMs();
    // 减少动效时连淡出都别做：隐藏两帧同样是一次闪烁。
    if (ms) {
      card.classList.add('is-content-leaving');
      await sleep(ms);
    }
    views.current = name;
    showView(name);
    const hook = views.hooks[name];
    if (hook) hook();
    // 窗口该变到哪一档由 Rust 的 card_set_view 决定；折叠态也走同一个入口，两处不会打架。
    await cfSize();
    await afterPaint();
    card.classList.remove('is-content-leaving');
    updateFade();
  }).catch((err) => {
    // 卡在其中一步时不要把内容永远留在透明态；报一声，用户还能继续操作。
    document.getElementById('card').classList.remove('is-content-leaving');
    toast(String(err));
  });
}

document.getElementById('tabs').addEventListener('click', (event) => {
  const tab = event.target.closest('[data-view]');
  if (tab) setView(tab.dataset.view);
});

// 换尺寸后内容还能不能往下翻要重算一次，否则淡出会停在错误的那一边。
window.addEventListener('resize', updateFade);
