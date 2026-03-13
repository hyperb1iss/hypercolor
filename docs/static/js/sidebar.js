/**
 * Hypercolor — Mobile sidebar toggle
 */

(() => {
  const BREAKPOINT = 768;

  const init = () => {
    const toggle = document.getElementById('sidebar-toggle');
    const sidebar = document.querySelector('.site-sidebar');
    if (!toggle || !sidebar) return;

    const overlay = document.querySelector('.site-overlay');

    const isMobile = () => window.innerWidth < BREAKPOINT;

    const isOpen = () => sidebar.classList.contains('site-sidebar--open');

    const open = () => {
      if (!isMobile()) return;
      sidebar.classList.add('site-sidebar--open');
      overlay?.classList.add('site-overlay--active');
      document.body.style.overflow = 'hidden';
      toggle.setAttribute('aria-expanded', 'true');
      sidebar.setAttribute('aria-hidden', 'false');
    };

    const close = () => {
      sidebar.classList.remove('site-sidebar--open');
      overlay?.classList.remove('site-overlay--active');
      document.body.style.overflow = '';
      toggle.setAttribute('aria-expanded', 'false');
      sidebar.setAttribute('aria-hidden', isMobile() ? 'true' : 'false');
    };

    // Set initial ARIA state
    toggle.setAttribute('aria-expanded', 'false');
    toggle.setAttribute('aria-controls', sidebar.id || 'sidebar');
    toggle.setAttribute('aria-label', 'Toggle navigation menu');
    if (isMobile()) sidebar.setAttribute('aria-hidden', 'true');

    // Toggle button
    toggle.addEventListener('click', () => {
      if (isOpen()) {
        close();
      } else {
        open();
      }
    });

    // Overlay click closes sidebar
    overlay?.addEventListener('click', close);

    // Escape key closes sidebar
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && isOpen()) {
        e.preventDefault();
        close();
        toggle.focus();
      }
    });

    // Reset state on viewport resize past breakpoint
    const handleResize = () => {
      if (!isMobile() && isOpen()) {
        close();
      }
      sidebar.setAttribute('aria-hidden', isMobile() && !isOpen() ? 'true' : 'false');
    };

    window.addEventListener('resize', handleResize, { passive: true });
  };

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
