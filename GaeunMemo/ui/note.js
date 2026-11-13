'use strict';
(function () {
  const { win, call, dragZone, confirmDelete, setTheme, setBand } = window.GM;
  const id = win.label.replace(/^note-/, '');
  const root = document.documentElement;
  const $ = (s) => document.getElementById(s);
  const titleEl = $('title'), bodyEl = $('body'), menu = $('menu');
  const pinBtn = $('pin'), moreBtn = $('more'), themeBtn = $('theme-btn');
  const BANDS = ['none', 'thin', 'full'];
  const BAND_NAMES = { none: '없음', thin: '얇은 띠', full: '제목 줄' };

  const NAMES = { '#AFA9EC': '라벤더', '#F4C0D1': '로즈', '#9FE1CB': '민트', '#F6C9A0': '살구', '#B5D4F4': '하늘', '#F3E9C6': '크림' };
  const FONT_MIN = 10, FONT_MAX = 32;
  let palette = Object.keys(NAMES);
  let folded = false;
  let fontSize = 14;
  let fontTimer = 0;

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
        applyColor(c);
        call('note_set_color', { id, color: c });
      });
      box.appendChild(b);
    }
  }

  // ---------- editing ----------

  const save = () => call('note_save', { id, title: titleEl.value, text: bodyEl.value });
  titleEl.addEventListener('input', save);
  bodyEl.addEventListener('input', save);

  function startRename() {
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
      toggleFold();
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
    const first = menu.querySelector('.menu-item');
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

  menu.addEventListener('click', (e) => {
    const act = e.target.closest('[data-act]')?.dataset.act;
    if (!act) return;
    if (act === 'band') {
      // stays open so the next click moves on: 없음 → 얇은 띠 → 제목 줄
      const next = BANDS[(BANDS.indexOf(root.dataset.band) + 1) % BANDS.length];
      applyBand(next);
      call('band_set', { band: next });
      return;
    }
    closeMenu();
    if (act === 'rename') startRename();
    else if (act === 'theme') {
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
  });

  // ---------- delete ----------

  async function deleteSelf() {
    const hasContent = titleEl.value.trim() !== '' || bodyEl.value.trim() !== '';
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
    bodyEl.value = n.text;
    applyColor(n.color);
    applyTheme(n.theme);
    applyBand(n.band);
    applyFont(n.fontSize);
    setFolded(n.folded);
    pinBtn.setAttribute('aria-pressed', String(n.pinned));
    syncFocus();
    if (document.hasFocus() && !folded) bodyEl.focus();
  })();

  window.GM.T.event.listen('theme', (e) => applyTheme(e.payload));
  window.GM.T.event.listen('band', (e) => applyBand(e.payload));
})();
