'use strict';
(function () {
  const { win, call, dragZone, confirmDelete, setTheme, setBand } = window.GM;
  const Rich = window.Rich;
  const id = win.label.replace(/^note-/, '');
  const root = document.documentElement;
  const $ = (s) => document.getElementById(s);
  const titleEl = $('title'), bodyEl = $('body'), menu = $('menu'), fmt = $('fmt');
  const pinBtn = $('pin'), moreBtn = $('more'), themeBtn = $('theme-btn');
  const BANDS = ['none', 'thin', 'full'];
  const BAND_NAMES = { none: '없음', thin: '얇은 띠', full: '제목 줄' };

  const NAMES = { '#AFA9EC': '라벤더', '#F4C0D1': '로즈', '#9FE1CB': '민트', '#F6C9A0': '살구', '#B5D4F4': '하늘', '#F3E9C6': '크림' };
  const FONT_MIN = 10, FONT_MAX = 32;
  let palette = Object.keys(NAMES);
  let folded = false;
  let locked = false;
  let full = false;
  let fontSize = 14;
  let fontTimer = 0, copiedTimer = 0;
  let plainPaste = false;

  // ---------- render ----------

  function applyColor(c) {
    root.style.setProperty('--accent', c);
    for (const b of $('swatches').children) b.setAttribute('aria-checked', String(b.dataset.color === c));
  }

  function applyTheme(t) {
    setTheme(t);
    themeBtn.textContent = t === 'light' ? '다크 모드' : '라이트 모드';
  }

  function applyBand(b) {
    setBand(b);
    $('band-value').textContent = BAND_NAMES[root.dataset.band];
  }

  function applyFont(px) {
    fontSize = Math.min(FONT_MAX, Math.max(FONT_MIN, px | 0));
    root.style.setProperty('--fs', fontSize + 'px');
  }

  function setFolded(f) {
    folded = !!f;
    if (folded) root.dataset.folded = ''; else delete root.dataset.folded;
  }

  function applyLock(l) {
    locked = !!l;
    if (locked) root.dataset.locked = ''; else delete root.dataset.locked;
    bodyEl.contentEditable = locked ? 'false' : 'true';
    // an <svg> has no .hidden property (HTML elements only): toggle the attribute
    $('lock-ico').toggleAttribute('hidden', !locked);
    $('lock-btn').textContent = locked ? '잠금 해제' : '잠금';
    for (const el of menu.querySelectorAll('[data-lockable], .swatch')) {
      el.disabled = locked;
      el.setAttribute('aria-disabled', String(locked));
    }
    if (locked) endRename();
    syncFull();
  }

  function buildSwatches() {
    const box = $('swatches');
    const NS = 'http://www.w3.org/2000/svg';
    box.replaceChildren();
    for (const c of palette) {
      const b = document.createElement('button');
      b.className = 'swatch';
      b.dataset.color = c;
      b.setAttribute('role', 'radio');
      b.setAttribute('aria-label', NAMES[c] || c);
      b.title = NAMES[c] || c;
      b.style.setProperty('--sw', c);
      const svg = document.createElementNS(NS, 'svg');
      svg.setAttribute('class', 'icon check');
      svg.setAttribute('viewBox', '0 0 10 10');
      svg.setAttribute('aria-hidden', 'true');
      const path = document.createElementNS(NS, 'path');
      path.setAttribute('d', 'M2 5.2l2 2 4-4.4');
      svg.appendChild(path);
      b.appendChild(svg);
      b.addEventListener('click', () => {
        if (locked) return;
        applyColor(c);
        call('note_set_color', { id, color: c });
      });
      box.appendChild(b);
    }
  }

  // ---------- 전체화면: maximised, or dragged out to fill the screen ----------

  function syncFull() {
    full = window.innerWidth >= screen.availWidth - 16 && window.innerHeight >= screen.availHeight - 16;
    fmt.hidden = !full || locked;
    $('restore').hidden = !full;
    $('full-btn').textContent = full ? '원래 크기' : '전체화면';
  }
  window.addEventListener('resize', syncFull);

  async function setFull(on) {
    await call('note_fullscreen', { id, on });
    if (on) setFolded(false);
  }
  $('restore').addEventListener('click', () => setFull(false));

  // ---------- editing ----------

  function syncEmpty() {
    const empty = Rich.plain(bodyEl).trim() === '' && !bodyEl.querySelector('img');
    bodyEl.classList.toggle('is-empty', empty);
  }

  const save = () => {
    if (locked) return;
    syncEmpty();
    return call('note_save', { id, title: titleEl.value, text: Rich.plain(bodyEl), html: Rich.toStored(bodyEl) });
  };
  titleEl.addEventListener('input', save);
  bodyEl.addEventListener('input', save);

  function startRename() {
    if (locked) return;
    titleEl.readOnly = false;
    titleEl.tabIndex = 0;
    titleEl.focus();
    titleEl.select();
  }
  function endRename() {
    titleEl.readOnly = true;
    titleEl.tabIndex = -1;
    titleEl.setSelectionRange(0, 0);
  }
  titleEl.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === 'Escape') {
      e.preventDefault();
      endRename();
      if (e.key === 'Enter') {
        if (folded) toggleFold();
        bodyEl.focus();
      }
    }
  });
  titleEl.addEventListener('blur', endRename);

  // Ctrl + wheel: text size, saved per note
  window.addEventListener('wheel', (e) => {
    if (!e.ctrlKey) return;
    e.preventDefault();
    applyFont(fontSize + (e.deltaY < 0 ? 1 : -1));
    clearTimeout(fontTimer);
    fontTimer = setTimeout(() => call('note_set_font', { id, size: fontSize }), 300);
  }, { passive: false });

  // ---------- formatting row ----------

  function syncFmt() {
    if (fmt.hidden) return;
    for (const b of fmt.querySelectorAll('[data-cmd]')) {
      let on = false;
      try { on = document.queryCommandState(b.dataset.cmd); } catch { /* not applicable */ }
      b.setAttribute('aria-pressed', String(on));
    }
  }
  document.addEventListener('selectionchange', syncFmt);
  // keep the text selection when a toolbar button is pressed
  fmt.addEventListener('mousedown', (e) => { if (e.target.closest('button')) e.preventDefault(); });
  fmt.addEventListener('click', (e) => {
    const b = e.target.closest('[data-cmd]');
    if (!b || locked) return;
    bodyEl.focus();
    document.execCommand(b.dataset.cmd, false, null);
    syncFmt();
    save();
  });

  // ---------- pictures ----------

  async function addImage(file) {
    if (locked || !file || !/^image\/(png|jpeg|gif|webp)$/.test(file.type)) return;
    const bytes = new Uint8Array(await file.arrayBuffer());
    let name = null;
    try {
      name = await window.GM.T.core.invoke('note_add_image', bytes, { headers: { 'x-gm-id': id } });
    } catch (err) {
      console.warn('image not added', err);
      return;
    }
    if (!Rich.NAME.test(name)) return;
    bodyEl.focus();
    document.execCommand('insertHTML', false, `<img data-gm="${name}">`);
    Rich.hydrate(bodyEl);
    save();
  }

  $('img-btn').addEventListener('click', () => { if (!locked) $('img-file').click(); });
  $('img-file').addEventListener('change', async (e) => {
    for (const f of e.target.files) await addImage(f);
    e.target.value = '';
  });

  // Ctrl+Shift+V = paste as plain text
  bodyEl.addEventListener('keydown', (e) => {
    if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === 'v') plainPaste = true;
  });
  bodyEl.addEventListener('paste', async (e) => {
    e.preventDefault();
    if (locked) return;
    const dt = e.clipboardData;
    const plainOnly = plainPaste;
    plainPaste = false;
    const files = [...dt.items].filter((i) => i.kind === 'file' && i.type.startsWith('image/')).map((i) => i.getAsFile());
    const html = plainOnly ? '' : dt.getData('text/html');
    const markup = html ? Rich.serialize(Rich.fromHtml(html)) : '';
    // a picture copied from a browser comes as a file plus <img src=web> markup:
    // keep the file when the markup has nothing else worth keeping
    const markupHasContent = markup.replace(/<[^>]*>/g, '').trim() !== '' || markup.includes('data-gm');
    if (files.length && !plainOnly && !markupHasContent) {
      for (const f of files) await addImage(f);
      return;
    }
    if (markupHasContent) {
      document.execCommand('insertHTML', false, markup);
      Rich.hydrate(bodyEl);
    } else {
      document.execCommand('insertText', false, dt.getData('text/plain'));
    }
    save();
  });
  bodyEl.addEventListener('dragover', (e) => { if (!locked) e.preventDefault(); });
  bodyEl.addEventListener('drop', async (e) => {
    const files = [...(e.dataTransfer?.files || [])].filter((f) => f.type.startsWith('image/'));
    if (!files.length) return;
    e.preventDefault();
    for (const f of files) await addImage(f);
  });

  // ---------- locked: click the body to copy it ----------

  function copyText(text) {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.setAttribute('readonly', '');
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand('copy');
    ta.remove();
    return ok;
  }

  bodyEl.addEventListener('click', () => {
    if (!locked) return;
    window.getSelection()?.removeAllRanges();
    if (!copyText(Rich.plain(bodyEl))) return;
    const bar = $('bar');
    $('copied').hidden = false;
    bar.classList.add('show-copied');
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => { bar.classList.remove('show-copied'); $('copied').hidden = true; }, 1000);
  });

  // ---------- drag / fold ----------

  const notDrag = (e) => !!e.target.closest('.tools') || (!titleEl.readOnly && e.target === titleEl);
  dragZone($('bar'), notDrag);
  dragZone($('band'));

  async function toggleFold() {
    const f = await call('note_toggle_fold', { id });
    if (f !== null) setFolded(f);
  }
  for (const el of [$('bar'), $('band')]) {
    el.addEventListener('dblclick', (e) => {
      if (notDrag(e)) return;
      if (full) setFull(false); // like a window title bar: double-click leaves full size
      else toggleFold();
    });
  }

  // ---------- tools ----------

  pinBtn.addEventListener('click', async () => {
    const p = await call('note_toggle_pin', { id });
    if (p !== null) pinBtn.setAttribute('aria-pressed', String(p));
  });
  $('new').addEventListener('click', () => call('note_new_near', { id }));
  $('close').addEventListener('click', () => call('note_close', { id }));

  // ---------- menu ----------

  async function openMenu() {
    if (folded) await toggleFold();
    menu.classList.remove('compact');
    menu.hidden = false;
    menu.classList.toggle('compact', window.innerHeight < menu.offsetTop + menu.offsetHeight + 4);
    moreBtn.setAttribute('aria-expanded', 'true');
    const first = menu.querySelector('.menu-item:not(:disabled)');
    if (first) first.focus();
  }
  function closeMenu() {
    menu.hidden = true;
    moreBtn.setAttribute('aria-expanded', 'false');
  }
  moreBtn.addEventListener('click', () => (menu.hidden ? openMenu() : closeMenu()));
  document.addEventListener('mousedown', (e) => {
    if (!menu.hidden && !menu.contains(e.target) && !moreBtn.contains(e.target)) closeMenu();
  });

  menu.addEventListener('click', async (e) => {
    const item = e.target.closest('[data-act]');
    if (!item || item.disabled) return;
    const act = item.dataset.act;
    if (act === 'band') {
      // stays open so the next click moves on: 없음 → 얇은 띠 → 제목 줄
      const next = BANDS[(BANDS.indexOf(root.dataset.band) + 1) % BANDS.length];
      applyBand(next);
      call('band_set', { band: next });
      return;
    }
    closeMenu();
    if (act === 'rename') startRename();
    else if (act === 'full') setFull(!full);
    else if (act === 'lock') {
      if (!locked) await save(); // keep the last keystrokes before freezing
      const l = await call('note_set_lock', { id, locked: !locked });
      if (l !== null) applyLock(l);
    } else if (act === 'theme') {
      const next = root.dataset.theme === 'light' ? 'dark' : 'light';
      applyTheme(next);
      call('theme_set', { theme: next });
    } else if (act === 'list') call('open_list');
    else if (act === 'delete') deleteSelf();
  });

  // ---------- keyboard ----------

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && !menu.hidden) { closeMenu(); moreBtn.focus(); return; }
    if (!$('confirm').hidden) return;
    if (!e.ctrlKey || e.altKey || e.shiftKey) return;
    const k = e.key.toLowerCase();
    if (k === 'n') { e.preventDefault(); call('note_new_near', { id }); }
    else if (k === 'w') { e.preventDefault(); call('note_close', { id }); }
    else if (locked && 'biu'.includes(k)) e.preventDefault();
  });

  // ---------- delete ----------

  async function deleteSelf() {
    if (locked) return;
    const hasContent = titleEl.value.trim() !== '' || !bodyEl.classList.contains('is-empty');
    if (hasContent && !(await confirmDelete())) { bodyEl.focus(); return; }
    call('note_delete', { id });
  }

  // ---------- focus-dependent tools ----------

  const syncFocus = () => document.body.classList.toggle('is-focused', document.hasFocus());
  window.addEventListener('focus', syncFocus);
  window.addEventListener('blur', () => { syncFocus(); closeMenu(); });

  // ---------- init ----------

  (async function init() {
    buildSwatches();
    applyBand(root.dataset.band);
    const n = await call('note_get', { id });
    if (!n) { call('note_close', { id }); return; }
    if (Array.isArray(n.palette) && n.palette.length) { palette = n.palette; buildSwatches(); }
    titleEl.value = n.title;
    Rich.render(bodyEl, n.html, n.text);
    syncEmpty();
    applyColor(n.color);
    applyTheme(n.theme);
    applyBand(n.band);
    applyFont(n.fontSize);
    setFolded(n.folded);
    applyLock(n.locked);
    pinBtn.setAttribute('aria-pressed', String(n.pinned));
    syncFocus();
    syncFull();
    if (document.hasFocus() && !folded && !locked) bodyEl.focus();
  })();

  window.GM.T.event.listen('theme', (e) => applyTheme(e.payload));
  window.GM.T.event.listen('band', (e) => applyBand(e.payload));
})();
