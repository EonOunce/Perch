/* 标签切换（DESIGN §2.1、§2.2）。
   这里只说「现在看哪个标签」：窗口该变多大由 Rust 的尺寸档决定（morphRect 来读），
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

function setView(name, clicked) {
  if (!TAB_BUTTONS[name]) return;
  // 标签自身立刻响应；旧内容淡出 → 换布局 → 窗口围着被点的标签长大/缩小（§8）。
  paintTabs(name);
  const mine = ++views.seq;
  views.queue = views.queue.catch(() => {}).then(async () => {
    if (mine !== views.seq || name === views.current) return;
    const incomingEl = document.getElementById(VIEW_CONTAINERS[name]);
    const outgoingEl = document.getElementById(VIEW_CONTAINERS[views.current]);
    const anchorEl = clicked || document.getElementById(TAB_BUTTONS[name]);
    const point = anchorPoint(anchorEl);
    await sinkOut([outgoingEl], motionOut(), point);
    views.current = name;
    showView(name);
    const hook = views.hooks[name];
    if (hook) hook();
    // 同一个任务里藏好新内容，morphRect 量锚点那几帧才不会先露一帧新视图。
    hideNow([incomingEl]);
    // 淡入不等于换完档：动画演完才算这一次结束，队列才不会和下一次换档抢窗口。
    // 标签三格是被窗口宽度撑开的，横向按等比预测（实测中间帧最多偏 6px，收尾归零）。
    const morph = morphRect(name, anchorEl, 'stretch');
    riseIn([incomingEl], motionIn(), point);
    await morph;
    updateFade();
  }).catch((err) => {
    // 卡在其中一步时不要把内容永远留在透明态；报一声，用户还能继续操作。
    for (const id of Object.values(VIEW_CONTAINERS)) {
      document.getElementById(id).style.opacity = '';
    }
    toast(String(err));
  });
}

document.getElementById('tabs').addEventListener('click', (event) => {
  const tab = event.target.closest('[data-view]');
  if (tab) setView(tab.dataset.view, tab);
});

// 换尺寸后内容还能不能往下翻要重算一次，否则淡出会停在错误的那一边。
window.addEventListener('resize', updateFade);
