// 快速记录小窗（§2.7）。只做「写下并保存」这一件事：
// 标签与 localDate 由 perch-core 在 add_note 里提取，这里不重算一份口径。
(() => {
  const invoke = window.__TAURI__.core.invoke;
  const input = document.getElementById('input');
  const saveBtn = document.getElementById('saveBtn');
  const hint = document.getElementById('hint');
  let saving = false;

  const setHint = (text) => {
    hint.textContent = text;
    hint.hidden = !text;
  };

  const syncSave = () => {
    saveBtn.disabled = saving || input.value.trim() === '';
  };

  async function close() {
    await invoke('quick_hide');
  }

  async function save() {
    const text = input.value.trim();
    if (!text || saving) return;
    saving = true;
    syncSave();
    try {
      await invoke('quick_save', { text });
      input.value = '';
      setHint('');
      await close();
    } catch (err) {
      // 存不下就留在原地让人改：报一句中性事实，不催不罚。
      setHint(String(err));
    } finally {
      saving = false;
      syncSave();
    }
  }

  input.addEventListener('input', () => {
    setHint('');
    syncSave();
  });

  input.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      save();
    }
  });

  window.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      close();
    }
  });

  saveBtn.addEventListener('click', save);
  document.getElementById('closeBtn').addEventListener('click', close);

  // 小窗是复用的（隐藏不销毁），每次回到前台都要把光标放回输入框。
  window.addEventListener('focus', () => input.focus());
  input.focus();
})();
