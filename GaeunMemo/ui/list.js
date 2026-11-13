'use strict';
(function () {
  const { T, call, dragZone, confirmDelete, setTheme } = window.GM;
  const $ = (s) => document.getElementById(s);
  const rowsEl = $('rows'), emptyEl = $('empty'), search = $('search'), ctx = $('ctx');

  let notes = []; // data-file order; shown most recently edited first
  let ctxId = null;

  const firstLine = (s) => (s.split('\n').find((l) => l.trim() !== '') || '').trim();

  function ago(ms) {
    if (!ms) return '';
    const s = (Date.now() - ms) / 1000;
    if (s < 60) return '방금';
    if (s < 3600) return Math.floor(s / 60) + '분 전';
    if (s < 86400) return Math.floor(s / 3600) + '시간 전';
    if (s < 172800) return '어제';
    if (s < 604800) return Math.floor(s / 86400) + '일 전';
    const d = new Date(ms);
    return (d.getMonth() + 1) + '월 ' + d.getDate() + '일';
  }

  function rowFor(n) {
    const li = document.createElement('li');
    const b = document.createElement('button');
    b.className = 'row' + (n.open ? '' : ' closed');
    b.dataset.id = n.id;
    const dot = document.createElement('span');
    dot.className = 'dot';
    dot.style.setProperty('--dot', n.color);
    const title = document.createElement('span');
    title.className = 'row-title' + (n.title.trim() ? '' : ' untitled');
    title.textContent = n.title.trim() || '제목 없음';
    const when = document.createElement('span');
    when.className = 'when';
    when.textContent = ago(n.updatedAt);
    b.append(dot, title, when);
    const line = firstLine(n.text);
    if (line) {
      const prev = document.createElement('span');
      prev.className = 'preview';
      prev.textContent = line;
      b.appendChild(prev);
    }
    li.appendChild(b);
    return li;
  }

  function sorted() {
    // newest edit first; ties keep "newest created first"
    return notes
      .map((n, i) => [n, i])
      .sort((a, b) => (b[0].updatedAt || 0) - (a[0].updatedAt || 0) || b[1] - a[1])
      .map((p) => p[0]);
  }

  let shownIds = []; // ids in the order they are on screen

  function visible() {
    const q = search.value.trim().toLowerCase();
    return sorted().filter((n) => !q || (n.title + '\n' + n.text).toLowerCase().includes(q));
  }

  // One note changed (typing in a note window): if the visible order is the same,
  // swap just that row instead of rebuilding the whole list.
  function update(n) {
    const ids = visible().map((x) => x.id);
    const same = ids.length === shownIds.length && ids.every((v, i) => v === shownIds[i]);
    const old = same && rowsEl.querySelector(`[data-id="${CSS.escape(n.id)}"]`);
    if (!old) { render(); return; }
    const wasFocused = document.activeElement === old;
    const li = rowFor(n);
    old.parentElement.replaceWith(li);
    if (wasFocused) li.firstChild.focus();
  }

  function render() {
    const focusedId = document.activeElement?.dataset?.id;
    const shown = visible();
    shownIds = shown.map((n) => n.id);
    $('count').textContent = String(notes.length);
    rowsEl.replaceChildren(...shown.map(rowFor));
    if (notes.length === 0) { emptyEl.textContent = '메모가 없어요'; emptyEl.hidden = false; }
    else if (shown.length === 0) { emptyEl.textContent = '찾는 메모가 없어요'; emptyEl.hidden = false; }
    else emptyEl.hidden = true;
    if (focusedId) rowsEl.querySelector(`[data-id="${CSS.escape(focusedId)}"]`)?.focus();
  }

  async function reload() {
    const v = await call('list_get');
    if (!v) return;
    notes = v.notes;
    setTheme(v.theme);
    render();
  }

  const rowButtons = () => [...rowsEl.querySelectorAll('.row')];

  // ---------- search and keyboard ----------

  search.addEventListener('input', render);
  search.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && search.value) { search.value = ''; render(); e.stopPropagation(); }
    else if (e.key === 'ArrowDown') { e.preventDefault(); rowButtons()[0]?.focus(); }
    else if (e.key === 'Enter') { const r = rowButtons()[0]; if (r) call('list_open_note', { id: r.dataset.id }); }
  });

  rowsEl.addEventListener('keydown', (e) => {
    const row = e.target.closest('.row');
    if (!row) return;
    const all = rowButtons();
    const i = all.indexOf(row);
    if (e.key === 'ArrowDown') { e.preventDefault(); all[i + 1]?.focus(); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); (all[i - 1] || search).focus(); }
    else if (e.key === 'Delete') {
      e.preventDefault();
      const next = all[i + 1] || all[i - 1] || search;
      remove(row.dataset.id).then((gone) => (gone ? next : row).focus());
    }
  });

  rowsEl.addEventListener('click', (e) => {
    const row = e.target.closest('.row');
    if (row) call('list_open_note', { id: row.dataset.id });
  });

  // ---------- context menu ----------

  function closeCtx() {
    ctx.hidden = true;
    rowsEl.querySelector('.is-context')?.classList.remove('is-context');
    ctxId = null;
  }
  rowsEl.addEventListener('contextmenu', (e) => {
    const row = e.target.closest('.row');
    if (!row) return;
    e.preventDefault();
    closeCtx();
    ctxId = row.dataset.id;
    row.classList.add('is-context');
    ctx.hidden = false;
    const x = Math.min(e.clientX, window.innerWidth - ctx.offsetWidth - 4);
    const y = Math.min(e.clientY, window.innerHeight - ctx.offsetHeight - 4);
    ctx.style.left = Math.max(4, x) + 'px';
    ctx.style.top = Math.max(4, y) + 'px';
    $('ctx-delete').focus();
  });
  document.addEventListener('mousedown', (e) => { if (!ctx.hidden && !ctx.contains(e.target)) closeCtx(); });
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && !ctx.hidden) closeCtx(); });
  window.addEventListener('blur', closeCtx);

  $('ctx-delete').addEventListener('click', () => {
    const id = ctxId;
    closeCtx();
    if (id) remove(id);
  });

  // ---------- delete: "삭제하시겠습니까?" for notes with content ----------

  async function remove(id) {
    const n = notes.find((x) => x.id === id);
    if (!n) return false;
    const hasContent = n.title.trim() !== '' || n.text.trim() !== '';
    if (hasContent && !(await confirmDelete())) return false;
    await call('note_delete', { id });
    return true;
  }

  // ---------- header ----------

  dragZone($('bar'), (e) => !!e.target.closest('.tools'));
  dragZone($('band'));
  $('new').addEventListener('click', () => call('list_new'));
  $('close').addEventListener('click', () => call('list_close'));

  // ---------- live updates (one note at a time) ----------

  T.event.listen('list-upsert', (e) => {
    const n = e.payload;
    const i = notes.findIndex((x) => x.id === n.id);
    if (i === -1) { reload(); return; } // new or restored: refetch to keep order
    notes[i] = n;
    update(n);
  });
  T.event.listen('list-remove', (e) => {
    notes = notes.filter((x) => x.id !== e.payload);
    render();
  });
  // keep "n분 전" fresh
  setInterval(() => { if (!document.hidden) render(); }, 60000);

  reload();
  search.focus();
})();
