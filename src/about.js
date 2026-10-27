/* 关于栖迟（DESIGN §2.10）：解释性长句集中在一处，主界面只留动作、数据与状态词。
   入口只有两个——顶栏的 ⓘ 和托盘菜单「关于栖迟」。面板不阻塞、不首启强弹。
   首次启动只在 ⓘ 上留一个提示点，她自己点开过一次就从此不再出现（§2.10「只提示一次入口存在」）。 */

const ab = {};
for (const id of ['aboutBtn', 'aboutSheet', 'aboutClose']) {
  ab[id] = document.getElementById(id);
}

let aboutReturnFocus = null;

// 提示点只负责「这儿有个入口」，看过一次就永久收掉：第二次再亮就成了催促。
function markAboutSeen() {
  if (!ab.aboutBtn.classList.contains('is-new')) return;
  ab.aboutBtn.classList.remove('is-new');
  invoke('prefs_set', { aboutHinted: true }).catch(() => {});
}

/// section 给了就把那一节滚到顶；从托盘进来没有 section，从顶部开始读。
function openAbout(section) {
  if (!ab.aboutSheet.hidden) return;
  aboutReturnFocus = document.activeElement;
  ab.aboutSheet.hidden = false;
  markAboutSeen();
  const target = section ? document.getElementById(section) : null;
  const body = ab.aboutSheet.querySelector('.sheet-body');
  if (target && body) {
    body.scrollTop = Math.max(0, target.getBoundingClientRect().top - body.getBoundingClientRect().top);
  } else if (body) {
    body.scrollTop = 0;
  }
  ab.aboutClose.focus({ preventScroll: true });
}

function closeAbout() {
  if (ab.aboutSheet.hidden) return;
  ab.aboutSheet.hidden = true;
  if (aboutReturnFocus && document.contains(aboutReturnFocus)) {
    aboutReturnFocus.focus({ preventScroll: true });
  }
}

ab.aboutBtn.addEventListener('click', () => openAbout(null));
ab.aboutClose.addEventListener('click', closeAbout);

// 点遮罩（面板本体以外的地方）走开；点面板里面不算。
ab.aboutSheet.addEventListener('pointerdown', (event) => {
  if (event.target === ab.aboutSheet) closeAbout();
});

window.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && !ab.aboutSheet.hidden) closeAbout();
});

// 托盘菜单那条「关于栖迟」：Rust 侧已经把卡片露出来，这里只负责把面板打开。
window.__TAURI__.event.listen('perch:about', () => openAbout(null));

(async () => {
  const version = await window.__TAURI__.app.getVersion().catch(() => null);
  const at = document.getElementById('aboutVersion');
  if (at) at.textContent = version ? `v${version}` : '';
  const prefs = await invoke('prefs_get').catch(() => null);
  if (prefs && !prefs.aboutHinted) ab.aboutBtn.classList.add('is-new');
})();
