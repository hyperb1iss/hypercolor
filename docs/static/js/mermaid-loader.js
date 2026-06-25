/**
 * Hypercolor — Lazy Mermaid.js loader with theme-aware rendering
 */

(() => {
  let mermaidLoaded = false;
  let loadPromise = null;

  const CDN_URL = 'https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.min.js';

  const isDark = () =>
    document.documentElement.getAttribute('data-theme') !== 'light';

  // SilkCircuit Neon — dark mode diagram palette
  const darkVars = {
    background: '#121218',
    primaryColor: '#2d1b4e',
    primaryBorderColor: '#e135ff',
    primaryTextColor: '#f8f8f2',
    secondaryColor: '#132e2e',
    secondaryBorderColor: '#80ffea',
    secondaryTextColor: '#121218',
    tertiaryColor: '#2a1525',
    tertiaryBorderColor: '#ff6ac1',
    tertiaryTextColor: '#121218',
    lineColor: '#80ffea',
    textColor: '#f8f8f2',
    mainBkg: '#2d1b4e',
    nodeBorder: '#e135ff',
    clusterBkg: 'rgba(225, 53, 255, 0.06)',
    clusterBorder: '#e135ff',
    titleColor: '#80ffea',
    edgeLabelBackground: '#1a1a24',
    nodeTextColor: '#f8f8f2',
    actorBorder: '#e135ff',
    actorBkg: '#2d1b4e',
    actorTextColor: '#f8f8f2',
    actorLineColor: '#80ffea',
    signalColor: '#80ffea',
    signalTextColor: '#f8f8f2',
    labelBoxBkgColor: '#1e1e28',
    labelBoxBorderColor: '#e135ff',
    labelTextColor: '#f8f8f2',
    loopTextColor: '#80ffea',
    noteBorderColor: '#ff6ac1',
    noteBkgColor: '#2a1525',
    noteTextColor: '#f8f8f2',
    activationBorderColor: '#80ffea',
    activationBkgColor: '#1e1e28',
    sequenceNumberColor: '#121218',
    fontSize: '14px',
    fontFamily: "'JetBrains Mono', 'Fira Code', monospace",
  };

  // SilkCircuit Dawn — light mode diagram palette
  const lightVars = {
    background: '#faf8ff',
    primaryColor: '#ede0ff',
    primaryBorderColor: '#7e2bd5',
    primaryTextColor: '#2b2540',
    secondaryColor: '#e0f5f5',
    secondaryBorderColor: '#007f8e',
    secondaryTextColor: '#2b2540',
    tertiaryColor: '#f1ecff',
    tertiaryBorderColor: '#b40077',
    tertiaryTextColor: '#2b2540',
    lineColor: '#007f8e',
    textColor: '#2b2540',
    mainBkg: '#ede0ff',
    nodeBorder: '#7e2bd5',
    clusterBkg: 'rgba(126, 43, 213, 0.06)',
    clusterBorder: '#7e2bd5',
    titleColor: '#7e2bd5',
    edgeLabelBackground: '#faf8ff',
    nodeTextColor: '#2b2540',
    actorBorder: '#7e2bd5',
    actorBkg: '#ede0ff',
    actorTextColor: '#2b2540',
    actorLineColor: '#007f8e',
    signalColor: '#007f8e',
    signalTextColor: '#2b2540',
    labelBoxBkgColor: '#f1ecff',
    labelBoxBorderColor: '#7e2bd5',
    labelTextColor: '#2b2540',
    loopTextColor: '#007f8e',
    noteBorderColor: '#b40077',
    noteBkgColor: '#fce8f4',
    noteTextColor: '#2b2540',
    activationBorderColor: '#007f8e',
    activationBkgColor: '#f1ecff',
    sequenceNumberColor: '#faf8ff',
    fontSize: '14px',
    fontFamily: "'JetBrains Mono', 'Fira Code', monospace",
  };

  const getMermaidConfig = () => ({
    startOnLoad: false,
    theme: 'base',
    themeVariables: isDark() ? darkVars : lightVars,
    flowchart: { curve: 'basis', htmlLabels: true, padding: 16 },
    sequence: { mirrorActors: false, messageMargin: 40 },
  });

  const showLoading = (elements) => {
    elements.forEach((el) => {
      if (!el.querySelector('.mermaid-loading')) {
        const loader = document.createElement('div');
        loader.className = 'mermaid-loading';
        loader.setAttribute('aria-label', 'Loading diagram');
        loader.textContent = 'Loading diagram…';
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

  // Bright node fills (green/cyan/yellow/coral) leave the default light label
  // invisible in dark mode. Force the label dark wherever the fill is bright.
  const BRIGHT_FILLS = [
    '#50fa7b',
    '#80ffea',
    '#f1fa8c',
    '#ff6ac1',
    'rgb(80, 250, 123)',
    'rgb(128, 255, 234)',
    'rgb(241, 250, 140)',
    'rgb(255, 106, 193)',
  ];

  const fixBrightNodeContrast = (containers) => {
    const dark = '#0a0a0f';
    containers.forEach((container) => {
      container.querySelectorAll('.node').forEach((node) => {
        const shape = node.querySelector('rect, polygon, circle, ellipse, path');
        if (!shape) return;
        const fill = (shape.getAttribute('style') || '').match(/fill:\s*([^;]+)/);
        const fillVal = fill ? fill[1].trim() : shape.getAttribute('fill') || '';
        if (BRIGHT_FILLS.some((b) => fillVal.includes(b))) {
          node
            .querySelectorAll('.nodeLabel, .nodeLabel *, text, tspan')
            .forEach((t) => {
              t.style.setProperty('color', dark, 'important');
              t.style.setProperty('fill', dark, 'important');
            });
        }
      });
    });
  };

  const renderDiagrams = async (elements) => {
    if (!window.mermaid || !elements.length) return;

    window.mermaid.initialize(getMermaidConfig());

    // Reset elements for re-render from the saved source
    elements.forEach((el) => {
      el.removeAttribute('data-processed');
      const src = el.getAttribute('data-mermaid-src');
      if (src) el.innerHTML = src;
    });

    await window.mermaid.run({ nodes: elements });
    fixBrightNodeContrast(elements);
    clearLoading(elements);
  };

  const init = () => {
    const elements = document.querySelectorAll('.mermaid');
    if (!elements.length) return;

    // Save source BEFORE showLoading prepends into the element — otherwise the
    // "Loading diagram…" text gets captured as the diagram source on re-render.
    elements.forEach((el) => {
      if (!el.getAttribute('data-mermaid-src')) {
        el.setAttribute('data-mermaid-src', el.innerHTML);
      }
    });

    showLoading(elements);

    loadMermaid()
      .then(() => renderDiagrams([...elements]))
      .catch((err) => {
        console.error('Mermaid render failed:', err);
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
