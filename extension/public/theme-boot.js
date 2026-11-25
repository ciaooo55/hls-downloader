try {
  const cached = localStorage.getItem('hlsd_resolved_theme')
  const resolved = cached === 'dark' || cached === 'light'
    ? cached
    : (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light')
  document.documentElement.setAttribute('data-hlsd-theme', resolved)
} catch {}
