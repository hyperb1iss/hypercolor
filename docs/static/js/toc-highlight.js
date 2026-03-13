/**
 * Hypercolor — Table of contents active heading tracker
 */

(() => {
  const init = () => {
    const toc = document.querySelector('.site-toc');
    const docContent = document.querySelector('.doc');
    if (!toc || !docContent) return;

    const tocLinks = [...toc.querySelectorAll('.toc-link')];
    if (!tocLinks.length) return;

    const headings = [...docContent.querySelectorAll('h2, h3')].filter((h) => h.id);
    if (!headings.length) return;

    // Build lookup: heading id -> toc link
    const linkMap = new Map();
    tocLinks.forEach((link) => {
      const hash = new URL(link.href, location.href).hash.slice(1);
      if (hash) linkMap.set(hash, link);
    });

    const setActive = (id) => {
      tocLinks.forEach((link) => link.classList.remove('toc-link--active'));
      const target = linkMap.get(id);
      if (target) {
        target.classList.add('toc-link--active');
        target.setAttribute('aria-current', 'true');
      }
      tocLinks
        .filter((l) => l !== target)
        .forEach((l) => l.removeAttribute('aria-current'));
    };

    // IntersectionObserver to track visible headings
    const visibleHeadings = new Set();

    const observer = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (entry.isIntersecting) {
            visibleHeadings.add(entry.target.id);
          } else {
            visibleHeadings.delete(entry.target.id);
          }
        });

        // Activate the topmost visible heading
        if (visibleHeadings.size > 0) {
          const topmost = headings.find((h) => visibleHeadings.has(h.id));
          if (topmost) setActive(topmost.id);
        }
      },
      {
        rootMargin: '-64px 0px -75% 0px',
        threshold: 0,
      }
    );

    headings.forEach((h) => observer.observe(h));

    // Edge case: scrolled to bottom — activate last heading
    const handleScroll = () => {
      const atBottom =
        window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 50;
      if (atBottom && headings.length) {
        setActive(headings[headings.length - 1].id);
      }
    };

    window.addEventListener('scroll', handleScroll, { passive: true });

    // Smooth scroll on TOC link click
    toc.addEventListener('click', (e) => {
      const link = e.target.closest('.toc-link');
      if (!link) return;

      const hash = new URL(link.href, location.href).hash.slice(1);
      const target = document.getElementById(hash);
      if (!target) return;

      e.preventDefault();
      target.scrollIntoView({ behavior: 'smooth', block: 'start' });
      history.replaceState(null, '', `#${hash}`);
      setActive(hash);
    });
  };

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
