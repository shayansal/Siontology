/* Payment streams, as in `sion demo streams` and `sion bench --only streams`.
 *
 * An AI agent pays an API per call from a payment stream: each payment is
 * the next element of the agent's hash chain (an outline cell: not final
 * yet), and the API checks it with one hash against the last one. The
 * payments speed up to a million. The API then settles them all in one
 * transaction, ordered by consensus (a hatched cell), proven by a jump to the
 * checkpoint below its position: 524 hashes and one signature check for
 * 1,048,064 payments. On the ledger it becomes final (a solid cell).
 * The closing numbers are the measured ones (one core, release build).
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;
  var D = 17;
  var PAY_START = 0.8, PAY_END = 9.4;
  var SETTLE_START = 9.7, SETTLE_END = 11.3;
  var TOTAL = 1048064;

  /* Payments made by time t: from 1 to TOTAL, doubling at a steady rate. */
  function count(t) {
    if (t < PAY_START) return 0;
    var p = U.clamp((t - PAY_START) / (PAY_END - PAY_START));
    return Math.min(TOTAL, Math.max(1, Math.round(Math.pow(2, 20 * p))));
  }

  /* Distance the flow has travelled by time t, for a speed that doubles
     every `double` seconds: the integral of v0 · 2^(s / double). */
  function travelled(t, v0, double) {
    var s = Math.max(0, Math.min(t, PAY_END) - PAY_START);
    var k = Math.LN2 / double;
    return v0 * (Math.exp(k * s) - 1) / k;
  }

  function speed(t, v0, double) {
    var s = Math.max(0, Math.min(t, PAY_END) - PAY_START);
    return v0 * Math.pow(2, s / double);
  }

  function layout(narrow) {
    if (!narrow) {
      return {
        narrow: false,
        agent: { x: 110, y: 120 },
        api: { x: 560, y: 120, w: 200, h: 64 },
        flow: { x1: 150, y1: 120, x2: 452, y2: 120 },
        check: { x: 690, y: 104 },
        ledger: { x: 40, y: 336, w: 880, h: 64 },
        cells: 8,
        target: 4,
        from: { x: 560, y: 152 },
        summary: { x: 40, y: 452 }
      };
    }
    return {
      narrow: true,
      agent: { x: 180, y: 48 },
      api: { x: 180, y: 250, w: 200, h: 58 },
      flow: { x1: 180, y1: 76, x2: 180, y2: 214 },
      check: { x: 150, y: 306 },
      ledger: { x: 20, y: 382, w: 320, h: 56 },
      cells: 5,
      target: 4,
      from: { x: 262, y: 279 },
      summary: { x: 20, y: 512 }
    };
  }

  function drawFlow(ctx, L, t, alpha) {
    var f = L.flow;
    var len = Math.hypot(f.x2 - f.x1, f.y2 - f.y1);
    var ux = (f.x2 - f.x1) / len, uy = (f.y2 - f.y1) / len;
    U.line(ctx, f.x1, f.y1, f.x2, f.y2, { color: C.rule2, dash: [3, 4], alpha: alpha });
    var gap = L.narrow ? 16 : 20, size = L.narrow ? 7 : 8;
    if (t > PAY_END + 0.4) {
      /* The stream at rest: open, waiting for the next call. */
      var rest = U.span(t, PAY_END + 0.4, PAY_END + 1.0);
      for (var r = gap / 2; r < len - size; r += gap * 2) {
        U.rect(ctx, f.x1 + ux * r - size / 2, f.y1 + uy * r - size / 2, size, size,
          { stroke: C.ink2, lw: 1, fill: C.bg, alpha: alpha * rest * 0.8 });
      }
      return;
    }
    if (t < PAY_START) return;
    var v = speed(t, 60, 1.25);
    var offset = travelled(t, 60, 1.25) % gap;
    var fade = 1 - U.span(t, PAY_END, PAY_END + 0.4);
    /* Fast enough to blur: draw the flow as a solid line too. */
    var blur = U.clamp((v - 400) / 1600);
    if (blur > 0) U.line(ctx, f.x1, f.y1, f.x2, f.y2, { color: C.ink, lw: 1.5, alpha: alpha * blur * fade });
    for (var d = offset; d < len - size; d += gap) {
      var x = f.x1 + ux * d, y = f.y1 + uy * d;
      U.rect(ctx, x - size / 2, y - size / 2, size, size,
        { stroke: C.ink, lw: 1.25, fill: C.bg, alpha: alpha * fade * (1 - 0.6 * blur) });
    }
  }

  S.scene('streams', {
    duration: D,
    still: 12.6,
    wide: { w: 960, h: 540 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var L = layout(view.narrow);
      ctx.save();
      ctx.globalAlpha = 1 - U.span(t, D - 0.8, D);
      var intro = U.span(t, 0, 0.6);
      var n = count(t);

      /* The agent, and the stream it opened. */
      U.diamond(ctx, L.agent.x, L.agent.y, L.narrow ? 16 : 20, { alpha: intro });
      if (L.narrow) {
        U.text(ctx, 'AI agent', L.agent.x + 28, L.agent.y + 5, { size: 12, weight: 600, alpha: intro });
      } else {
        U.text(ctx, 'AI agent', L.agent.x, L.agent.y + 44, { size: 14, weight: 600, align: 'center', alpha: intro });
        U.text(ctx, 'escrow in a stream', L.agent.x, L.agent.y + 61, { size: 11, color: C.ink3, align: 'center', alpha: intro });
      }

      /* The API. */
      var a = L.api;
      U.rect(ctx, a.x - a.w / 2, a.y - a.h / 2, a.w, a.h, { stroke: C.ink, lw: 1.25, alpha: intro });
      U.text(ctx, 'the API', a.x, a.y - 4, { size: L.narrow ? 13 : 14, weight: 600, align: 'center', alpha: intro });
      U.text(ctx, '0.001 USD per call', a.x, a.y + 14, { size: 11, color: C.ink3, align: 'center', alpha: intro });

      drawFlow(ctx, L, t, intro);

      /* The payment counter, over the flow. */
      var shown = U.win(t, PAY_START, SETTLE_END + 3.8, 0.4);
      var label = n === 1 ? '1 call paid' : U.fmt(n) + ' calls paid';
      if (L.narrow) {
        U.text(ctx, label, L.flow.x1 + 14, (L.flow.y1 + L.flow.y2) / 2, { size: 13, weight: 600, mono: true, alpha: shown });
        U.text(ctx, 'one chain element each', L.flow.x1 + 14, (L.flow.y1 + L.flow.y2) / 2 + 16, { size: 11, color: C.ink3, alpha: shown });
      } else {
        U.text(ctx, label, (L.flow.x1 + L.flow.x2) / 2, L.flow.y1 - 22, { size: 15, weight: 600, mono: true, align: 'center', alpha: shown });
        U.text(ctx, 'each paid with the next element of a hash chain,', L.flow.x1, L.flow.y1 + 24, { size: 11, color: C.ink3, alpha: shown });
        U.text(ctx, 'off the ledger', L.flow.x1, L.flow.y1 + 39, { size: 11, color: C.ink3, alpha: shown });
      }

      /* What the API checks for each payment. */
      var c = L.check;
      var checking = U.span(t, PAY_START + 0.2, PAY_START + 0.6);
      if (n > 0) {
        var eq = 'H(w' + U.fmt(n) + ') = w' + U.fmt(n - 1);
        var fs = L.narrow ? 11 : 12;
        var align = L.narrow ? 'center' : 'left';
        U.text(ctx, 'checks each payment with one hash', c.x, c.y, { size: fs, weight: 600, align: align, alpha: checking });
        var w = U.measure(ctx, eq, fs, 450, true);
        var ex = L.narrow ? c.x - w / 2 - 6 : c.x;
        U.text(ctx, eq, ex, c.y + 19, { size: fs, mono: true, alpha: checking });
        U.check(ctx, ex + w + 10, c.y + 15, 9, { alpha: checking });
        if (!L.narrow) {
          U.text(ctx, '0.42 µs, no signature, no ledger read', c.x, c.y + 38, { size: 11, color: C.ink3, alpha: checking });
        }
      }

      /* The ledger: a row of Pulses. */
      var g = L.ledger;
      U.text(ctx, 'Siontology', g.x, g.y - 10, { size: L.narrow ? 12 : 13, weight: 600, alpha: intro });
      var cw = g.w / L.cells;
      for (var i = 0; i < L.cells; i++) {
        var x = g.x + i * cw;
        U.rect(ctx, x, g.y, cw, g.h, { stroke: C.rule2, lw: 1, alpha: intro });
        U.text(ctx, 'Pulse ' + (37 + i), x + 8, g.y + 16, { size: 10, color: C.ink3, mono: true, alpha: intro });
      }

      /* The settlement: one transaction for every payment so far. */
      var tx = g.x + L.target * cw + cw * 0.7, ty = g.y + g.h / 2 + 6;
      var sp = U.span(t, SETTLE_START, SETTLE_END);
      var landed = t >= SETTLE_END;
      var s = L.narrow ? 14 : 16;
      var fx = L.from.x, fy = L.from.y;
      if (t >= SETTLE_START && !landed) {
        var sx = U.lerp(fx, tx, sp), sy = U.lerp(fy, ty, sp);
        U.line(ctx, fx, fy, sx, sy - s / 2 - 2, { color: C.ink2, lw: 1 });
        U.rect(ctx, sx - s / 2, sy - s / 2, s, s, { fill: C.bg });
        U.hatch(ctx, sx - s / 2, sy - s / 2, s, s, { gap: 3 });
        U.rect(ctx, sx - s / 2, sy - s / 2, s, s, { stroke: C.ink, lw: 1.25 });
        var tag = L.narrow ? 'one settlement' : 'SettleStream: position ' + U.fmt(TOTAL) + ', by checkpoint';
        U.text(ctx, tag, L.narrow ? sx - s : sx + s, sy + 4,
          { size: L.narrow ? 11 : 12, weight: 600, align: L.narrow ? 'right' : 'left', alpha: U.span(t, SETTLE_START, SETTLE_START + 0.3) });
      }
      if (landed) {
        var k = U.span(t, SETTLE_END, SETTLE_END + 0.35);
        U.line(ctx, fx, fy, tx, ty - s / 2 - 2, { color: C.rule2, lw: 1, dash: [3, 4], alpha: k });
        U.rect(ctx, tx - s / 2, ty - s / 2, s, s, { fill: C.ink, alpha: k });
        var lines = L.narrow
          ? ['1 transaction: ' + U.fmt(TOTAL) + ' payments,', '524 hashes, one signature check']
          : ['1 transaction:', U.fmt(TOTAL) + ' payments, 524 hashes,', 'one signature check'];
        var lx = L.narrow ? g.x : tx + s;
        var ly = L.narrow ? g.y + g.h + 20 : g.y - 44;
        lines.forEach(function (row, j) {
          U.text(ctx, row, lx, ly + j * 16, { size: L.narrow ? 11 : 12, weight: j === 0 ? 600 : 450, alpha: k });
        });
      }

      /* The measured cost per payment. */
      var sum = U.span(t, SETTLE_END + 0.6, SETTLE_END + 1.1);
      if (sum > 0) {
        var m = L.summary;
        if (L.narrow) {
          U.text(ctx, 'Per payment, measured on one core:', m.x, m.y, { size: 11, color: C.ink3, alpha: sum });
          U.text(ctx, '0.42 µs at the API, one hash', m.x, m.y + 18, { size: 12, weight: 600, alpha: sum });
          U.text(ctx, 'about 1 ns on the ledger, by checkpoint', m.x, m.y + 36, { size: 12, weight: 600, alpha: sum });
        } else {
          U.text(ctx, 'Per payment, measured on one core:', m.x, m.y, { size: 12, color: C.ink3, alpha: sum });
          U.text(ctx, '0.42 µs at the API, one hash.', m.x + 222, m.y, { size: 13, weight: 600, alpha: sum });
          U.text(ctx, 'About 1 ns on the ledger, settled by checkpoint.', m.x + 452, m.y, { size: 13, weight: 600, alpha: sum });
          U.text(ctx, 'Payments are one-way, out of escrow. Settlement rates are from one machine, not a network.', m.x, m.y + 24,
            { size: 11, color: C.ink3, alpha: sum });
        }
      }
      ctx.restore();
    }
  });
})();
