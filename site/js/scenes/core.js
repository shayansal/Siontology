/* Siontology figures: a small runtime for canvas animations.
 *
 * Each figure is a <canvas data-scene="name"> inside a <figure>. Scenes are
 * registered by the files in this folder with Sion.scene(name, definition).
 *
 *  - One requestAnimationFrame loop drives every visible, running canvas.
 *  - A canvas that is off screen is not drawn (IntersectionObserver).
 *  - With prefers-reduced-motion, every scene shows one labelled static frame
 *    and its button offers to play it.
 *  - Every figure has a Pause / Play button ([data-scene-toggle]).
 *  - Colours come from the CSS custom properties, so figures follow the theme.
 *
 * A scene definition:
 *   { duration: seconds per loop (or 0 for an endless timeline),
 *     still: the time of the static frame,
 *     wide: { w, h }, narrow: { w, h }   logical drawing spaces,
 *     draw(ctx, t, view) }                view = { narrow, W, H, u }
 * A scene with free: true draws in CSS pixels instead: view = { w, h }.
 *
 * A scene may have chapters: [{ at: start in seconds, still: seconds into
 * the chapter for its static frame }]. Buttons with data-chapter="i" in its
 * figure go to chapter i, and are pressed while it plays; elements with
 * data-chapter-panel, in the same order, are shown only while theirs plays.
 */
