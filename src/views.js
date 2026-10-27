/* 标签切换（DESIGN §2.1、§2.2）。
   这里只说「现在看哪个标签」：窗口该变多大由 Rust 的 card_set_view 决定，
   尺寸表如果在 JS 里再抄一份，就会出现「标签是记录、窗口还是今天的尺寸」这类对不上的状态。 */

const views = {
  current: 'today',
  // 每个视图进来时要拉一次自己的快照，由各自的脚本登记（records.js 用）。
  hooks: {},
};

const TAB_BUTTONS = { today: 'tabToday', records: 'tabRecords', stats: 'tabStats' };

// 每个视图各自的容器；今天那块本身就是滚动区，另两块各带一段工具区。
const VIEW_CONTAINERS = { today: 'scroller', records: 'viewRecords', stats: 'viewStats' };

function setView(name) {
  if (!TAB_BUTTONS[name] || name === views.current) return;
  views.current = name;
  for (const [key, id] of Object.entries(TAB_BUTTONS)) {
    const button = document.getElementById(id);
    button.classList.toggle('is-on', key === name);
    button.setAttribute('aria-selected', String(key === name));
  }
  for (const [key, id] of Object.entries(VIEW_CONTAINERS))
    document.getElementById(id).hidden = key !== name;
  const hook = views.hooks[name];
  if (hook) hook();
  // 窗口该变到哪一档由 Rust 的 card_set_view 决定；折叠态也走同一个入口，两处不会打架。
  cfSize();
  updateFade();
}

document.getElementById('tabs').addEventListener('click', (event) => {
  const tab = event.target.closest('[data-view]');
  if (tab) setView(tab.dataset.view);
});

// 换尺寸后内容还能不能往下翻要重算一次，否则淡出会停在错误的那一边。
window.addEventListener('resize', updateFade);
