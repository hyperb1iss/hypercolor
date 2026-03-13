/**
 * Hypercolor — Lazy Mermaid.js loader with theme-aware rendering
 */

(() => {
  let mermaidLoaded = false;
  let loadPromise = null;

  const CDN_URL = 'https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.min.js';

  const getMermaidTheme = () => {
    const theme = document.documentElement.getAttribute('data-theme');
    return theme === 'light' ? 'default' : 'dark';
  };

  const showLoading = (elements) => {
    elements.forEach((el) => {
      if (!el.querySelector('.mermaid-loading')) {
        const loader = document.createElement('div');
        loader.className = 'mermaid-loading';
        loader.setAttribute('aria-label', 'Loading diagram');
        loader.textContent = 'Loading diagram\u2026';
        el.prepend(loader);
      }
    });
  };

  const clearLoading = (elements) => {
    elements.forEach((el) => {
      const loader = el.querySelector('.mermaid-loading');
      if (loader) loader.remove();
    });
  };

  const loadMermaid = () => {
    if (loadPromise) return loadPromise;

    loadPromise = new Promise((resolve, reject) => {
      const script = document.createElement('script');
      script.src = CDN_URL;
      script.async = true;
      script.onload = () => {
        mermaidLoaded = true;
        resolve();
      };
      script.onerror = () => reject(new Error('Failed to load Mermaid'));
      document.head.appendChild(script);
    });

    return loadPromise;
  };

  const renderDiagrams = async (elements) => {
    if (!window.mermaid || !elements.length) return;

    // Store original source before mermaid mutates the DOM
    elements.forEach((el) => {
      if (!el.getAttribute('data-mermaid-src')) {
        el.setAttribute('data-mermaid-src', el.textContent);
      }
    });

    window.mermaid.initialize({
      startOnLoad: false,
      theme: getMermaidTheme(),
      fontFamily: 'Inter, sans-serif',
    });

    // Reset elements for re-render
    elements.forEach((el) => {
      el.removeAttribute('data-processed');
      const src = el.getAttribute('data-mermaid-src');
      if (src) el.textContent = src;
    });

    await window.mermaid.run({ nodes: elements });
    clearLoading(elements);
  };

  const init = () => {
    const elements = document.querySelectorAll('.mermaid');
    if (!elements.length) return;

    showLoading(elements);

    loadMermaid()
      .then(() => renderDiagrams([...elements]))
      .catch(() => {
        clearLoading(elements);
        elements.forEach((el) => {
          el.textContent = 'Diagram failed to load.';
        });
      });

    // Re-render on theme change
    window.addEventListener('themechange', () => {
      if (!mermaidLoaded) return;
      const current = document.querySelectorAll('.mermaid');
      if (current.length) renderDiagrams([...current]);
    });
  };

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
