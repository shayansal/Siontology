/* Siontology website: theme, menu, copy buttons, contents highlighting.
   No trackers, no cookies. The theme choice is kept in localStorage only
   after you press the theme button. */
(function () {
  'use strict';

  var root = document.documentElement;
  root.classList.add('js');
  var KEY = 'siontology-theme';

  function systemTheme() {
    return window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  }

  function currentTheme() {
    var set = root.getAttribute('data-theme');
    return set === 'light' || set === 'dark' ? set : systemTheme();
  }

  function labelThemeButtons() {
    var next = currentTheme() === 'dark' ? 'light' : 'dark';
    document.querySelectorAll('[data-theme-toggle]').forEach(function (button) {
      button.setAttribute('aria-label', 'Switch to ' + next + ' theme');
      button.setAttribute('title', 'Switch to ' + next + ' theme');
    });
  }

  document.querySelectorAll('[data-theme-toggle]').forEach(function (button) {
    button.addEventListener('click', function () {
      var next = currentTheme() === 'dark' ? 'light' : 'dark';
      root.setAttribute('data-theme', next);
      try {
        window.localStorage.setItem(KEY, next);
      } catch (error) {
        /* storage unavailable: the choice lasts for this page only */
      }
      labelThemeButtons();
      document.dispatchEvent(new CustomEvent('sion:themechange'));
    });
  });

  if (window.matchMedia) {
    var query = window.matchMedia('(prefers-color-scheme: dark)');
    var onSystemChange = function () {
      labelThemeButtons();
      document.dispatchEvent(new CustomEvent('sion:themechange'));
    };
    if (query.addEventListener) query.addEventListener('change', onSystemChange);
    else if (query.addListener) query.addListener(onSystemChange);
  }
  labelThemeButtons();

  /* Menu on narrow screens */
  var menuButton = document.querySelector('[data-menu-toggle]');
  var nav = document.getElementById('site-nav');
  if (menuButton && nav) {
    var setOpen = function (open) {
      nav.classList.toggle('is-open', open);
      menuButton.setAttribute('aria-expanded', open ? 'true' : 'false');
    };
    menuButton.addEventListener('click', function () {
      setOpen(menuButton.getAttribute('aria-expanded') !== 'true');
    });
    document.addEventListener('keydown', function (event) {
      if (event.key === 'Escape' && menuButton.getAttribute('aria-expanded') === 'true') {
        setOpen(false);
        menuButton.focus();
      }
    });
    nav.addEventListener('click', function (event) {
      if (event.target.closest('a')) setOpen(false);
    });
  }

  /* Copy buttons on code blocks */
  document.querySelectorAll('[data-copy]').forEach(function (button) {
    button.addEventListener('click', function () {
      var source = document.getElementById(button.getAttribute('data-copy'));
      if (!source) return;
      var text = source.innerText.replace(/\n$/, '');
      var done = function (ok) {
        button.textContent = ok ? 'Copied' : 'Select and copy';
        window.setTimeout(function () { button.textContent = 'Copy'; }, 1800);
      };
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(function () { done(true); }, function () { done(false); });
      } else {
        var range = document.createRange();
        range.selectNodeContents(source);
        var selection = window.getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
        done(false);
      }
    });
  });

  /* The team's faces (site.css, Team): one button pauses them all, and while
     they move their eyes follow the pointer. With reduced motion they start
     paused, and the button offers Play. */
  document.querySelectorAll('[data-faces]').forEach(function (team) {
    var motion = window.matchMedia ? window.matchMedia('(prefers-reduced-motion: reduce)') : null;
    var button = team.querySelector('[data-faces-toggle]');
    var faces = Array.prototype.slice.call(team.querySelectorAll('.face'));
    var paused = false;
    var rest = function () {
      faces.forEach(function (face) {
        face.style.removeProperty('--lx');
        face.style.removeProperty('--ly');
      });
    };
    var setPaused = function (value) {
      paused = value;
      team.classList.toggle('is-paused', paused);
      if (button) {
        button.setAttribute('aria-pressed', paused ? 'true' : 'false');
        var label = button.querySelector('[data-label]');
        if (label) label.textContent = paused ? 'Play' : 'Pause';
      }
      if (paused) rest();
    };
    setPaused(!!(motion && motion.matches));
    if (button) {
      button.hidden = false;
      button.addEventListener('click', function () { setPaused(!paused); });
    }
    var x = 0, y = 0, queued = false;
    var look = function () {
      queued = false;
      if (paused) return;
      var h = window.innerHeight;
      faces.forEach(function (face) {
        var r = face.getBoundingClientRect();
        if (r.bottom < 0 || r.top > h) return;
        var dx = x - (r.left + r.width / 2), dy = y - (r.top + r.height / 2);
        var d = Math.sqrt(dx * dx + dy * dy) || 1, k = Math.min(1, d / 240);
        face.style.setProperty('--lx', (dx / d * 3.5 * k).toFixed(2) + 'px');
        face.style.setProperty('--ly', (dy / d * 2.5 * k).toFixed(2) + 'px');
      });
    };
    document.addEventListener('pointermove', function (event) {
      if (paused) return;
      x = event.clientX;
      y = event.clientY;
      if (!queued) {
        queued = true;
        window.requestAnimationFrame(look);
      }
    }, { passive: true });
    document.documentElement.addEventListener('pointerleave', rest);
  });

  /* Highlight the current section in a table of contents */
  var toc = document.querySelector('[data-toc]');
  if (toc && 'IntersectionObserver' in window) {
    var links = {};
    toc.querySelectorAll('a[href^="#"]').forEach(function (link) {
      links[link.getAttribute('href').slice(1)] = link;
    });
    var visible = {};
    var observer = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        visible[entry.target.id] = entry.isIntersecting;
      });
      var first = Object.keys(links).find(function (id) { return visible[id]; });
      Object.keys(links).forEach(function (id) {
        if (id === first) links[id].setAttribute('aria-current', 'true');
        else links[id].removeAttribute('aria-current');
      });
    }, { rootMargin: '-20% 0px -60% 0px' });
    Object.keys(links).forEach(function (id) {
      var section = document.getElementById(id);
      if (section) observer.observe(section);
    });
  }
})();
