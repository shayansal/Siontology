/* What your agent can do: six short runs, one after another, each of one
 * mechanism, with the numbers of a recording in the book.
 *
 *  1. Apps: one agent on many messengers; a voice note answered aloud; a
 *     stranger given pairing code QBRS-9XRF and let in (agent/voice.md).
 *  2. Sandbox: primes.py written and run (17 primes below 60); /usr/local
 *     read-only; no network (agent/tools.md).
 *  3. Memory: a log only it appends to, its head on the ledger; a rewrite
 *     refused, "the new head does not extend the log" (the robots demo).
 *  4. Schedule: every weekday at 09:00 UTC, "How much budget is left?",
 *     answered "85 USD left in the budget." (agent/schedules.md).
 *  5. Helpers: 12 researchers with 40 USD each, at most 10 a payment; 3 more
 *     with 10 each, at most 5; 6 USD refused; a suspension refuses the whole
 *     tree until it is lifted (the swarm demo).
 *  6. Skills: a release on a 150 USD bond, refused for 100 USD at risk with
 *     κ = 2; an audit adds 100 USD, and 250 covers 200 (agent/skills.md).
 *
 * The chapter buttons and captions beside the canvas follow def.chapters.
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;

  /* --------------------------------------------------------- helpers */
  function box(ctx, x, y, w, h, title, o) {
    o = o || {};
    U.rect(ctx, x, y, w, h, { stroke: o.stroke || C.ink, lw: o.lw || 1.25, alpha: o.alpha });
    if (o.on > 0) U.rect(ctx, x, y, w, h, { fill: C.ink, alpha: o.on * 0.08 * (o.alpha === undefined ? 1 : o.alpha) });
    if (title) {
      U.text(ctx, title, x + (o.pad || 12), y + (o.ty || 22), { size: o.size || 13, weight: o.weight || 600, alpha: o.alpha });
    }
  }

  function note(ctx, s, x, y, o) {
    o = o || {};
    U.text(ctx, s, x, y, { size: o.size || 11, color: o.color || C.ink2, align: o.align, alpha: o.alpha, mono: o.mono, weight: o.weight });
  }

  /* A packet from a to b during [t0, t1], with its dashed path while it moves. */
  function travel(ctx, t, a, b, t0, t1, o) {
    o = o || {};
    var vis = U.win(t, t0, t1 + (o.linger === undefined ? 0.6 : o.linger), 0.15);
    if (vis <= 0) return;
    U.line(ctx, a.x, a.y, b.x, b.y, { color: o.color || C.ink2, alpha: vis * 0.45, dash: [2, 3] });
    var p = U.span(t, t0, t1, U.linear);
    if (p > 0 && p < 1) U.packet(ctx, a.x, a.y, b.x, b.y, p, { size: 7, color: o.color || C.ink });
    if (o.label) {
      U.text(ctx, o.label, (a.x + b.x) / 2 + (o.dx || 0), (a.y + b.y) / 2 + (o.dy === undefined ? -8 : o.dy),
        { size: o.size || 11, color: o.labelColor || C.ink2, align: o.align || 'center', alpha: vis, mono: o.mono });
    }
  }

  /* A move toward b that stops at a wall, refused there. */
  function blocked(ctx, t, a, wall, t0, t1, label, o) {
    o = o || {};
    var vis = U.win(t, t0, o.until || 99, 0.15);
    if (vis <= 0) return;
    var p = U.span(t, t0, t1, U.linear);
    var x = U.lerp(a.x, wall.x, p), y = U.lerp(a.y, wall.y, p);
    U.line(ctx, a.x, a.y, x, y, { color: C.ink2, alpha: vis * 0.5, dash: [2, 3] });
    if (p < 1) {
      U.packet(ctx, a.x, a.y, wall.x, wall.y, p, { size: 7 });
    } else {
      U.cross(ctx, wall.x, wall.y, 10, { alpha: vis });
      if (label) {
        U.text(ctx, label, wall.x + (o.dx === undefined ? 0 : o.dx), wall.y + (o.dy === undefined ? 22 : o.dy),
          { size: o.size || 11, color: C.signal, align: o.align || 'center', alpha: vis, mono: o.mono });
      }
    }
  }

  /* ---------------------------------------------------- 1. your apps */
  var APPS = ['Terminal', 'Telegram', 'WhatsApp', 'Slack', 'Email'];
  var apps = {
    dur: 6.6, still: 5.6,
    steps: [
      { at: 0, text: 'One agent answers you on the apps you already use: 28 messaging services, and your terminal.' },
      { at: 1.3, text: 'A voice note on Telegram: “How much budget is left?” It answers aloud: 100 USD.' },
      { at: 3.2, text: 'Someone it does not know writes “Hello?”. They get a pairing code, and no answer.' },
      { at: 4.9, text: 'You let them in with that code; it answers them from their next message.' }
    ],
    draw: function (ctx, t, N) {
      var boxes = APPS.map(function (name, i) {
        return N ? { x: i % 2 ? 186 : 12, y: 14 + Math.floor(i / 2) * 46, w: 162, h: 36, name: name }
                 : { x: 40, y: 40 + i * 64, w: 210, h: 44, name: name };
      });
      var A = N ? { x: 180, y: 230, r: 24 } : { x: 470, y: 200, r: 30 };
      var stranger = N ? { x: 12, y: 316, w: 336, h: 48 } : { x: 650, y: 150, w: 270, h: 56 };
      var hot = function (i) {
        var intro = U.win(t, 0.15 + i * 0.18, 0.9 + i * 0.18, 0.2) * 0.8;
        return i === 1 ? Math.max(intro, U.win(t, 1.3, 3.0, 0.2)) : intro;
      };
      boxes.forEach(function (b, i) {
        box(ctx, b.x, b.y, b.w, b.h, b.name, { on: hot(i), size: N ? 12 : 14, ty: N ? 23 : 27 });
      });
      note(ctx, 'and 24 more messaging services', N ? 12 : 40, N ? 162 : 370, { color: C.ink3, size: N ? 11 : 12 });

      U.diamond(ctx, A.x, A.y, A.r, { lw: 1.6 });
      U.text(ctx, 'your agent', A.x, A.y + A.r + 18, { size: N ? 12 : 13, weight: 600, align: 'center' });

      /* the voice note, and the answer spoken */
      var tg = boxes[1];
      var tgPort = N ? { x: tg.x + tg.w / 2, y: tg.y + tg.h + 2 } : { x: tg.x + tg.w + 2, y: tg.y + tg.h / 2 };
      var aPort = N ? { x: A.x + 4, y: A.y - A.r - 4 } : { x: A.x - A.r - 4, y: A.y };
      travel(ctx, t, tgPort, aPort, 1.4, 2.0, { label: N ? '' : 'voice note', dy: -10 });
      travel(ctx, t, aPort, tgPort, 2.2, 2.8, { label: N ? '' : '“100 USD left”, aloud', dy: 16 });

      /* a stranger, a code, and you letting them in */
      var sa = U.span(t, 3.2, 3.5);
      if (sa > 0) {
        box(ctx, stranger.x, stranger.y, stranger.w, stranger.h, 'someone new, on Telegram', { stroke: C.rule2, alpha: sa, size: N ? 12 : 13 });
        note(ctx, '“Hello?”', stranger.x + 12, stranger.y + (N ? 38 : 40), { alpha: sa, color: C.ink2, size: 12 });
      }
      var sPort = N ? { x: stranger.x + stranger.w / 2, y: stranger.y - 2 } : { x: stranger.x - 2, y: stranger.y + stranger.h / 2 };
      var aPort2 = N ? { x: A.x, y: A.y + A.r + 26 } : { x: A.x + A.r + 4, y: A.y };
      travel(ctx, t, sPort, aPort2, 3.4, 3.9, { linger: 0.3 });
      travel(ctx, t, aPort2, sPort, 4.0, 4.5, { linger: 0.3 });
      var code = U.win(t, 4.3, 99, 0.2);
      if (code > 0) {
        var cy = stranger.y + stranger.h + (N ? 22 : 26);
        note(ctx, 'pairing code QBRS-9XRF', stranger.x, cy, { mono: true, color: C.ink, size: 12, alpha: code });
        var let_ = U.span(t, 5.0, 5.3);
        if (let_ > 0) {
          U.check(ctx, stranger.x + stranger.w - 14, stranger.y + 16, 10, { alpha: let_ });
          note(ctx, 'let in', stranger.x + stranger.w - 26, stranger.y + 21, { align: 'right', alpha: let_, color: C.ink });
          note(ctx, '$ sion agent pairing approve QBRS-9XRF', stranger.x, cy + (N ? 20 : 22), { mono: true, size: N ? 10.5 : 11, alpha: let_ });
        }
      }
    }
  };

  /* ------------------------------------------------------ 2. sandbox */
  var sandbox = {
    dur: 6.6, still: 5.9,
    steps: [
      { at: 0, text: 'Ask it to get something done: it writes a script in its own folder, and runs it.' },
      { at: 1.6, text: '“Count the primes below 60”: primes.py, run in its sandbox, says 17.' },
      { at: 3.2, text: 'Outside its folder, programs are read-only and your files out of sight: writing to /usr/local fails.' },
      { at: 4.8, text: 'And it reaches no network unless you give it one.' }
    ],
    draw: function (ctx, t, N) {
      var sb = N ? { x: 12, y: 12, w: 336, h: 300 } : { x: 40, y: 30, w: 540, h: 400 };
      var ws = N ? { x: 28, y: 92, w: 304, h: 196 } : { x: 70, y: 140, w: 300, h: 260 };
      var A = N ? { x: 60, y: 54, r: 18 } : { x: 110, y: 88, r: 24 };
      var sys = N ? { x: 12, y: 340, w: 160, h: 86 } : { x: 650, y: 120, w: 270, h: 110 };
      var net = N ? { x: 188, y: 340, w: 160, h: 86 } : { x: 650, y: 280, w: 270, h: 110 };

      box(ctx, sb.x, sb.y, sb.w, sb.h, 'its sandbox', { lw: 1.5, size: N ? 12 : 13 });
      note(ctx, 'bubblewrap or Docker', sb.x + sb.w - 12, sb.y + 22, { align: 'right', color: C.ink3 });
      U.diamond(ctx, A.x, A.y, A.r, { lw: 1.5 });
      note(ctx, 'your agent', A.x + A.r + 10, A.y + 4, { color: C.ink, size: 12, weight: 600 });
      box(ctx, ws.x, ws.y, ws.w, ws.h, 'its folder', { stroke: C.rule2, size: N ? 12 : 13, weight: 500 });
      if (N) note(ctx, 'the only place it may write', ws.x + ws.w - 12, ws.y + 22, { align: 'right', color: C.ink3 });
      else note(ctx, 'the only place it may write', ws.x + 12, ws.y + 42, { color: C.ink3 });

      /* the file, then its output */
      var fy = N ? 40 : 62;
      var f = U.span(t, 0.8, 1.1);
      if (f > 0) {
        U.rect(ctx, ws.x + 16, ws.y + fy, N ? 120 : 150, 30, { stroke: C.ink, lw: 1.25, alpha: f });
        note(ctx, 'primes.py', ws.x + 28, ws.y + fy + 20, { mono: true, size: 12, color: C.ink, alpha: f });
      }
      var run = U.span(t, 1.8, 2.1);
      if (run > 0) {
        note(ctx, '$ python3 primes.py', ws.x + 16, ws.y + fy + 58, { mono: true, size: N ? 11 : 12, alpha: run });
        var out = U.span(t, 2.4, 2.7);
        note(ctx, '17 primes below 60', ws.x + 16, ws.y + fy + 80, { mono: true, size: N ? 11 : 12, color: C.ink, alpha: out });
        if (out > 0) U.check(ctx, ws.x + (N ? 170 : 200), ws.y + fy + 76, 10, { alpha: out });
      }

      /* outside: read-only programs, and the network */
      box(ctx, sys.x, sys.y, sys.w, sys.h, 'your computer', { stroke: C.rule2, size: N ? 12 : 13 });
      note(ctx, '/usr/local', sys.x + 12, sys.y + (N ? 42 : 46), { mono: true, color: C.ink3 });
      box(ctx, net.x, net.y, net.w, net.h, 'the internet', { stroke: C.rule2, size: N ? 12 : 13 });
      note(ctx, 'example.com', net.x + 12, net.y + (N ? 42 : 46), { mono: true, color: C.ink3 });

      if (N) {
        blocked(ctx, t, { x: 92, y: 290 }, { x: 92, y: sb.y + sb.h }, 3.3, 3.8, 'Read-only file system', { dy: 18, size: 10.5, mono: true, dx: 0 });
        blocked(ctx, t, { x: 268, y: 290 }, { x: 268, y: sb.y + sb.h }, 4.9, 5.4, 'no network', { dy: 18, size: 10.5, mono: true });
      } else {
        var wall = sb.x + sb.w, y1 = sys.y + sys.h / 2, y2 = net.y + net.h / 2;
        blocked(ctx, t, { x: ws.x + ws.w + 2, y: y1 }, { x: wall, y: y1 }, 3.3, 3.8, 'Read-only file system', { dy: -14, mono: true, dx: -10, align: 'right' });
        blocked(ctx, t, { x: ws.x + ws.w + 2, y: y2 }, { x: wall, y: y2 }, 4.9, 5.4, 'no network', { dy: -14, mono: true, dx: -10, align: 'right' });
      }
    }
  };

  /* ------------------------------------------------------- 3. memory */
  var memory = {
    dur: 6.6, still: 4.4,
    steps: [
      { at: 0, text: 'What it learns goes into a log on your computer that only it can add to.' },
      { at: 1.4, text: 'When it saves what it learned, the log’s new head goes to the ledger, which lets the log only grow.' },
      { at: 3.2, text: 'Change something it wrote before, and the ledger refuses the new head.' },
      { at: 4.9, text: 'So a reader can check what it knew, and when, against the head on the ledger.' }
    ],
    draw: function (ctx, t, N) {
      var log = N ? { x: 12, y: 14, w: 336, h: 104 } : { x: 40, y: 50, w: 520, h: 130 };
      var led = N ? { x: 12, y: 168, w: 336, h: 104 } : { x: 650, y: 50, w: 270, h: 130 };
      var rdr = N ? { x: 12, y: 340, w: 336, h: 96 } : { x: 40, y: 290, w: 520, h: 110 };
      box(ctx, log.x, log.y, log.w, log.h, 'its memory log, on your computer', { size: N ? 12 : 13 });
      var cell = N ? 22 : 28, gap = N ? 8 : 10, cx = log.x + 14, cy = log.y + (N ? 44 : 52);
      var added = U.span(t, 1.4, 1.7);
      var changed = U.win(t, 3.2, 4.9, 0.15);
      for (var i = 0; i < 4; i++) {
        var a = i < 3 ? 1 : added;
        if (a <= 0) continue;
        var x = cx + i * (cell + gap);
        if (i === 1 && changed > 0) {
          U.rect(ctx, x, cy, cell, cell, { stroke: C.signal, lw: 1.5, alpha: changed });
          U.rect(ctx, x, cy, cell, cell, { fill: i === 3 ? C.ink : C.ink3, alpha: (1 - changed) });
        } else {
          U.rect(ctx, x, cy, cell, cell, { fill: i === 3 ? C.ink : C.ink3, alpha: a });
        }
      }
      if (changed > 0) note(ctx, 'an entry changed', cx + 1 * (cell + gap), cy + cell + 18, { alpha: changed, color: C.ink2 });

      box(ctx, led.x, led.y, led.w, led.h, 'the ledger', { size: N ? 12 : 13 });
      var entries = t >= 2.3 ? 4 : 3;
      note(ctx, 'its log’s head: ' + entries + ' entries', led.x + 12, led.y + (N ? 50 : 56), { color: C.ink, size: 12 });
      var ok = U.win(t, 2.3, 3.1, 0.15);
      if (ok > 0) U.check(ctx, led.x + led.w - 22, led.y + (N ? 46 : 52), 10, { alpha: ok });
      var refused = U.win(t, 4.1, 99, 0.15);
      if (refused > 0) {
        U.cross(ctx, led.x + 18, led.y + (N ? 76 : 86), 9, { alpha: refused });
        note(ctx, N ? 'refused: the new head does' : 'refused: the new head does not extend the log', led.x + 30, led.y + (N ? 80 : 90), { color: C.signal, alpha: refused });
        if (N) note(ctx, 'not extend the log', led.x + 30, led.y + 94, { color: C.signal, alpha: refused });
      }
      var lPort = N ? { x: log.x + log.w / 2, y: log.y + log.h + 2 } : { x: log.x + log.w + 2, y: log.y + log.h / 2 };
      var gPort = N ? { x: led.x + led.w / 2, y: led.y - 2 } : { x: led.x - 2, y: led.y + led.h / 2 };
      travel(ctx, t, lPort, gPort, 1.7, 2.3, { label: 'new head', dx: N ? 40 : 0, dy: N ? 4 : -8 });
      travel(ctx, t, lPort, gPort, 3.5, 4.1, { label: 'a rewritten head', dx: N ? 52 : 0, dy: N ? 4 : -8 });

      var ra = U.span(t, 4.9, 5.2);
      if (ra > 0) {
        box(ctx, rdr.x, rdr.y, rdr.w, rdr.h, 'a reader asks what it recorded', { stroke: C.rule2, alpha: ra, size: N ? 12 : 13 });
        note(ctx, '4 entries, with a proof checked against the head', rdr.x + 12, rdr.y + (N ? 52 : 58), { color: C.ink, size: 12, alpha: ra });
        U.check(ctx, rdr.x + rdr.w - 22, rdr.y + (N ? 48 : 54), 10, { alpha: ra });
      }
    }
  };

  /* ----------------------------------------------------- 4. schedule */
  var DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  var schedule = {
    dur: 5.8, still: 4.6,
    steps: [
      { at: 0, text: 'Give it a job and a time: every weekday at 09:00 UTC, check the budget.' },
      { at: 1.6, text: 'Each run is a session of its own, under the same limits; its answer goes where you said.' },
      { at: 3.6, text: 'Runs happen while the scheduler runs on your computer; one it missed happens once, late.' }
    ],
    draw: function (ctx, t, N) {
      var head = N ? { x: 12, y: 30 } : { x: 40, y: 50 };
      U.text(ctx, 'Every weekday at 09:00 UTC: How much budget is left?', head.x, head.y, { size: N ? 12 : 15, weight: 600, max: N ? 336 : 880 });
      note(ctx, "--cron '0 9 * * mon-fri'", head.x, head.y + (N ? 20 : 24), { mono: true, color: C.ink3 });
      var x0 = N ? 12 : 40, y0 = N ? 92 : 120, dw = N ? 48 : 126, dh = N ? 120 : 150, cw = N ? 44 : 118;
      var sweep = U.span(t, 0.6, 3.6, U.linear) * 7;
      DAYS.forEach(function (d, i) {
        var x = x0 + i * dw;
        var weekday = i < 5;
        U.rect(ctx, x, y0, cw, dh, { stroke: weekday ? C.ink : C.rule2, lw: 1 });
        U.text(ctx, d, x + 8, y0 + 18, { size: N ? 11 : 13, weight: 600, color: weekday ? C.ink : C.ink3 });
        if (weekday) {
          note(ctx, '09:00', x + 8, y0 + (N ? 36 : 40), { color: C.ink3, size: N ? 10 : 11 });
          var ran = U.span(t, 0.6 + (i + 0.4) * 3 / 7, 0.6 + (i + 0.4) * 3 / 7 + 0.2);
          if (ran > 0) {
            U.rect(ctx, x + 8, y0 + (N ? 50 : 56), N ? 12 : 16, N ? 12 : 16, { fill: C.ink, alpha: ran });
            if (!N) note(ctx, 'a run', x + 30, y0 + 69, { alpha: ran, color: C.ink2 });
            note(ctx, N ? '85' : '85 USD left', x + 8, y0 + (N ? 84 : 100), { alpha: ran, color: C.ink, size: N ? 10 : 12 });
            if (N) note(ctx, 'USD', x + 8, y0 + 98, { alpha: ran, color: C.ink, size: 10 });
          }
        }
      });
      /* the playhead */
      if (sweep > 0 && sweep < 7) {
        var px = x0 + sweep * dw;
        U.line(ctx, px, y0 - 8, px, y0 + dh + 8, { color: C.ink, lw: 1.25 });
      }
      var dest = N ? { x: 12, y: 270 } : { x: 40, y: 330 };
      var da = U.span(t, 1.6, 1.9);
      if (da > 0) {
        note(ctx, 'each answer goes to your chat app, a file or a webhook', dest.x, dest.y + 16, { alpha: da, color: C.ink2, size: N ? 11 : 12 });
      }
      var late = U.span(t, 3.6, 3.9);
      if (late > 0) {
        U.para(ctx, 'a run missed while your computer slept happens once, when the scheduler runs again', dest.x, dest.y + (N ? 40 : 44),
          N ? 336 : 880, { alpha: late, color: C.ink3, size: N ? 11 : 12, lh: N ? 15 : 16 });
      }
    }
  };

  /* ------------------------------------------------------ 5. helpers */
  var helpers = {
    dur: 7.4, still: 3.9,
    steps: [
      { at: 0, text: 'It can hand work to helpers. Each gets an identity of its own and a slice of its budget.' },
      { at: 1.5, text: '12 researchers with 40 USD each, at most 10 USD a payment; one hires 3 more, with 10 USD each.' },
      { at: 3.0, text: 'A helper two levels down tries to pay 6 USD, over its 5 USD limit: the ledger refuses.' },
      { at: 4.6, text: 'Suspend your agent, and every helper below it is refused too, in one step.' },
      { at: 6.2, text: 'Lift the suspension, and they pay again.' }
    ],
    draw: function (ctx, t, N) {
      var top = N ? { x: 180, y: 40 } : { x: 480, y: 56 };
      var l1y = N ? 150 : 170, l2y = N ? 250 : 286;
      var l1 = N ? [60, 140, 220] : [200, 360, 520, 680];
      var l2 = N ? [30, 70, 110] : [120, 200, 280];
      var ledgerY = N ? 360 : 400;
      var suspended = U.win(t, 4.6, 6.2, 0.2);
      var dim = 1 - suspended * 0.6;

      var r0 = N ? 18 : 22, r1 = N ? 12 : 16, r2 = N ? 9 : 12;
      var s1 = U.span(t, 0.6, 1.2), s2 = U.span(t, 1.6, 2.2);
      /* lines of the tree */
      l1.forEach(function (x) {
        U.line(ctx, top.x, top.y + r0, U.lerp(top.x, x, s1), U.lerp(top.y + r0, l1y - r1, s1), { color: C.rule2, alpha: s1 * dim });
      });
      l2.forEach(function (x) {
        U.line(ctx, l1[0], l1y + r1, U.lerp(l1[0], x, s2), U.lerp(l1y + r1, l2y - r2, s2), { color: C.rule2, alpha: s2 * dim });
      });

      U.diamond(ctx, top.x, top.y, r0, { lw: 1.6, alpha: dim });
      U.text(ctx, 'your agent', top.x + r0 + 10, top.y - 2, { size: 12, weight: 600, alpha: dim });
      note(ctx, '1,000 USD', top.x + r0 + 10, top.y + 14, { alpha: dim });
      if (suspended > 0) note(ctx, 'suspended', top.x - r0 - 10, top.y + 4, { align: 'right', color: C.ink, alpha: suspended, weight: 600, size: 12 });

      l1.forEach(function (x, i) {
        if (s1 <= 0) return;
        U.diamond(ctx, x, l1y, r1, { lw: 1.4, alpha: s1 * dim });
      });
      if (s1 > 0) {
        if (N) {
          note(ctx, 'and 9 more,', 248, l1y + 4, { alpha: s1 * dim, color: C.ink3 });
          note(ctx, '40 USD each,', 248, l1y + 20, { alpha: s1 * dim });
          note(ctx, '≤ 10 a payment', 248, l1y + 34, { alpha: s1 * dim });
        } else {
          note(ctx, 'and 8 more', l1[3] + r1 + 12, l1y + 4, { alpha: s1 * dim, color: C.ink3 });
          note(ctx, '40 USD each, at most 10 USD a payment', l1[3] + r1 + 12, l1y + 20, { alpha: s1 * dim });
        }
      }
      l2.forEach(function (x) {
        if (s2 <= 0) return;
        U.diamond(ctx, x, l2y, r2, { lw: 1.3, alpha: s2 * dim });
      });
      if (s2 > 0) note(ctx, N ? '10 USD each, ≤ 5 a payment' : '10 USD each, at most 5 USD a payment', N ? 12 : l2[0] - 40, l2y + r2 + 20, { alpha: s2 * dim });

      /* the ledger, and three payments from the last helper */
      U.line(ctx, N ? 12 : 40, ledgerY, N ? 348 : 920, ledgerY, { color: C.ink, lw: 1.25 });
      note(ctx, 'the ledger', N ? 12 : 40, ledgerY + 18, { color: C.ink, weight: 600, size: 12 });
      var from = { x: l2[2], y: l2y + r2 + 4 };
      var p1 = { x: N ? 60 : 420, y: ledgerY - 2 };
      travel(ctx, t, from, p1, 3.0, 3.6, { linger: 0.1 });
      var v1 = U.win(t, 3.6, 4.5, 0.15);
      if (v1 > 0) {
        U.cross(ctx, p1.x, p1.y - 14, 9, { alpha: v1 });
        note(ctx, '6 USD: above its 5 USD limit', p1.x + 12, p1.y - 10, { color: C.signal, alpha: v1 });
      }
      travel(ctx, t, from, p1, 5.0, 5.6, { linger: 0.1 });
      var v2 = U.win(t, 5.6, 6.2, 0.15);
      if (v2 > 0) {
        U.cross(ctx, p1.x, p1.y - 14, 9, { alpha: v2 });
        note(ctx, '3 USD: refused, its agent is suspended', p1.x + 12, p1.y - 10, { color: C.signal, alpha: v2 });
      }
      travel(ctx, t, from, p1, 6.4, 6.9, { linger: 0.1 });
      var v3 = U.win(t, 6.9, 99, 0.15);
      if (v3 > 0) {
        U.check(ctx, p1.x, p1.y - 14, 10, { alpha: v3 });
        note(ctx, '3 USD: accepted', p1.x + 12, p1.y - 10, { color: C.ink, alpha: v3 });
      }
    }
  };

  /* ------------------------------------------------------- 6. skills */
  var skills = {
    dur: 6.8, still: 4.9,
    steps: [
      { at: 0, text: 'A skill is know-how it can take from others, and someone has to answer for it.' },
      { at: 1.0, text: 'This one has its publisher’s 150 USD bond behind it. Your agent could lose 100 USD by it, so it needs 200.' },
      { at: 2.3, text: 'Not enough: it is not installed, and refusing costs nothing.' },
      { at: 3.5, text: 'An auditor reads every file and adds a 100 USD bond. 250 covers 200: installed.' },
      { at: 5.2, text: 'If the skill turns out to be malicious, the bonds pay the agents that relied on it.' }
    ],
    draw: function (ctx, t, N) {
      var sk = N ? { x: 12, y: 14, w: 336, h: 70 } : { x: 40, y: 40, w: 300, h: 110 };
      box(ctx, sk.x, sk.y, sk.w, sk.h, 'the skill pay-invoices', { size: N ? 12 : 14 });
      note(ctx, 'a skill: instructions and files', sk.x + 12, sk.y + (N ? 42 : 48), { color: C.ink3 });
      if (!N) note(ctx, 'it declares the calls it may make', sk.x + 12, sk.y + 68, { color: C.ink3 });

      /* backing against what is needed: a scale from 0 to 300 USD */
      var bx = N ? 12 : 420, bw = N ? 336 : 500, by = N ? 150 : 90, bh = N ? 28 : 34;
      var scale = function (usd) { return bx + bw * usd / 300; };
      U.line(ctx, bx, by + bh + 10, bx + bw, by + bh + 10, { color: C.rule2 });
      [0, 100, 200, 300].forEach(function (v) {
        U.line(ctx, scale(v), by + bh + 6, scale(v), by + bh + 14, { color: C.rule2 });
        note(ctx, v + (v === 300 ? ' USD' : ''), scale(v), by + bh + 28, { align: v === 0 ? 'left' : (v === 300 ? 'right' : 'center'), color: C.ink3 });
      });
      var need = scale(200);
      U.line(ctx, need, by - 18, need, by + bh + 4, { color: C.ink, lw: 1.5 });
      note(ctx, N ? 'needed: 200' : 'needed: 2 × the 100 USD at risk = 200', N ? need + 6 : need - 6, by - 8,
        { color: C.ink, size: 12, align: N ? 'left' : 'right' });

      var b1 = U.span(t, 0.9, 1.6);
      if (b1 > 0) {
        U.rect(ctx, bx, by, scale(150 * b1) - bx, bh, { fill: C.ink });
        if (b1 >= 1) note(ctx, 'publisher’s bond 150', bx + 8, by + bh / 2 + 4, { color: C.bg, size: N ? 11 : 12, weight: 600 });
      }
      var b2 = U.span(t, 3.5, 4.2);
      if (b2 > 0) {
        U.rect(ctx, scale(150) + 2, by, scale(150 + 100 * b2) - scale(150) - 2, bh, { fill: C.ink2 });
        if (b2 >= 1) note(ctx, 'audit 100', scale(150) + 8, by + bh / 2 + 4, { color: C.bg, size: N ? 11 : 12, weight: 600 });
      }
      var vy = N ? 262 : 210;
      var no = U.win(t, 2.3, 3.5, 0.15);
      if (no > 0) {
        U.cross(ctx, bx + 6, vy - 4, 9, { alpha: no });
        note(ctx, 'not installed: 150 is below 200', bx + 20, vy, { color: C.signal, size: 12, alpha: no });
      }
      var yes = U.win(t, 4.3, 99, 0.15);
      if (yes > 0) {
        U.check(ctx, bx + 6, vy - 4, 10, { alpha: yes });
        note(ctx, 'installed: 250 covers 200', bx + 20, vy, { color: C.ink, size: 12, alpha: yes });
      }
      var bonds = U.span(t, 5.2, 5.5);
      if (bonds > 0) {
        var ny = N ? 330 : 300;
        note(ctx, 'if it misleads an agent, a challenge can slash both bonds', N ? 12 : 40, ny,
          { color: C.ink2, size: N ? 11 : 12, alpha: bonds, max: N ? 336 : 880 });
        note(ctx, 'to repay the agents that relied on it', N ? 12 : 40, ny + 18, { color: C.ink2, size: N ? 11 : 12, alpha: bonds });
      }
    }
  };

  /* --------------------------------------------------------- the reel */
  var CHAPTERS = [apps, sandbox, memory, schedule, helpers, skills];
  var at = 0;
  var chapters = CHAPTERS.map(function (c) {
    var start = at;
    at += c.dur;
    return { at: start, still: c.still };
  });
  var D = at;

  S.scene('capabilities', {
    duration: D,
    still: chapters[0].at + chapters[0].still,
    chapters: chapters,
    wide: { w: 960, h: 540 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var N = view.narrow;
      var i = 0;
      while (i + 1 < chapters.length && t >= chapters[i + 1].at) i++;
      var c = CHAPTERS[i], lt = t - chapters[i].at;
      ctx.save();
      ctx.globalAlpha = Math.min(U.span(lt, 0, 0.3), 1 - U.span(lt, c.dur - 0.35, c.dur));
      c.draw(ctx, lt, N);
      /* what is happening, in words */
      var ry = N ? 506 : 470, ty = N ? 528 : 500;
      U.line(ctx, N ? 12 : 40, ry, N ? 348 : 920, ry, { color: C.rule });
      var step = c.steps[0];
      c.steps.forEach(function (s) { if (lt >= s.at) step = s; });
      var sa = U.span(lt, step.at, step.at + 0.3);
      U.para(ctx, step.text, N ? 12 : 40, ty, N ? 336 : 880, { size: N ? 13 : 15, alpha: sa, lh: N ? 17 : 20 });
      ctx.restore();
    }
  });
})();
