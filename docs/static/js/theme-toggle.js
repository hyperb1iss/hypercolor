/**
 * Hypercolor — Dark/light theme toggle
 * Runs early to prevent FOUC. Sets data-theme on <html> before paint.
 */

(() => {
  const STORAGE_KEY = 'theme';
  const DARK = 'dark';
  const LIGHT = 'light';

  const getPreferred = () => {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === DARK || stored === LIGHT) return stored;
    // Dark is the brand — RGB lighting apps live in the dark
    return DARK;
  };

  const apply = (theme) => {
    document.documentElement.setAttribute('data-theme', theme);
    localStorage.setItem(STORAGE_KEY, theme);
    window.dispatchEvent(new CustomEvent('themechange', { detail: { theme } }));
  };

  // Apply immediately — no waiting for DOMContentLoaded
  apply(getPreferred());

  const updateButton = (btn, theme) => {
    const isDark = theme === DARK;
    btn.textContent = isDark ? '\u2600\uFE0F' : '\uD83C\uDF19';
    btn.setAttribute('aria-label', isDark ? 'Switch to light mode' : 'Switch to dark mode');
  };

  const init = () => {
    const btn = document.getElementById('theme-toggle');
    if (!btn) return;

    const current = () => document.documentElement.getAttribute('data-theme') || DARK;

    updateButton(btn, current());

    btn.addEventListener('click', () => {
      const next = current() === DARK ? LIGHT : DARK;
      apply(next);
      updateButton(btn, next);
    });

    // Sync if another tab changes the theme
    window.addEventListener('storage', (e) => {
      if (e.key !== STORAGE_KEY) return;
      const theme = e.newValue === LIGHT ? LIGHT : DARK;
      apply(theme);
      updateButton(btn, theme);
    });
  };

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
