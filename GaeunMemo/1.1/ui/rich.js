'use strict';
// Formatted note bodies: an allow-list sanitiser and helpers.
// Only these survive (from our own editor, pasted web/Word content, or the
// data file): b i u s, ul/li, br, div, and app pictures <img data-gm="NAME">.
// Everything else is unwrapped to its text; all attributes except data-gm are
// dropped. Inline styles that mean bold / italic / underline / strike are
// turned into the matching tags, so formatting copied from a web page or Word
// keeps working ("지원 서식만 살리기").
window.Rich = (function () {
  const IMG_HOST = 'http://gmimg.localhost/';
  const NAME = /^[0-9a-f-]{36}\.(png|jpg|gif|webp)$/;
  const MAP = { B: 'b', STRONG: 'b', I: 'i', EM: 'i', U: 'u', INS: 'u', S: 's', STRIKE: 's', DEL: 's',
    UL: 'ul', OL: 'ul', LI: 'li', BR: 'br', DIV: 'div', P: 'div' };
  const BLOCK = new Set(['H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'SECTION', 'ARTICLE', 'HEADER', 'FOOTER',
    'BLOCKQUOTE', 'PRE', 'TR', 'TABLE', 'FIGURE', 'ASIDE', 'NAV', 'MAIN', 'ADDRESS', 'DT', 'DD']);
  const DROP = new Set(['SCRIPT', 'STYLE', 'TEMPLATE', 'IFRAME', 'OBJECT', 'EMBED', 'HEAD', 'TITLE',
    'META', 'LINK', 'NOSCRIPT', 'SVG', 'MATH', 'CANVAS', 'VIDEO', 'AUDIO', 'SELECT', 'TEXTAREA', 'INPUT', 'BUTTON']);

  function styleWraps(el) {
    const st = (el.getAttribute && el.getAttribute('style')) || '';
    const w = [];
    if (/font-weight\s*:\s*(bold|bolder|[6-9]00)/i.test(st)) w.push('b');
    if (/font-style\s*:\s*italic/i.test(st)) w.push('i');
    if (/text-decoration[^;]*underline/i.test(st)) w.push('u');
    if (/text-decoration[^;]*line-through/i.test(st)) w.push('s');
    return w;
  }

  function clean(src, out) {
    for (const node of src.childNodes) {
      if (node.nodeType === 3) { out.appendChild(document.createTextNode(node.data)); continue; }
      if (node.nodeType !== 1) continue;
      const tag = node.tagName.toUpperCase();
      if (DROP.has(tag)) continue;
      if (tag === 'IMG') {
        const n = node.getAttribute('data-gm');
        if (n && NAME.test(n)) {
          const img = document.createElement('img');
          img.setAttribute('data-gm', n);
          out.appendChild(img);
        }
        continue;
      }
      let target = out;
      const mapped = MAP[tag] || (BLOCK.has(tag) ? 'div' : null);
      if (mapped) {
        const el = document.createElement(mapped);
        target.appendChild(el);
        target = el;
      }
      if (tag !== 'BR') {
        for (const w of styleWraps(node)) {
          if (w === mapped) continue;
          const el = document.createElement(w);
          target.appendChild(el);
          target = el;
        }
        clean(node, target);
      }
    }
    return out;
  }

  // Parse untrusted HTML without running or loading anything (<template> is inert).
  function fromHtml(html) {
    const t = document.createElement('template');
    t.innerHTML = html;
    return clean(t.content, document.createDocumentFragment());
  }

  function hydrate(root) {
    for (const img of root.querySelectorAll('img[data-gm]')) {
      const n = img.getAttribute('data-gm');
      if (NAME.test(n) && !img.getAttribute('src')) img.src = IMG_HOST + n;
      img.draggable = false;
    }
  }

  // What gets saved: the editor's content, cleaned, without picture URLs.
  function toStored(editor) {
    const box = document.createElement('div');
    box.appendChild(clean(editor, document.createDocumentFragment()));
    return box.innerHTML;
  }

  function plain(editor) {
    return editor.innerText.replace(/ /g, ' ').replace(/\n+$/, '');
  }

  // Fill the editor from stored HTML, or from plain text for older notes.
  function render(editor, html, text) {
    editor.replaceChildren();
    if (html) {
      editor.appendChild(fromHtml(html));
    } else if (text) {
      text.split('\n').forEach((line, i) => {
        if (i) editor.appendChild(document.createElement('br'));
        if (line) editor.appendChild(document.createTextNode(line));
      });
    }
    hydrate(editor);
  }

  function serialize(frag) {
    const box = document.createElement('div');
    box.appendChild(frag);
    return box.innerHTML;
  }

  return { NAME, fromHtml, hydrate, toStored, plain, render, serialize };
})();
