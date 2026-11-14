'use strict';
// Shared helpers for the note and list windows.
window.GM = (function () {
  const T = window.__TAURI__;
  const win = T.window.getCurrentWindow();
  const root = document.documentElement;

  // Commands run in order so a close never overtakes the last save.
  let queue = Promise.resolve();
  function call(cmd, args, opts) {
    const p = queue.then(() => T.core.invoke(cmd, args, opts));
    queue = p.catch(() => {});
    return p.catch(() => null);
  }

  // Native drag after 4px of movement; plain clicks and double-clicks still arrive.
  function dragZone(el, skip) {
    el.addEventListener('mousedown', (e) => {
      if (e.button !== 0 || (skip && skip(e))) return;
      const sx = e.screenX, sy = e.screenY;
      const move = (ev) => {
        if (Math.abs(ev.screenX - sx) >= 4 || Math.abs(ev.screenY - sy) >= 4) {
          done();
          win.startDragging().catch(() => {});
        }
      };
      const done = () => {
        window.removeEventListener('mousemove', move);
        window.removeEventListener('mouseup', done);
      };
      window.addEventListener('mousemove', move);
      window.addEventListener('mouseup', done);
    });
  }

  // "삭제하시겠습니까?" card (#confirm). Resolves true for 삭제, false for 유지.
  function confirmDelete() {
    const box = document.getElementById('confirm');
    const keep = document.getElementById('confirm-keep');
    const del = document.getElementById('confirm-delete');
    box.hidden = false;
    keep.focus();
    return new Promise((resolve) => {
      const finish = (v) => {
        box.hidden = true;
        keep.removeEventListener('click', onKeep);
        del.removeEventListener('click', onDel);
        box.removeEventListener('keydown', onKey);
        box.removeEventListener('mousedown', onScrim);
        resolve(v);
      };
      const onKeep = () => finish(false);
      const onDel = () => finish(true);
      const onKey = (e) => { if (e.key === 'Escape') { e.preventDefault(); finish(false); } };
      const onScrim = (e) => { if (e.target === box) finish(false); };
      keep.addEventListener('click', onKeep);
      del.addEventListener('click', onDel);
      box.addEventListener('keydown', onKey);
      box.addEventListener('mousedown', onScrim);
    });
  }

  function setTheme(t) {
    root.dataset.theme = t === 'light' ? 'light' : 'dark';
  }

  function setBand(b) {
    root.dataset.band = (b === 'none' || b === 'full') ? b : 'thin';
  }

  // Only text fields get the native context menu (copy / paste).
  document.addEventListener('contextmenu', (e) => {
    if (!e.target.closest('input, textarea')) e.preventDefault();
  });

  T.event.listen('theme', (e) => setTheme(e.payload));
  T.event.listen('band', (e) => setBand(e.payload));

  return { T, win, call, dragZone, confirmDelete, setTheme, setBand };
})();
