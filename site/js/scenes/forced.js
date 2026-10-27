/* Forced inclusion, as in `sion demo censorship`.
 *
 * A gateway silently drops a user's payment. The user posts it as a forced
 * intent straight to consensus; one honest proposer includes it, the shard's
 * validators certify it among themselves and it executes. Then two of the
 * shard's four validators censor too, so no certificate (3 of 4) can form.
 * The shard has 8 Pulses; after that, every certified Pulse header counts the
 * intent as overdue, a light client sees it, and the shard's committee is
 * replaced by validators outside it (specification v1.3).
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;
  var D = 17;
  var P0 = 112, STEP = 0.6;
  var POST2 = 120, WINDOW = 8;

  function headerAt(n) { return (n - P0) * STEP + 0.3; }
  function latestPulse(t) { return Math.min(POST2 + WINDOW + 3, P0 + Math.max(0, Math.floor((t - 0.3) / STEP))); }

  function layout(narrow) {
    if (!narrow) {
      return {
        user: { x: 40, y: 34, w: 170, h: 50 }, gate: { x: 250, y: 34, w: 170, h: 50 },
        band: { x: 460, y: 26, w: 460, h: 66 },
        vals: [0, 1, 2, 3].map(function (i) { return { x: 460 + i * 118, y: 128, w: 106, h: 58 }; }),
        heads: { x: 40, y: 244, w: 64, h: 62, gap: 8, per: 12, rows: 1 },
        phone: { x: 40, y: 356, w: 72, h: 104 }, phoneText: { x: 132, y: 380, w: 300 },
        rot: true, note: { x: 460, y: 220 },
        story: { x: 40, y: 512, w: 880 }
      };
    }
    return {
      user: { x: 16, y: 10, w: 160, h: 46 }, gate: { x: 184, y: 10, w: 160, h: 46 },
      band: { x: 16, y: 68, w: 328, h: 52 },
      vals: [0, 1, 2, 3].map(function (i) { return { x: 16 + i * 84, y: 134, w: 76, h: 52 }; }),
      heads: { x: 16, y: 222, w: 60, h: 52, gap: 7, per: 5, rows: 2 },
      phone: { x: 16, y: 352, w: 72, h: 92 }, phoneText: { x: 100, y: 372, w: 244 },
      rot: true, note: { x: 16, y: 206 },
      story: { x: 16, y: 488, w: 328 },
      narrow: true
    };
  }

  function censoring(i, t) { return i < 2 && t >= 4.8; }

  S.scene('forced', {
    duration: D,
    still: 11.9,
    wide: { w: 960, h: 540 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var L = layout(view.narrow);
      ctx.save();
      ctx.globalAlpha = 1 - U.span(t, D - 0.8, D);
      var fs = L.narrow ? 11 : 12;

      /* the user and the gateway */
      var Us = L.user, G = L.gate;
      U.rect(ctx, Us.x, Us.y, Us.w, Us.h, { stroke: C.ink, lw: 1.25 });
      U.text(ctx, 'a user', Us.x + 10, Us.y + 19, { size: 13, weight: 600 });
      U.text(ctx, t < 4.8 ? 'pays alice 5 USD' : 'pays alice 7 USD', Us.x + 10, Us.y + Us.h - 11, { size: fs, color: C.ink2 });
      U.rect(ctx, G.x, G.y, G.w, G.h, { stroke: C.ink, lw: 1.25 });
      U.text(ctx, 'the gateway', G.x + 10, G.y + 19, { size: 13, weight: 600 });
      var dropped = U.span(t, 0.9, 1.2);
      if (dropped > 0) U.text(ctx, 'drops it silently', G.x + 10, G.y + G.h - 11, { size: fs, color: C.signal, alpha: dropped });
      var p1 = U.span(t, 0.4, 0.9, U.linear);
      if (p1 > 0 && p1 < 1) U.packet(ctx, Us.x + Us.w, Us.y + Us.h / 2, G.x, G.y + G.h / 2, p1, { size: 7 });
      if (t >= 0.9 && t < 2.2) U.cross(ctx, G.x + G.w - 18, G.y + 18, 10, { alpha: 1 });

      /* consensus: every validator proposes every round */
      var B = L.band;
      U.rect(ctx, B.x, B.y, B.w, B.h, { stroke: C.rule2, lw: 1 });
      U.text(ctx, L.narrow ? 'consensus: every validator proposes' : 'consensus: every validator proposes every round', B.x + 10, B.y + 16, { size: fs, color: C.ink3 });
      var colW = 18, cols = Math.floor((B.w - 20) / colW);
      var tick = t / 0.3;
      for (var k = 0; k < cols; k++) {
        var round = Math.floor(tick) - k;
        if (round < 0) continue;
        var x = B.x + B.w - 14 - k * colW - (tick % 1) * colW;
        if (x < B.x + 6) continue;
        for (var v = 0; v < 4; v++) {
          var y = B.y + 24 + v * (L.narrow ? 6 : 9);
          U.rect(ctx, x, y, 6, L.narrow ? 4 : 6, { fill: C.ink3, alpha: 0.7 });
        }
      }
      /* forced intents posted straight to consensus */
      [2.4, 5.2].forEach(function (at) {
        var pp = U.span(t, at, at + 0.5, U.linear);
        var bx = B.x + B.w * 0.55, by = B.y + B.h / 2;
        if (pp > 0 && pp < 1) {
          var sx = Us.x + Us.w / 2, sy = Us.y + Us.h;
          U.packet(ctx, sx, sy, bx, by, pp, { size: 7, color: C.ink });
        }
        var inc = U.win(t, at + 0.5, at + 2.2, 0.2);
        if (inc > 0) {
          U.rect(ctx, bx - 6, by - 6, 12, 12, { stroke: C.ink, lw: 2, alpha: inc });
          U.text(ctx, L.narrow ? 'intent included' : 'forced intent, included by an honest proposer',
            L.narrow ? B.x + B.w - 8 : B.x + B.w, L.narrow ? B.y + 16 : B.y + B.h + 14,
            { size: 11, weight: 600, color: C.ink, align: 'right', alpha: inc });
        }
      });

      /* the shard committee */
      L.vals.forEach(function (b, i) {
        var cens = censoring(i, t);
        var ra = U.span(t, 12.4 + i * 0.1, 13.0 + i * 0.1);
        var old = 1 - ra;
        if (ra > 0) {
          U.rect(ctx, b.x, b.y, b.w, b.h, { stroke: C.ink, lw: 1.25, dash: [4, 3], alpha: ra });
          U.text(ctx, 'V' + (i + 5), b.x + 10, b.y + 19, { size: 13, weight: 600, alpha: ra });
          U.text(ctx, L.narrow ? 'new' : 'new member', b.x + 10, b.y + b.h - 11, { size: fs, color: C.ink2, alpha: ra });
        }
        if (old <= 0) return;
        U.rect(ctx, b.x, b.y, b.w, b.h, { stroke: cens ? C.signal : C.ink, lw: 1.25, alpha: old });
        U.text(ctx, 'V' + (i + 1), b.x + 10, b.y + 19, { size: 13, weight: 600, alpha: old });
        var s = '';
        if (t >= 3.4 && t < 4.8) s = 'certifies';
        if (cens) s = 'censoring';
        if (!cens && t >= 6.0) s = 'certifies';
        var col = cens ? C.signal : C.ink2;
        if (s) U.text(ctx, s, b.x + 10, b.y + b.h - 11, { size: fs, color: col, alpha: old });
        var ok = (t >= 3.4 + i * 0.12 && t < 4.8) || (!cens && t >= 6.0);
        if (ok) U.check(ctx, b.x + b.w - 16, b.y + 16, 10, { alpha: old });
      });
      var certLabel = '';
      if (t >= 3.9 && t < 4.8) certLabel = '4 of 4: certified, executes in Pulse 117';
      if (t >= 6.4) certLabel = '2 of 4: no certificate (it needs 3)';
      var noteA = U.span(t, 12.4, 13.0);
      if (certLabel && noteA < 1) {
        U.text(ctx, certLabel, L.note.x, L.note.y, { size: fs, weight: 600, color: t >= 6.4 ? C.signal : C.ink, alpha: 1 - noteA });
      }
      if (noteA > 0) {
        U.text(ctx, L.narrow ? 'Rotated: a committee from outside the shard.' : 'Rotated: the shard\u2019s committee is replaced by validators outside it.',
          L.note.x, L.note.y, { size: fs, weight: 600, color: C.ink, alpha: noteA });
      }

      /* certified Pulse headers count what the shard owes */
      var H = L.heads, last = latestPulse(t);
      var total = H.per * H.rows;
      var first = Math.max(P0, last - total + 1);
      for (var n = first; n <= last; n++) {
        var idx = n - first, row = Math.floor(idx / H.per), col = idx % H.per;
        var hx = H.x + col * (H.w + H.gap), hy = H.y + row * (H.h + H.gap);
        var a = U.span(t, headerAt(n), headerAt(n) + 0.25);
        var pending = (n >= POST2 && n <= POST2 + WINDOW) || n === 116 ? 1 : 0;
        var overdue = n > POST2 + WINDOW ? 1 : 0;
        U.rect(ctx, hx, hy, H.w, H.h, { stroke: overdue ? C.signal : C.ink, lw: overdue ? 1.75 : 1, alpha: a });
        U.text(ctx, 'Pulse ' + n, hx + 6, hy + 16, { size: L.narrow ? 10 : 11, weight: 600, alpha: a });
        U.text(ctx, 'pending ' + pending, hx + 6, hy + (L.narrow ? 32 : 36), { size: L.narrow ? 10 : 11, color: C.ink2, alpha: a });
        U.text(ctx, 'overdue ' + overdue, hx + 6, hy + (L.narrow ? 46 : 54), { size: L.narrow ? 10 : 11, weight: overdue ? 600 : 400, color: overdue ? C.signal : C.ink3, alpha: a });
      }
      if (!L.narrow && last >= POST2) {
        var bi = POST2 - first, bj = Math.min(POST2 + WINDOW, last) - first;
        if (bi >= 0) {
          var x0 = H.x + bi * (H.w + H.gap), x1 = H.x + bj * (H.w + H.gap) + H.w;
          var yb = H.y + H.h + 12;
          U.poly(ctx, [[x0, yb - 5], [x0, yb], [x1, yb], [x1, yb - 5]], { color: C.ink2 });
          U.text(ctx, '8-Pulse deadline', x0, yb + 16, { size: 11, color: C.ink2 });
        }
      }

      /* a light client reads the certified headers */
      var Ph = L.phone, T = L.phoneText;
      U.phone(ctx, Ph.x, Ph.y, Ph.w, Ph.h, { stroke: C.ink, lw: 1.25 });
      U.text(ctx, 'light client', Ph.x + Ph.w / 2, Ph.y + 26, { size: 10, color: C.ink3, align: 'center' });
      U.text(ctx, 'reads headers', Ph.x + Ph.w / 2, Ph.y + 74, { size: 9, color: C.ink3, align: 'center' });
      var seen = t >= headerAt(POST2 + WINDOW + 1) + 0.5;
      U.text(ctx, seen ? 'overdue 1' : 'overdue 0', Ph.x + Ph.w / 2, Ph.y + 52, { size: 12, weight: 600, align: 'center', color: seen ? C.signal : C.ink2 });
      var msg = seen ? 'Censorship detected: certified headers count 1 overdue intent.'
        : 'Checks the forced-inclusion count in every certified header.';
      U.para(ctx, msg, T.x, T.y, T.w, { size: L.narrow ? 12 : 13, lh: 17, color: seen ? C.signal : C.ink2, weight: seen ? 600 : 400 });

      /* narration */
      var story;
      if (t < 2.4) story = 'The gateway acknowledges the payment and does nothing with it.';
      else if (t < 4.8) story = 'The user posts it straight to consensus. One honest proposer is enough; the shard certifies it and it executes.';
      else if (t < 10.8) story = 'Now two of the shard’s four validators censor as well. The shard has 8 Pulses to execute the intent.';
      else if (t < 12.4) story = 'It misses the deadline, and every certified header says so. Censorship stops being silent.';
      else story = 'The missed deadline rotates the committee: validators from outside the shard take its seats.';
      U.line(ctx, L.story.x, L.story.y - 22, L.story.x + L.story.w, L.story.y - 22, { color: C.rule });
      if (L.narrow) U.para(ctx, story, L.story.x, L.story.y, L.story.w, { size: 13, lh: 18 });
      else U.text(ctx, story, L.story.x, L.story.y, { size: 15 });
      ctx.restore();
    }
  });
})();
