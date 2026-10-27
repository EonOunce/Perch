/* 开机自启的首次一问（DESIGN §5.3、§12 #6）。
   默认关，而且只问这一次：两个答案都算回答，问过就再也不弹；之后要改只能走托盘的勾选项。
   「先不用」不写注册表也不删已有启动项——那一半由 Rust 侧的 autostart_decline 保证。 */

const ase = {};
for (const id of ['askSheet', 'askOn', 'askLater']) {
  ase[id] = document.getElementById(id);
}

function closeAsk() {
  ase.askSheet.hidden = true;
}

ase.askOn.addEventListener('click', async () => {
  try {
    await invoke('autostart_set', { enabled: true });
    closeAsk();
    toast('已设为开机自启，想关掉就在托盘菜单里点同一项');
  } catch (err) {
    // 写启动项没成功（多半是注册表权限）：留着问题，让她能选「先不用」走开。
    toast(String(err));
  }
});

ase.askLater.addEventListener('click', async () => {
  closeAsk();
  await invoke('autostart_decline').catch(() => {});
});

// 按 Esc 走开等于没回答：不钉「问过了」，下次开机还会再问一次。
window.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && !ase.askSheet.hidden) closeAsk();
});

// 开机第一帧就带上这个问题：它是这一次启动里唯一要做的事，不该等人自己去别处找入口。
(async () => {
  const prefs = await invoke('prefs_get').catch(() => null);
  // 问过、或本来就已经开着（换过安装位置之类），都不再打扰。
  if (!prefs || prefs.autostartAsked || prefs.autostart) return;
  ase.askSheet.hidden = false;
  ase.askLater.focus({ preventScroll: true });
})();
