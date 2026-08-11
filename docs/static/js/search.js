/**
 * Hypercolor — Full-text search using Zola's Elasticlunr index
 */
(() => {
  let index = null, loadPromise = null, debounceTimer = null, activeIdx = -1;
  const isMac = navigator.platform.toUpperCase().includes('MAC');

  // Both URLs are injected by base.html via Zola get_url() so they resolve
  // under any base_url (GitHub Pages sub-path, custom domain). Falls back to
  // the root-relative paths only if the attributes are absent.
  const indexUrl = document.body.getAttribute('data-search-index') || '/search_index.en.js';
  const libUrl = document.body.getAttribute('data-search-lib') || '/elasticlunr.min.js';

  const loadScript = (src) =>
    new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = src;
      s.onload = resolve;
      s.onerror = () => reject(new Error(`Failed to load ${src}`));
      document.head.appendChild(s);
    });

  // Zola emits elasticlunr.min.js alongside the index but never wires it up;
  // the index script only assigns window.searchIndex, so the library has to be
  // loaded too before the index can be deserialized.
  const loadIndex = () => {
    if (index) return Promise.resolve();
    if (loadPromise) return loadPromise;
    loadPromise = Promise.all([loadScript(libUrl), loadScript(indexUrl)]).then(() => {
      if (!window.elasticlunr || !window.searchIndex) {
        throw new Error('Search index unavailable');
      }
      index = window.elasticlunr.Index.load(window.searchIndex);
    });
    // A failed load must not be cached, or a transient network error would kill
    // search for the rest of the session.
    loadPromise.catch(() => { loadPromise = null; });
    return loadPromise;
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

  // Zola's index carries `path` but no section field, so the top-level segment
  // is what tells two same-named pages apart in the results list.
  const sectionLabel = (path) => {
    const seg = (path || '').replace(/^\/+/, '').split('/')[0];
    if (!seg || seg.endsWith('.md')) return '';
    return seg.replace(/-/g, ' ');
  };

  const render = (hits, terms, el) => {
    activeIdx = -1;
    if (!hits.length) { el.innerHTML = '<div class="search-result search-result--empty">No results found</div>'; return; }
    el.innerHTML = hits.slice(0, 10).map((r) => {
      const { title, body, path } = r.doc;
      const section = sectionLabel(path);
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
    const trigger = document.getElementById('search-trigger');
    const backdrop = modal?.querySelector('.search-modal__backdrop');
    if (!modal || !input || !results) return;

    const EMPTY = '<div class="search-results__empty">Type to search the documentation</div>';
    const resetResults = () => { results.innerHTML = EMPTY; };

    modal.setAttribute('role', 'dialog');
    modal.setAttribute('aria-modal', 'true');
    modal.setAttribute('aria-label', 'Search documentation');
    modal.setAttribute('aria-hidden', 'true');
    input.setAttribute('role', 'combobox');
    input.setAttribute('aria-expanded', 'false');
    input.setAttribute('aria-controls', 'search-results');
    results.setAttribute('role', 'listbox');

    // Visibility is driven by the `hidden` attribute so the modal stays closed
    // for anyone without JS. The class only carries the open-state styling.
    const isOpen = () => !modal.hasAttribute('hidden');

    const open = () => {
      if (isOpen()) return;
      modal.removeAttribute('hidden');
      modal.classList.add('search-modal--open');
      modal.setAttribute('aria-hidden', 'false');
      document.body.style.overflow = 'hidden';
      input.value = '';
      input.focus();
      // Warm the index while the user is still typing; the input handler
      // surfaces the failure if it comes to that.
      loadIndex().catch(() => {});
    };
    const close = () => {
      modal.setAttribute('hidden', '');
      modal.classList.remove('search-modal--open');
      modal.setAttribute('aria-hidden', 'true');
      document.body.style.overflow = '';
      input.value = '';
      resetResults();
      input.setAttribute('aria-expanded', 'false');
      activeIdx = -1;
    };

    document.addEventListener('keydown', (e) => {
      if ((isMac ? e.metaKey : e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); open(); }
      if (e.key === 'Escape' && isOpen()) { e.preventDefault(); close(); trigger?.focus(); }
    });

    trigger?.addEventListener('click', open);
    backdrop?.addEventListener('click', close);

    input.addEventListener('input', () => {
      clearTimeout(debounceTimer);
      const q = input.value.trim();
      if (!q) { resetResults(); input.setAttribute('aria-expanded', 'false'); return; }
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