(function () {
  'use strict';

  var SANS = '"Archivo", "Siontology Symbols", system-ui, sans-serif';
  var MONO = '"Fragment Mono", "Siontology Symbols", ui-monospace, monospace';
  var C = {};

  function readColors() {
    var cs = getComputedStyle(document.documentElement);
    var get = function (name) { return cs.getPropertyValue(name).trim(); };
    C.bg = get('--color-bg');
    C.raised = get('--color-bg-raised');
    C.ink = get('--color-ink');
    C.ink2 = get('--color-ink-2');
    C.ink3 = get('--color-ink-3');
    C.rule = get('--color-rule');
    C.rule2 = get('--color-rule-strong');
    C.signal = get('--color-signal');
    C.dark = C.bg.toLowerCase() === '#000000' || C.bg.toLowerCase() === '#000';
  }

  /* ------------------------------------------------------------ maths */
  var clamp = function (v, a, b) { return Math.min(b === undefined ? 1 : b, Math.max(a === undefined ? 0 : a, v)); };
  var lerp = function (a, b, t) { return a + (b - a) * t; };
  var ease = function (t) { return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2; };
  var easeOut = function (t) { return 1 - Math.pow(1 - t, 3); };
  var linear = function (t) { return t; };
  /* progress of t inside [a, b], eased */
  var span = function (t, a, b, fn) { return (fn || ease)(clamp((t - a) / (b - a))); };
  /* fade in at a, fade out at b */
  var window_ = function (t, a, b, fade) {
    fade = fade || 0.3;
    return Math.min(clamp((t - a) / fade), clamp((b - t) / fade));
  };
  function rng(seed) {
    var s = seed >>> 0;
    return function () {
      s = (s + 0x6d2b79f5) | 0;
      var x = Math.imul(s ^ (s >>> 15), 1 | s);
      x = (x + Math.imul(x ^ (x >>> 7), 61 | x)) ^ x;
      return ((x ^ (x >>> 14)) >>> 0) / 4294967296;
    };
  }
  var fmt = function (n) { return Math.round(n).toLocaleString('en-US'); };
  var hex = function (seed, n) {
    var r = rng(seed), s = '';
    for (var i = 0; i < n; i++) s += '0123456789abcdef'[Math.floor(r() * 16)];
    return s;
  };

  /* ---------------------------------------------------------- drawing */
  function font(size, weight, mono) {
    return (weight || 450) + ' ' + size + 'px ' + (mono ? MONO : SANS);
  }

  function rect(ctx, x, y, w, h, o) {
    o = o || {};
    if (w <= 0 || h <= 0) return;
    ctx.save();
    if (o.alpha !== undefined) ctx.globalAlpha *= o.alpha;
    if (o.fill) {
      ctx.fillStyle = o.fill;
      ctx.fillRect(x, y, w, h);
    }
    if (o.stroke) {
      ctx.strokeStyle = o.stroke;
      ctx.lineWidth = o.lw || 1;
      if (o.dash) ctx.setLineDash(o.dash);
      var i = ctx.lineWidth / 2;
      ctx.strokeRect(x + i, y + i, Math.max(0, w - 2 * i), Math.max(0, h - 2 * i));
    }
    ctx.restore();
  }

  function line(ctx, x1, y1, x2, y2, o) {
    o = o || {};
    ctx.save();
    if (o.alpha !== undefined) ctx.globalAlpha *= o.alpha;
    ctx.strokeStyle = o.color || C.ink;
    ctx.lineWidth = o.lw || 1;
    if (o.dash) ctx.setLineDash(o.dash);
    ctx.beginPath();
    ctx.moveTo(x1, y1);
    ctx.lineTo(x2, y2);
    ctx.stroke();
    ctx.restore();
  }

  function poly(ctx, pts, o) {
    o = o || {};
    if (pts.length < 2) return;
    ctx.save();
    if (o.alpha !== undefined) ctx.globalAlpha *= o.alpha;
    ctx.strokeStyle = o.color || C.ink;
    ctx.lineWidth = o.lw || 1;
    ctx.lineJoin = 'miter';
    if (o.dash) ctx.setLineDash(o.dash);
    ctx.beginPath();
    ctx.moveTo(pts[0][0], pts[0][1]);
    for (var i = 1; i < pts.length; i++) ctx.lineTo(pts[i][0], pts[i][1]);
    if (o.close) ctx.closePath();
    if (o.fill) {
      ctx.fillStyle = o.fill;
      ctx.fill();
    }
    if (!o.noStroke) ctx.stroke();
    ctx.restore();
  }

  /* An arrow drawn up to fraction p of its length (p = 1: whole arrow). */
  function arrow(ctx, x1, y1, x2, y2, o) {
    o = o || {};
    var p = o.p === undefined ? 1 : clamp(o.p);
    if (p <= 0) return;
    var xe = lerp(x1, x2, p), ye = lerp(y1, y2, p);
    line(ctx, x1, y1, xe, ye, o);
    if (o.head === false) return;
    var a = Math.atan2(y2 - y1, x2 - x1), s = o.head || 6;
    poly(ctx, [
      [xe - s * Math.cos(a - 0.5), ye - s * Math.sin(a - 0.5)],
      [xe, ye],
      [xe - s * Math.cos(a + 0.5), ye - s * Math.sin(a + 0.5)]
    ], { color: o.color, lw: o.lw, alpha: o.alpha });
  }

  function text(ctx, s, x, y, o) {
    o = o || {};
    ctx.save();
    if (o.alpha !== undefined) ctx.globalAlpha *= o.alpha;
    ctx.font = font(o.size || 13, o.weight || 450, o.mono);
    ctx.fillStyle = o.color || C.ink;
    ctx.textAlign = o.align || 'left';
    ctx.textBaseline = o.base || 'alphabetic';
    if (o.max) ctx.fillText(s, x, y, o.max);
    else ctx.fillText(s, x, y);
    ctx.restore();
  }

  function measure(ctx, s, size, weight, mono) {
    ctx.save();
    ctx.font = font(size || 13, weight || 450, mono);
    var w = ctx.measureText(s).width;
    ctx.restore();
    return w;
  }

  /* Text that wraps to a width; returns the height used. */
  function para(ctx, s, x, y, width, o) {
    o = o || {};
    var size = o.size || 13, lh = o.lh || size * 1.4;
    var words = s.split(' '), lineText = '', yy = y, lines = 0;
    for (var i = 0; i < words.length; i++) {
      var test = lineText ? lineText + ' ' + words[i] : words[i];
      if (lineText && measure(ctx, test, size, o.weight, o.mono) > width) {
        text(ctx, lineText, x, yy, o);
        lineText = words[i];
        yy += lh;
        lines++;
      } else {
        lineText = test;
      }
    }
    if (lineText) {
      text(ctx, lineText, x, yy, o);
      lines++;
    }
    return lines * lh;
  }

  function check(ctx, x, y, s, o) {
    o = o || {};
    poly(ctx, [[x - s * 0.5, y], [x - s * 0.12, y + s * 0.38], [x + s * 0.55, y - s * 0.42]],
      { color: o.color || C.ink, lw: o.lw || 1.8, alpha: o.alpha });
  }

  function cross(ctx, x, y, s, o) {
    o = o || {};
    var c = o.color || C.signal, lw = o.lw || 1.8;
    line(ctx, x - s / 2, y - s / 2, x + s / 2, y + s / 2, { color: c, lw: lw, alpha: o.alpha });
    line(ctx, x + s / 2, y - s / 2, x - s / 2, y + s / 2, { color: c, lw: lw, alpha: o.alpha });
  }

  function roundRect(ctx, x, y, w, h, r) {
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.arcTo(x + w, y, x + w, y + h, r);
    ctx.arcTo(x + w, y + h, x, y + h, r);
    ctx.arcTo(x, y + h, x, y, r);
    ctx.arcTo(x, y, x + w, y, r);
    ctx.closePath();
  }

  /* A light client: a phone outline (the only rounded thing we draw). */
  function phone(ctx, x, y, w, h, o) {
    o = o || {};
    ctx.save();
    if (o.alpha !== undefined) ctx.globalAlpha *= o.alpha;
    roundRect(ctx, x + 0.75, y + 0.75, w - 1.5, h - 1.5, Math.min(10, w * 0.18));
    if (o.fill) {
      ctx.fillStyle = o.fill;
      ctx.fill();
    }
    ctx.strokeStyle = o.stroke || C.ink;
    ctx.lineWidth = o.lw || 1.5;
    ctx.stroke();
    ctx.fillStyle = o.stroke || C.ink;
    ctx.fillRect(x + w / 2 - w * 0.12, y + 5, w * 0.24, 1.5);
    ctx.restore();
  }

  /* An agent: a diamond. */
  function diamond(ctx, cx, cy, r, o) {
    o = o || {};
    poly(ctx, [[cx, cy - r], [cx + r, cy], [cx, cy + r], [cx - r, cy]],
      { close: true, fill: o.fill, color: o.stroke || C.ink, lw: o.lw || 1.5, alpha: o.alpha, noStroke: o.noStroke });
  }

  /* An agent with a face: the diamond, with eyes of solid cells and a mouth
     of lines. o: { look: -1..1, blink: 0..1, eyes: 'square' | 'happy' |
     'closed' | 'goggles', mouth: 'flat' | 'smile' | 'open' | 'o', talk: 0..1,
     think: 0..1 with thinkT (seconds), alpha, lw, stroke }. */
  function face(ctx, cx, cy, r, o) {
    o = o || {};
    var col = o.stroke || C.ink, a = o.alpha === undefined ? 1 : o.alpha;
    diamond(ctx, cx, cy, r, { lw: o.lw || 1.5, alpha: a, stroke: col });
    var s = Math.max(2, r * 0.15), lw = Math.max(1.2, r * 0.07);
    var ex = r * 0.3, ey = cy - r * 0.1, dx = (o.look || 0) * r * 0.1;
    if (o.eyes === 'happy') {
      [-1, 1].forEach(function (k) {
        var x = cx + k * ex + dx;
        poly(ctx, [[x - s * 0.8, ey + s * 0.45], [x, ey - s * 0.45], [x + s * 0.8, ey + s * 0.45]], { color: col, lw: lw, alpha: a });
      });
    } else if (o.eyes === 'closed') {
      [-1, 1].forEach(function (k) {
        var x = cx + k * ex;
        line(ctx, x - s * 0.8, ey + s * 0.2, x + s * 0.8, ey + s * 0.2, { color: col, lw: lw, alpha: a });
      });
    } else {
      var h = s * (1 - 0.85 * clamp(o.blink || 0));
      [-1, 1].forEach(function (k) {
        var x = cx + k * ex + dx;
        if (o.eyes === 'goggles') rect(ctx, cx + k * ex - s * 1.15, ey - s * 1.15, s * 2.3, s * 2.3, { stroke: col, lw: 1, alpha: a });
        rect(ctx, x - s / 2, ey - h / 2, s, h, { fill: col, alpha: a });
      });
      if (o.eyes === 'goggles') line(ctx, cx - ex + s * 1.15, ey, cx + ex - s * 1.15, ey, { color: col, lw: 1, alpha: a });
    }
    var my = cy + r * 0.32, mw = r * 0.42, talk = clamp(o.talk || 0);
    if (talk > 0) {
      rect(ctx, cx - mw * 0.3, my - s * 0.35, mw * 0.6, s * 0.35 + talk * s * 0.8, { fill: col, alpha: a });
    } else if (o.mouth === 'smile') {
      poly(ctx, [[cx - mw / 2, my - s * 0.45], [cx - mw / 4, my + s * 0.2], [cx + mw / 4, my + s * 0.2], [cx + mw / 2, my - s * 0.45]],
        { color: col, lw: lw, alpha: a });
    } else if (o.mouth === 'open') {
      rect(ctx, cx - mw * 0.3, my - s * 0.35, mw * 0.6, s * 0.7, { fill: col, alpha: a });
    } else if (o.mouth === 'o') {
      rect(ctx, cx - s * 0.6, my - s * 0.6, s * 1.2, s * 1.2, { stroke: col, lw: 1.2, alpha: a });
    } else {
      line(ctx, cx - mw / 2, my, cx + mw / 2, my, { color: col, lw: lw, alpha: a });
    }
    if (o.think > 0) {
      for (var i = 0; i < 3; i++) {
        var on = Math.floor((o.thinkT || 0) * 3) % 3 === i ? 1 : 0.3;
        rect(ctx, cx - s * 1.9 + i * s * 1.4, cy - r - s * 2.4, s, s, { fill: col, alpha: a * o.think * on });
      }
    }
  }

  /* A blink every period seconds, offset so that faces do not blink together;
     none in a still frame (a paused scene, or Sion.render), so that a face
     at rest keeps its eyes open. */
  var still = false;
  function blink(t, period, offset) {
    if (still) return 0;
    var p = (t + (offset || 0)) % period;
    return p > period - 0.18 ? window_(p, period - 0.18, period, 0.07) : 0;
  }

  function hatch(ctx, x, y, w, h, o) {
    o = o || {};
    ctx.save();
    if (o.alpha !== undefined) ctx.globalAlpha *= o.alpha;
    ctx.beginPath();
    ctx.rect(x, y, w, h);
    ctx.clip();
    ctx.strokeStyle = o.color || C.ink;
    ctx.lineWidth = o.lw || 1;
    var gap = o.gap || 4;
    ctx.beginPath();
    for (var d = -h; d < w; d += gap) {
      ctx.moveTo(x + d, y + h);
      ctx.lineTo(x + d + h, y);
    }
    ctx.stroke();
    ctx.restore();
  }

  /* A small square travelling from (x1, y1) to (x2, y2). */
  function packet(ctx, x1, y1, x2, y2, p, o) {
    o = o || {};
    if (p <= 0 || p >= 1) return;
    var s = o.size || 6;
    rect(ctx, lerp(x1, x2, p) - s / 2, lerp(y1, y2, p) - s / 2, s, s,
      { fill: o.color || C.ink, alpha: o.alpha });
  }

  /* Tally marks: approvals counted toward a quorum. */
  function tally(ctx, x, y, n, o) {
    o = o || {};
    var h = o.h || 14, gap = o.gap || 5;
    for (var i = 0; i < n; i++) line(ctx, x + i * gap, y, x + i * gap, y + h, { color: o.color || C.ink, lw: o.lw || 1.5, alpha: o.alpha });
  }

  /* ----------------------------------------------------------- runtime */
  var defs = {};
  var runners = [];
  var reduced = false;
  var raf = 0;
  var last = 0;

  function Runner(canvas, def) {
    this.canvas = canvas;
    this.ctx = canvas.getContext('2d');
    this.def = def;
    this.t = def.start || 0;
    this.visible = false;
    this.paused = false;
    this.dirty = true;
    this.w = 0;
    this.h = 0;
    var figure = canvas.closest('figure');
    this.button = figure ? figure.querySelector('[data-scene-toggle]') : null;
    this.chapterButtons = figure ? Array.prototype.slice.call(figure.querySelectorAll('[data-chapter]')) : [];
    this.chapterPanels = figure ? Array.prototype.slice.call(figure.querySelectorAll('[data-chapter-panel]')) : [];
    this.chapter = -1;
  }

  /* Show which chapter is playing: its button pressed, its caption shown,
     and how far it has played as --progress (0 to 1) on its button. */
  Runner.prototype.syncChapter = function (t) {
    var ch = this.def.chapters;
    if (!ch || (!this.chapterButtons.length && !this.chapterPanels.length)) return;
    var i = 0;
    while (i + 1 < ch.length && t >= ch[i + 1].at) i++;
    var end = i + 1 < ch.length ? ch[i + 1].at : this.def.duration;
    var b = this.chapterButtons[i];
    if (b && end > ch[i].at) b.style.setProperty('--progress', clamp((t - ch[i].at) / (end - ch[i].at), 0, 1).toFixed(3));
    if (i === this.chapter) return;
    this.chapter = i;
    this.chapterButtons.forEach(function (b, k) { b.setAttribute('aria-pressed', k === i ? 'true' : 'false'); });
    this.chapterPanels.forEach(function (p, k) { p.hidden = k !== i; });
  };

  /* Go to chapter i: from its start while playing, to its still while paused. */
  Runner.prototype.goTo = function (i) {
    var c = this.def.chapters[i];
    this.t = c.at + (this.paused ? (c.still || 0) : 0);
    this.dirty = true;
    kick();
  };

  Runner.prototype.resize = function () {
    var r = this.canvas.getBoundingClientRect();
    var dpr = Math.min(window.devicePixelRatio || 1, 2);
    if (r.width < 1 || r.height < 1) return;
    this.w = r.width;
    this.h = r.height;
    this.dpr = dpr;
    this.canvas.width = Math.round(r.width * dpr);
    this.canvas.height = Math.round(r.height * dpr);
    this.dirty = true;
  };

  Runner.prototype.setPaused = function (paused) {
    this.paused = paused;
    if (this.button) {
      this.button.setAttribute('aria-pressed', paused ? 'true' : 'false');
      var label = this.button.querySelector('[data-label]');
      if (label) label.textContent = paused ? 'Play' : 'Pause';
    }
    this.dirty = true;
    kick();
  };

  Runner.prototype.draw = function () {
    var ctx = this.ctx, def = this.def, w = this.w, h = this.h, dpr = this.dpr;
    if (!w || !h) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    var t = this.t;
    if (def.duration) t = t % def.duration;
    this.syncChapter(t);
    if (def.free) {
      def.draw(ctx, t, { w: w, h: h, raw: this.t });
      return;
    }
    var narrow = w / h < 1.2 && def.narrow;
    var L = narrow ? def.narrow : def.wide;
    var u = Math.min(w / L.w, h / L.h);
    var ox = (w - L.w * u) / 2, oy = (h - L.h * u) / 2;
    ctx.setTransform(dpr * u, 0, 0, dpr * u, dpr * ox, dpr * oy);
    still = this.paused;
    def.draw(ctx, t, { narrow: !!narrow, W: L.w, H: L.h, u: u });
    still = false;
  };

  function frame(now) {
    var dt = last ? Math.min(0.1, (now - last) / 1000) : 0;
    last = now;
    var running = false;
    for (var i = 0; i < runners.length; i++) {
      var r = runners[i];
      if (!r.visible) continue;
      if (!r.paused) {
        r.t += dt;
        r.dirty = true;
        running = true;
      }
      if (r.dirty) {
        r.draw();
        r.dirty = false;
      }
    }
    if (running) {
      raf = requestAnimationFrame(frame);
    } else {
      raf = 0;
      last = 0;
    }
  }

  function kick() {
    if (!raf) {
      last = 0;
      raf = requestAnimationFrame(frame);
    }
  }

  function redrawAll() {
    readColors();
    runners.forEach(function (r) { r.dirty = true; });
    kick();
  }

  function init() {
    readColors();
    var motion = window.matchMedia ? window.matchMedia('(prefers-reduced-motion: reduce)') : null;
    reduced = !!(motion && motion.matches);

    document.querySelectorAll('canvas[data-scene]').forEach(function (canvas) {
      var def = defs[canvas.getAttribute('data-scene')];
      if (!def) return;
      var r = new Runner(canvas, def);
      runners.push(r);
      if (reduced) {
        r.t = def.still || 0;
        r.setPaused(true);
      }
      if (r.button) {
        r.button.hidden = false;
        r.button.addEventListener('click', function () {
          r.setPaused(!r.paused);
        });
      }
      r.chapterButtons.forEach(function (b, k) {
        b.addEventListener('click', function () { r.goTo(k); });
      });
      r.resize();
    });

    if ('IntersectionObserver' in window) {
      var io = new IntersectionObserver(function (entries) {
        entries.forEach(function (entry) {
          runners.forEach(function (r) {
            if (r.canvas === entry.target) {
              r.visible = entry.isIntersecting;
              if (r.visible) r.dirty = true;
            }
          });
        });
        kick();
      }, { rootMargin: '80px 0px' });
      runners.forEach(function (r) { io.observe(r.canvas); });
    } else {
      runners.forEach(function (r) { r.visible = true; });
    }

    if ('ResizeObserver' in window) {
      var ro = new ResizeObserver(function (entries) {
        entries.forEach(function (entry) {
          runners.forEach(function (r) { if (r.canvas === entry.target) r.resize(); });
        });
        kick();
      });
      runners.forEach(function (r) { ro.observe(r.canvas); });
    } else {
      window.addEventListener('resize', function () {
        runners.forEach(function (r) { r.resize(); });
        kick();
      });
    }

    document.addEventListener('sion:themechange', function () {
      /* let the new custom properties apply first */
      requestAnimationFrame(redrawAll);
    });
    if (motion) {
      var onMotion = function () {
        reduced = motion.matches;
        runners.forEach(function (r) {
          if (reduced) {
            r.t = r.def.still || 0;
            r.setPaused(true);
          }
        });
      };
      if (motion.addEventListener) motion.addEventListener('change', onMotion);
      else if (motion.addListener) motion.addListener(onMotion);
    }
    if (document.fonts && document.fonts.load) {
      Promise.all([
        document.fonts.load(font(13, 450)),
        document.fonts.load(font(13, 600)),
        document.fonts.load(font(13, 400, true)),
        document.fonts.load(font(13, 450) , '∧∨∞≥φκ→✓')
      ]).then(redrawAll, redrawAll);
    }
    kick();
  }

  window.Sion = {
    scene: function (name, def) { defs[name] = def; },
    /* Draw one frame of a scene at time t (used for stills and tests). */
    render: function (canvas, name, t) {
      readColors();
      var r = new Runner(canvas, defs[name]);
      r.resize();
      r.t = t;
      r.paused = true;
      r.draw();
    },
    C: C,
    u: {
      clamp: clamp, lerp: lerp, ease: ease, easeOut: easeOut, linear: linear, span: span, win: window_,
      rng: rng, fmt: fmt, hex: hex, font: font, rect: rect, line: line, poly: poly, arrow: arrow,
      text: text, measure: measure, para: para, check: check, cross: cross, phone: phone,
      diamond: diamond, face: face, blink: blink, hatch: hatch, packet: packet, tally: tally
    }
  };

  /* Scene files load after this one (all deferred), so wait for the DOM. */
  var started = false;
  var start = function () {
    if (started) return;
    started = true;
    init();
  };
  if (document.readyState === 'complete') window.setTimeout(start, 0);
  else document.addEventListener('DOMContentLoaded', start);
})();
