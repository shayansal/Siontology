/* What your agent can do: six short runs, one after another, each of one
 * mechanism, with the facts of a recording in the book.
 *
 *  1. Connect: one agent on many messengers; a voice note answered aloud;
 *     a stranger given pairing code QBRS-9XRF and let in (agent/voice.md).
 *  2. Remember: a memory log only it appends to; a skill it wrote down
 *     after a long task, pay-bobs-invoices (agent/skill-improvement.md); a
 *     search of its past sessions; the log's head on the ledger, a rewrite
 *     refused (the robots demo).
 *  3. Schedule: every weekday at 09:00 UTC, "How much budget is left?",
 *     answered "85 USD left in the budget." (agent/schedules.md).
 *  4. Delegate: 12 helpers with 40 USD each, at most 10 a payment; 3 more
 *     with 10 each, at most 5; 6 USD refused; a suspension refuses the whole
 *     tree until it is lifted (the swarm demo).
 *  5. Browse: "What is the serde_json crate for? Look on docs.rs." in a
 *     browser of its own: the page read as its accessibility tree, typed
 *     into, clicked, pictured; the answer as recorded (agent/browser.md).
 *  6. Sandbox: primes.py written and run (17 primes below 60); /usr/local
 *     read-only; no network (agent/tools.md).
 *
 * The chapter buttons and captions beside the canvas follow def.chapters.
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;

  /* --------------------------------------------------------- helpers */
  function box(ctx, x, y, w, h, title, o) {
    o = o || {};
    U.rect(ctx, x, y, w, h, { stroke: o.stroke || C.ink, lw: o.lw || 1.25, alpha: o.alpha, fill: o.fill });
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
        label.split('\n').forEach(function (s, k) {
          U.text(ctx, s, wall.x + (o.dx || 0), wall.y + (o.dy === undefined ? 22 : o.dy) + k * (o.lh || 13),
            { size: o.size || 11, color: C.signal, align: o.align || 'center', alpha: vis, mono: o.mono });
        });
      }
    }
  }

  /* The agent, with a face that blinks, talks while it answers and thinks while it works. */
  function agent(ctx, t, A, o) {
    o = o || {};
    U.face(ctx, A.x, A.y, A.r, {
      lw: 1.6, blink: U.blink(t, 3.4, o.off || 0.5), look: o.look || 0, eyes: o.eyes, mouth: o.mouth || 'smile',
      talk: o.talk > 0 ? (Math.sin(t * 18) * 0.5 + 0.5) * o.talk : 0, think: o.think || 0, thinkT: t, alpha: o.alpha
    });
  }

  /* ------------------------------------------------------- 1. connect */
  var APPS = ['Terminal', 'Telegram', 'WhatsApp', 'Slack', 'Email'];
  var connect = {
    dur: 6.6, still: 5.6,
    steps: [
      { at: 0, text: 'One agent answers you on the apps you already use: 28 messaging services, and your terminal.' },
      { at: 1.3, text: 'A voice note on Telegram: “How much budget is left?” It answers aloud.' },
      { at: 3.2, text: 'Someone it does not know writes “Hello?”. They get a pairing code, and no answer.' },
      { at: 4.9, text: 'You let them in with that code; it answers them from their next message.' }
    ],
    draw: function (ctx, t, N) {
      var boxes = APPS.map(function (name, i) {
        return N ? { x: i % 2 ? 186 : 12, y: 14 + Math.floor(i / 2) * 46, w: 162, h: 36, name: name }
                 : { x: 40, y: 40 + i * 64, w: 210, h: 44, name: name };
      });
      var A = N ? { x: 180, y: 236, r: 26 } : { x: 470, y: 200, r: 34 };
      var stranger = N ? { x: 12, y: 316, w: 336, h: 48 } : { x: 650, y: 150, w: 270, h: 56 };
      var hot = function (i) {
        var intro = U.win(t, 0.15 + i * 0.18, 0.9 + i * 0.18, 0.2) * 0.8;
        return i === 1 ? Math.max(intro, U.win(t, 1.3, 3.0, 0.2)) : intro;
      };
      boxes.forEach(function (b, i) {
        box(ctx, b.x, b.y, b.w, b.h, b.name, { on: hot(i), size: N ? 12 : 14, ty: N ? 23 : 27 });
      });
      note(ctx, 'and 24 more messaging services', N ? 12 : 40, N ? 162 : 370, { color: C.ink3, size: N ? 11 : 12 });

      agent(ctx, t, A, { talk: Math.max(U.win(t, 2.2, 2.8, 0.1), U.win(t, 4.0, 4.5, 0.1)), look: -U.win(t, 1.3, 2.1, 0.2) * 0.6 });
      U.text(ctx, 'your agent', A.x, A.y + A.r + 18, { size: N ? 12 : 13, weight: 600, align: 'center' });

      /* the voice note, and the answer spoken */
      var tg = boxes[1];
      var tgPort = N ? { x: tg.x - 2, y: tg.y + tg.h / 2 } : { x: tg.x + tg.w + 2, y: tg.y + tg.h / 2 };
      var aPort = N ? { x: A.x + 4, y: A.y - A.r - 4 } : { x: A.x - A.r - 4, y: A.y };
      travel(ctx, t, tgPort, aPort, 1.4, 2.0, { label: N ? '' : 'voice note', dy: -10, linger: 0.15 });
      travel(ctx, t, aPort, tgPort, 2.2, 2.8, { label: N ? '' : 'its answer, aloud', dy: 16 });

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

  /* ------------------------------------------------------ 2. remember */
  var remember = {
    dur: 7.0, still: 6.2,
    steps: [
      { at: 0, text: 'It keeps what it learns about your work in a memory on your computer that only it can add to.' },
      { at: 1.4, text: 'After a long task, it is asked whether to write down how it did it, as a skill to use again.' },
      { at: 3.0, text: 'Next time, it searches what it did before, each find checked against its memory.' },
      { at: 4.7, text: 'The ledger keeps the head of its memory, so a rewritten past is refused.' }
    ],
    draw: function (ctx, t, N) {
      var log = N ? { x: 12, y: 12, w: 336, h: 84 } : { x: 40, y: 34, w: 520, h: 104 };
      var skill = N ? { x: 12, y: 112, w: 336, h: 100 } : { x: 40, y: 164, w: 520, h: 110 };
      var find = N ? { x: 12, y: 228, w: 336, h: 84 } : { x: 40, y: 300, w: 520, h: 104 };
      var led = N ? { x: 12, y: 328, w: 236, h: 112 } : { x: 650, y: 34, w: 270, h: 140 };
      var A = N ? { x: 302, y: 374, r: 24 } : { x: 760, y: 300, r: 34 };

      box(ctx, log.x, log.y, log.w, log.h, 'its memory, on your computer', { size: N ? 12 : 13 });
      var cell = N ? 20 : 26, gap = N ? 7 : 9, cx = log.x + 14, cy = log.y + (N ? 40 : 48);
      var added = U.span(t, 0.7, 1.0);
      for (var i = 0; i < 5; i++) {
        var a = i < 4 ? 1 : added;
        if (a > 0) U.rect(ctx, cx + i * (cell + gap), cy, cell, cell, { fill: i === 4 ? C.ink : C.ink3, alpha: a });
      }

      var sk = U.span(t, 1.4, 1.7);
      if (sk > 0) {
        box(ctx, skill.x, skill.y, skill.w, skill.h, 'a skill it wrote: pay-bobs-invoices', { alpha: sk, size: N ? 12 : 13 });
        var lines = N ? ['1. Check the budget.', '2. Pay each invoice, its number', '    as the purpose.', '3. Check the budget again.']
                      : ['1. Check the budget.', '2. Pay each invoice, its number as the purpose.', '3. Check the budget again.'];
        lines.forEach(function (s, k) {
          note(ctx, s, skill.x + 12, skill.y + (N ? 42 : 46) + k * (N ? 14 : 18), { color: C.ink2, size: N ? 11 : 12, alpha: sk });
        });
      }

      var fa = U.span(t, 3.0, 3.3);
      if (fa > 0) {
        box(ctx, find.x, find.y, find.w, find.h, 'it searches its past sessions', { stroke: C.rule2, alpha: fa, size: N ? 12 : 13 });
        note(ctx, 'bob’s invoices', find.x + 12, find.y + (N ? 44 : 50), { mono: true, color: C.ink, size: 12, alpha: fa });
        var hit = U.span(t, 3.7, 4.0);
        if (hit > 0) {
          note(ctx, N ? 'a past session, checked' : 'a past session, checked against its memory', find.x + 12, find.y + (N ? 66 : 76), { color: C.ink, size: 12, alpha: hit });
          U.check(ctx, find.x + find.w - 22, find.y + (N ? 62 : 72), 10, { alpha: hit });
        }
      }

      box(ctx, led.x, led.y, led.w, led.h, 'the ledger', { size: N ? 12 : 13 });
      var entries = t >= 1.2 ? 5 : 4;
      note(ctx, 'its memory’s head: ' + entries + ' entries', led.x + 12, led.y + (N ? 46 : 54), { color: C.ink, size: 12 });
      var ok = U.win(t, 1.2, 2.6, 0.15);
      if (ok > 0) U.check(ctx, led.x + led.w - 22, led.y + (N ? 42 : 50), 10, { alpha: ok });
      var lPort = N ? { x: log.x + log.w - 40, y: log.y + log.h + 2 } : { x: log.x + log.w + 2, y: log.y + log.h / 2 };
      var gPort = N ? { x: led.x + led.w - 40, y: led.y - 2 } : { x: led.x - 2, y: led.y + 40 };
      if (!N) travel(ctx, t, lPort, gPort, 0.8, 1.2, { label: 'new head', dy: -8, linger: 0.3 });
      var rw = U.win(t, 5.0, 99, 0.15);
      if (rw > 0) {
        U.cross(ctx, led.x + 18, led.y + (N ? 72 : 90), 9, { alpha: rw });
        note(ctx, 'a rewritten past: refused', led.x + 30, led.y + (N ? 76 : 94), { color: C.signal, alpha: rw });
        note(ctx, N ? 'it does not extend the log' : 'the new head does not extend the log', led.x + 30, led.y + (N ? 92 : 112), { color: C.ink3, alpha: rw });
      }

      agent(ctx, t, A, { think: U.win(t, 3.0, 3.8, 0.15), look: -0.5 });
      U.text(ctx, 'your agent', A.x, A.y + A.r + 18, { size: N ? 12 : 13, weight: 600, align: 'center' });
    }
  };

  /* ------------------------------------------------------ 3. schedule */
  var DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  var schedule = {
    dur: 5.8, still: 4.6,
    steps: [
      { at: 0, text: 'A job and a time, set with one command: every weekday at 09:00 UTC, check the budget.' },
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
          }
        } else if (!N) {
          agent(ctx, t, { x: x + cw / 2, y: y0 + 92, r: 16 }, { eyes: 'happy', mouth: 'flat', alpha: 0.6 });
        }
      });
      if (!N) note(ctx, 'weekends off', x0 + 5 * dw + 8, y0 + dh - 12, { color: C.ink3 });
      var sweepOn = sweep > 0 && sweep < 7;
      if (sweepOn) {
        var px = x0 + sweep * dw;
        U.line(ctx, px, y0 - 8, px, y0 + dh + 8, { color: C.ink, lw: 1.25 });
      }
      var dest = N ? { x: 12, y: 236 } : { x: 40, y: 310 };
      var da = U.span(t, 1.6, 1.9);
      if (da > 0) {
        U.para(ctx, 'each answer goes to your chat app, a file or a webhook', dest.x, dest.y + 16,
          N ? 336 : 880, { alpha: da, color: C.ink2, size: N ? 11 : 12, lh: N ? 15 : 16 });
      }
      var rec = U.span(t, 2.4, 2.7);
      if (rec > 0) {
        U.para(ctx, 'in the recording, a job run every minute answered: “85 USD left in the budget.”', dest.x, dest.y + (N ? 40 : 44),
          N ? 336 : 880, { alpha: rec, color: C.ink, size: N ? 11 : 12, lh: N ? 15 : 16 });
      }
      var late = U.span(t, 3.6, 3.9);
      if (late > 0) {
        U.para(ctx, 'a run missed while the scheduler was off happens once, when it runs again', dest.x, dest.y + (N ? 80 : 72),
          N ? 336 : 880, { alpha: late, color: C.ink3, size: N ? 11 : 12, lh: N ? 15 : 16 });
      }
    }
  };

  /* ------------------------------------------------------ 4. delegate */
  var delegate = {
    dur: 7.4, still: 3.9,
    steps: [
      { at: 0, text: 'It can hand work to helpers. Each gets a conversation of its own and a slice of its budget.' },
      { at: 1.5, text: '12 helpers at once, with 40 USD each; one hands part of its work to 3 more, with 10 USD each.' },
      { at: 3.0, text: 'A helper two levels down tries to pay 6 USD, over its 5 USD limit: the ledger refuses.' },
      { at: 4.6, text: 'Suspend your agent, and every helper below it is refused too, in one step.' },
      { at: 6.2, text: 'Lift the suspension, and they pay again.' }
    ],
    draw: function (ctx, t, N) {
      var top = N ? { x: 180, y: 44 } : { x: 480, y: 60 };
      var l1y = N ? 150 : 176, l2y = N ? 250 : 292;
      var l1 = N ? [60, 140, 220] : [200, 360, 520, 680];
      var l2 = N ? [30, 70, 110] : [120, 200, 280];
      var ledgerY = N ? 360 : 404;
      var suspended = U.win(t, 4.6, 6.2, 0.2);
      var dim = 1 - suspended * 0.6;

      var r0 = N ? 22 : 28, r1 = N ? 14 : 19, r2 = N ? 10 : 14;
      var s1 = U.span(t, 0.6, 1.2), s2 = U.span(t, 1.6, 2.2);
      l1.forEach(function (x) {
        U.line(ctx, top.x, top.y + r0, U.lerp(top.x, x, s1), U.lerp(top.y + r0, l1y - r1, s1), { color: C.rule2, alpha: s1 * dim });
      });
      l2.forEach(function (x) {
        U.line(ctx, l1[0], l1y + r1, U.lerp(l1[0], x, s2), U.lerp(l1y + r1, l2y - r2, s2), { color: C.rule2, alpha: s2 * dim });
      });

      agent(ctx, t, { x: top.x, y: top.y, r: r0 }, { alpha: dim, eyes: suspended > 0.5 ? 'closed' : 'square', mouth: suspended > 0.5 ? 'flat' : 'smile' });
      U.text(ctx, 'your agent', top.x + r0 + 10, top.y - 2, { size: 12, weight: 600, alpha: dim });
      note(ctx, '1,000 USD', top.x + r0 + 10, top.y + 14, { alpha: dim });
      if (suspended > 0) note(ctx, 'suspended', top.x - r0 - 10, top.y + 4, { align: 'right', color: C.ink, alpha: suspended, weight: 600, size: 12 });

      l1.forEach(function (x, i) {
        if (s1 > 0) agent(ctx, t, { x: x, y: l1y, r: r1 }, { alpha: s1 * dim, off: i * 0.9, think: t > 2.2 && t < 4.4 ? 1 : 0, mouth: 'flat' });
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
      l2.forEach(function (x, i) {
        if (s2 > 0) agent(ctx, t, { x: x, y: l2y, r: r2 }, { alpha: s2 * dim, off: 1.7 + i, mouth: 'flat' });
      });
      if (s2 > 0) {
        if (N) note(ctx, '10 USD each, ≤ 5 a payment', l2[2] + r2 + 10, l2y + 4, { alpha: s2 * dim });
        else note(ctx, '10 USD each, at most 5 USD a payment', l2[0] - 40, l2y + r2 + 20, { alpha: s2 * dim });
      }

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

  /* -------------------------------------------------------- 5. browse */
  var TREE = ['RootWebArea “Docs.rs” focused', '  navigation “Main navigation” [ref=e1]', '    form [ref=e2]', '      link “Docs.rs” [ref=e3]', '      …'];
  var browse = {
    dur: 7.2, still: 6.4,
    steps: [
      { at: 0, text: 'Ask it to look something up: “What is the serde_json crate for? Look on docs.rs.”' },
      { at: 1.2, text: 'It opens the site in a browser of its own, and reads the page as a list of what is on it.' },
      { at: 2.7, text: 'It types into the search box and opens the result, as you would.' },
      { at: 4.1, text: 'It takes a picture of the page, and looks at it.' },
      { at: 5.3, text: 'Then it answers, having stayed on the one site you allowed.' }
    ],
    draw: function (ctx, t, N) {
      var w = N ? { x: 12, y: 12, w: 336, h: 300 } : { x: 40, y: 30, w: 580, h: 396 };
      var A = N ? { x: 44, y: 372, r: 24 } : { x: 790, y: 150, r: 38 };
      box(ctx, w.x, w.y, w.w, w.h, null, { lw: 1.5 });
      U.line(ctx, w.x, w.y + 36, w.x + w.w, w.y + 36, { color: C.rule });
      var opened = U.span(t, 1.0, 1.3);
      var onResult = t >= 3.6;
      var url = onResult ? 'docs.rs/serde_json/latest/serde_json/' : 'docs.rs';
      U.rect(ctx, w.x + 10, w.y + 8, w.w - 20, 20, { stroke: C.rule2, lw: 1 });
      if (opened > 0) note(ctx, url, w.x + 18, w.y + 22, { mono: true, size: N ? 10.5 : 11.5, color: C.ink, alpha: opened });
      note(ctx, 'its browser: only docs.rs', w.x + w.w - 16, w.y + 22, { align: 'right', color: C.ink3, size: N ? 10 : 11, alpha: N ? 0 : 1 });

      var bx = w.x + 18, by = w.y + 64, lh = N ? 18 : 22, sz = N ? 11 : 12.5;
      if (!onResult && opened > 0) {
        TREE.forEach(function (s, k) {
          note(ctx, s, bx, by + k * lh, { mono: true, size: sz, color: C.ink2, alpha: U.span(t, 1.3 + k * 0.15, 1.5 + k * 0.15) });
        });
        var typed = 'serde_json'.slice(0, Math.max(0, Math.floor(U.span(t, 2.8, 3.3, U.linear) * 10)));
        var fy = by + 5 * lh;
        if (t > 2.7) {
          U.rect(ctx, bx, fy, N ? 200 : 300, 26, { stroke: C.ink, lw: 1.25 });
          note(ctx, typed, bx + 10, fy + 18, { mono: true, size: 12.5, color: C.ink });
          note(ctx, 'e11', bx + (N ? 208 : 308), fy + 18, { mono: true, size: 11, color: C.ink3 });
        }
        if (t > 3.3) {
          var res = { x: bx, y: fy + 40, w: N ? 300 : 420, h: 30 };
          U.rect(ctx, res.x, res.y, res.w, res.h, { stroke: C.rule2, lw: 1, fill: t > 3.45 ? C.raised : null });
          note(ctx, 'serde_json: a JSON serialization file format', res.x + 10, res.y + 19, { size: N ? 11 : 12.5, color: C.ink });
        }
      }
      if (onResult) {
        var page = U.span(t, 3.6, 3.9);
        U.text(ctx, 'serde_json - Rust', bx, by + 6, { size: N ? 16 : 22, weight: 600, alpha: page });
        for (var k = 0; k < 6; k++) {
          U.rect(ctx, bx, by + 30 + k * (N ? 18 : 24), (N ? 280 : 500) * (k % 3 === 2 ? 0.6 : 0.92), N ? 7 : 9, { fill: C.rule2, alpha: page * 0.8 });
        }
        var shot = U.win(t, 4.2, 4.9, 0.1);
        if (shot > 0) U.rect(ctx, w.x + 6, w.y + 42, w.w - 12, w.h - 48, { stroke: C.ink, lw: 2, alpha: shot });
        if (t > 4.3) note(ctx, 'a picture of the page', w.x + w.w - 16, w.y + w.h - 14, { align: 'right', color: C.ink2, alpha: U.span(t, 4.3, 4.6) });
      }

      var looking = U.win(t, 1.2, 4.9, 0.2);
      agent(ctx, t, A, {
        look: N ? looking * 0.6 : -looking * 0.7, think: U.win(t, 1.2, 1.9, 0.15),
        talk: U.win(t, 5.3, 6.2, 0.1), mouth: t > 5.2 ? 'smile' : 'flat'
      });
      var ans = U.span(t, 5.3, 5.6);
      if (N) {
        U.text(ctx, 'your agent', A.x + A.r + 12, A.y - A.r + 4, { size: 12, weight: 600 });
        if (ans > 0) U.para(ctx, '“serde_json is a JSON serialization file format, built on Serde.”', A.x + A.r + 12, A.y - A.r + 22, 336 - A.r * 2 - 30, { size: 12, alpha: ans, lh: 16 });
      } else {
        U.text(ctx, 'your agent', A.x, A.y + A.r + 20, { size: 13, weight: 600, align: 'center' });
        if (ans > 0) U.para(ctx, '“serde_json is a JSON serialization file format, built on Serde.”', 660, A.y + A.r + 54, 260, { size: 13, alpha: ans, lh: 18 });
      }
    }
  };

  /* ------------------------------------------------------- 6. sandbox */
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
      var A = N ? { x: 60, y: 60, r: 20 } : { x: 110, y: 100, r: 26 };
      var sys = N ? { x: 12, y: 362, w: 160, h: 86 } : { x: 650, y: 120, w: 270, h: 110 };
      var net = N ? { x: 188, y: 362, w: 160, h: 86 } : { x: 650, y: 280, w: 270, h: 110 };

      box(ctx, sb.x, sb.y, sb.w, sb.h, 'its sandbox', { lw: 1.5, size: N ? 12 : 13 });
      note(ctx, 'bubblewrap or Docker', sb.x + sb.w - 12, sb.y + 22, { align: 'right', color: C.ink3 });
      agent(ctx, t, A, { think: N ? 0 : U.win(t, 1.8, 2.4, 0.15), eyes: 'goggles', mouth: t > 2.4 ? 'smile' : 'flat' });
      note(ctx, 'your agent', A.x + A.r + 10, A.y + 4, { color: C.ink, size: 12, weight: 600 });
      box(ctx, ws.x, ws.y, ws.w, ws.h, 'its folder', { stroke: C.rule2, size: N ? 12 : 13, weight: 500 });
      if (N) note(ctx, 'the only place it may write', ws.x + ws.w - 12, ws.y + 22, { align: 'right', color: C.ink3 });
      else note(ctx, 'the only place it may write', ws.x + 12, ws.y + 42, { color: C.ink3 });

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

      box(ctx, sys.x, sys.y, sys.w, sys.h, 'your computer', { stroke: C.rule2, size: N ? 12 : 13 });
      note(ctx, '/usr/local', sys.x + 12, sys.y + (N ? 42 : 46), { mono: true, color: C.ink3 });
      box(ctx, net.x, net.y, net.w, net.h, 'the internet', { stroke: C.rule2, size: N ? 12 : 13 });
      note(ctx, 'example.com', net.x + 12, net.y + (N ? 42 : 46), { mono: true, color: C.ink3 });

      if (N) {
        blocked(ctx, t, { x: 92, y: 290 }, { x: 92, y: sb.y + sb.h }, 3.3, 3.8, 'Read-only file system', { dy: 18, size: 10.5, mono: true });
        blocked(ctx, t, { x: 268, y: 290 }, { x: 268, y: sb.y + sb.h }, 4.9, 5.4, 'Temporary failure\nin name resolution', { dy: 18, size: 10.5, mono: true });
      } else {
        var wall = sb.x + sb.w, y1 = sys.y + sys.h / 2, y2 = net.y + net.h / 2;
        blocked(ctx, t, { x: ws.x + ws.w + 2, y: y1 }, { x: wall, y: y1 }, 3.3, 3.8, 'Read-only file system', { dy: -14, mono: true, dx: -10, align: 'right' });
        blocked(ctx, t, { x: ws.x + ws.w + 2, y: y2 }, { x: wall, y: y2 }, 4.9, 5.4, 'Temporary failure\nin name resolution', { dy: -27, mono: true, dx: -10, align: 'right' });
      }
    }
  };

  /* --------------------------------------------------------- the reel */
  var CHAPTERS = [connect, remember, schedule, delegate, browse, sandbox];
  var at = 0;
  var chapters = CHAPTERS.map(function (c) {
    var start = at;
    at += c.dur;
    return { at: start, still: c.still };
  });
  var D = at;

  S.scene('features', {
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
