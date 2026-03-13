/**
 * Hypercolor — Full-text search using Zola's Elasticlunr index
 */
(() => {
  let index = null, debounceTimer = null, activeIdx = -1;
  const isMac = navigator.platform.toUpperCase().includes('MAC');

  const loadIndex = () => {
    if (index) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = '/search_index.en.js';
      s.onload = () => window.elasticlunr
        ? (index = elasticlunr.Index.load(window.searchIndex), resolve())
        : reject(new Error('No elasticlunr'));
      s.onerror = () => reject(new Error('Index load failed'));
      document.head.appendChild(s);
    });
  };

  const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

  const highlight = (text, terms) => {
    if (!text || !terms.length) return esc(text);
    const re = new RegExp(`(${terms.map(t => t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|')})`, 'gi');
    return esc(text).replace(re, '<mark>$1</mark>');
  };

  const snippet = (body, terms) => {
    if (!body) return '';
    const lo = body.toLowerCase();
    let pos = 0;
    for (const t of terms) { const i = lo.indexOf(t.toLowerCase()); if (i !== -1) { pos = i; break; } }
    const start = Math.max(0, pos - 60), end = Math.min(body.length, start + 120);
    let s = body.slice(start, end).trim();
    if (start > 0) s = '...' + s;
    if (end < body.length) s += '...';
    return highlight(s, terms);
  };

  const render = (hits, terms, el) => {
    activeIdx = -1;
    if (!hits.length) { el.innerHTML = '<div class="search-result search-result--empty">No results found</div>'; return; }
    el.innerHTML = hits.slice(0, 10).map((r) => {
      const { title, body, section } = r.doc;
      const sec = section ? `<div class="search-result__section">${esc(section)}</div>` : '';
      return `<a class="search-result" href="${esc(r.ref)}">
<div class="search-result__title">${highlight(title || 'Untitled', terms)}</div>
${sec}<div class="search-result__snippet">${snippet(body || '', terms)}</div></a>`;
    }).join('');
  };

  const setActive = (container, idx) => {
    const items = container.querySelectorAll('.search-result:not(.search-result--empty)');
    if (!items.length) return;
    items.forEach(el => el.classList.remove('search-result--active'));
    activeIdx = Math.max(0, Math.min(idx, items.length - 1));
    items[activeIdx].classList.add('search-result--active');
    items[activeIdx].scrollIntoView({ block: 'nearest' });
  };

  const init = () => {
    const modal = document.getElementById('search-modal');
    const input = document.getElementById('search-input');
    const results = document.getElementById('search-results');
    const backdrop = modal?.querySelector('.search-modal__backdrop');
    if (!modal || !input || !results) return;

    modal.setAttribute('role', 'dialog');
    modal.setAttribute('aria-modal', 'true');
    modal.setAttribute('aria-label', 'Search documentation');
    modal.setAttribute('aria-hidden', 'true');
    input.setAttribute('role', 'combobox');
    input.setAttribute('aria-expanded', 'false');
    input.setAttribute('aria-controls', 'search-results');
    results.setAttribute('role', 'listbox');

    const open = () => {
      modal.classList.add('search-modal--open');
      modal.setAttribute('aria-hidden', 'false');
      document.body.style.overflow = 'hidden';
      input.value = '';
      input.focus();
      loadIndex();
    };
    const close = () => {
      modal.classList.remove('search-modal--open');
      modal.setAttribute('aria-hidden', 'true');
      document.body.style.overflow = '';
      results.innerHTML = '';
      activeIdx = -1;
    };

    document.addEventListener('keydown', (e) => {
      if ((isMac ? e.metaKey : e.ctrlKey) && e.key === 'k') { e.preventDefault(); open(); }
      if (e.key === 'Escape' && modal.classList.contains('search-modal--open')) { e.preventDefault(); close(); }
    });

    backdrop?.addEventListener('click', close);

    input.addEventListener('input', () => {
      clearTimeout(debounceTimer);
      const q = input.value.trim();
      if (!q) { results.innerHTML = ''; input.setAttribute('aria-expanded', 'false'); return; }
      debounceTimer = setTimeout(async () => {
        try { await loadIndex(); } catch { results.innerHTML = '<div class="search-result search-result--empty">Search index unavailable</div>'; return; }
        const terms = q.split(/\s+/).filter(Boolean);
        const hits = index.search(q, { fields: { title: { boost: 2 }, body: { boost: 1 } }, bool: 'OR', expand: true });
        input.setAttribute('aria-expanded', hits.length ? 'true' : 'false');
        render(hits, terms, results);
      }, 200);
    });

    input.addEventListener('keydown', (e) => {
      const items = results.querySelectorAll('.search-result:not(.search-result--empty)');
      if (!items.length) return;
      if (e.key === 'ArrowDown') { e.preventDefault(); setActive(results, activeIdx + 1); }
      else if (e.key === 'ArrowUp') { e.preventDefault(); setActive(results, activeIdx - 1); }
      else if (e.key === 'Enter' && activeIdx >= 0) { e.preventDefault(); if (items[activeIdx]?.href) window.location.href = items[activeIdx].href; }
    });
  };

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
  else init();
})();
