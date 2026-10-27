/* Data availability sampling, as in crates/sion-da.
 *
 * A Pulse's data is laid out as a k x k square (k = 8) and extended with a
 * two-dimensional Reed-Solomon code to 2k x 2k: rows first, then columns.
 * Rows and columns are committed with hash-based Merkle roots. To make the
 * data unrecoverable a producer must withhold at least (k + 1)^2 of the 4k^2
 * shares, more than a quarter (here a 9 x 9 block). Light clients sample
 * random shares; each sample hits a gap with probability above 1/4, so one
 * client misses the withholding with probability at most (3/4)^s.
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;
  var D = 17, K = 8, N = 16;
  var W0 = { r0: 4, c0: 6, n: K + 1 };

  function withheld(r, c) {
    return r >= W0.r0 && r < W0.r0 + W0.n && c >= W0.c0 && c < W0.c0 + W0.n;
  }

  /* each client's samples, until the first missing share */
  var CLIENTS = [];
  (function build() {
    var wanted = [3, 1, 5];
    for (var c = 0; c < 3; c++) {
      for (var k = 0; k < 500; k++) {
        var rr = U.rng(1009 + c * 97 + k * 7919), list = [], hit = -1;
        for (var j = 0; j < 16; j++) {
          var cell = [Math.floor(rr() * N), Math.floor(rr() * N)];
          list.push(cell);
          if (hit < 0 && withheld(cell[0], cell[1])) hit = j;
        }
        if (hit === wanted[c]) {
          CLIENTS.push({ samples: list.slice(0, hit + 1), hit: hit });
          break;
        }
      }
    }
  })();

  function layout(narrow) {
    if (!narrow) {
      return { gx: 60, gy: 74, cell: 22, gap: 2,
        phones: [[512, 74], [512, 198], [512, 322]], pw: 96, ph: 110,
        text: { x: 650, y: 96, w: 280 }, legend: { x: 60, y: 506 }, narrow: false };
    }
    return { gx: 21, gy: 30, cell: 18, gap: 2,
      phones: [[22, 380], [136, 380], [250, 380]], pw: 88, ph: 118,
      text: { x: 16, y: 522, w: 328 }, legend: null, narrow: true };
  }

  function cellXY(L, r, c) {
    return [L.gx + c * (L.cell + L.gap), L.gy + r * (L.cell + L.gap)];
  }

  function sup(ctx, base, exp, x, y, size, o) {
    U.text(ctx, base, x, y, Object.assign({ size: size }, o));
    var w = U.measure(ctx, base, size, o && o.weight, o && o.mono);
    U.text(ctx, exp, x + w + 1, y - size * 0.42, Object.assign({}, o, { size: size * 0.7 }));
    return w + U.measure(ctx, exp, size * 0.7, o && o.weight, o && o.mono) + 2;
  }

  S.scene('da', {
    duration: D,
    still: 12.4,
    wide: { w: 960, h: 540 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var L = layout(view.narrow);
      ctx.save();
      ctx.globalAlpha = 1 - U.span(t, D - 0.8, D);
      var gw = N * L.cell + (N - 1) * L.gap;
      var hold = U.span(t, 5.7, 6.5);
      var r, c;

      for (r = 0; r < N; r++) {
        for (c = 0; c < N; c++) {
          var xy = cellXY(L, r, c), x = xy[0], y = xy[1], s = L.cell;
          var appear, alpha;
          if (r < K && c < K) { appear = 0.1 + r * 0.17 + c * 0.012; alpha = 1; }
          else if (r < K) { appear = 1.6 + (c - K) * 0.16 + r * 0.01; alpha = 0.42; }
          else { appear = 3.0 + (r - K) * 0.16 + c * 0.01; alpha = 0.42; }
          var a = U.span(t, appear, appear + 0.25);
          U.rect(ctx, x, y, s, s, { stroke: C.rule, lw: 1 });
          if (withheld(r, c) && hold > 0) {
            U.rect(ctx, x, y, s, s, { fill: C.ink, alpha: a * alpha * (1 - hold) });
            U.rect(ctx, x + 1, y + 1, s - 2, s - 2, { stroke: C.signal, lw: 1, dash: [2, 2], alpha: hold * 0.9 });
          } else {
            U.rect(ctx, x, y, s, s, { fill: C.ink, alpha: a * alpha });
          }
        }
      }

      /* commitments: a Merkle root per row and per column */
      var commit = U.span(t, 4.5, 5.3);
      if (commit > 0) {
        for (var i = 0; i < N; i++) {
          var rxy = cellXY(L, i, 0), cxy = cellXY(L, 0, i);
          U.rect(ctx, L.gx + gw + 5, rxy[1] + L.cell / 2 - 3, 6, 6, { fill: C.ink2, alpha: U.span(t, 4.5 + i * 0.03, 4.8 + i * 0.03) });
          U.rect(ctx, cxy[0] + L.cell / 2 - 3, L.gy + gw + 5, 6, 6, { fill: C.ink2, alpha: U.span(t, 4.7 + i * 0.03, 5.0 + i * 0.03) });
        }
      }

      /* light clients sample random shares */
      CLIENTS.forEach(function (cl, ci) {
        var px = L.phones[ci][0], py = L.phones[ci][1];
        U.phone(ctx, px, py, L.pw, L.ph, { stroke: C.ink, lw: 1.25 });
        var t0 = 7.0 + ci * 0.15, step = 0.42;
        var done = 0;
        cl.samples.forEach(function (cell, j) {
          var at = t0 + j * step;
          if (t < at) return;
          done = j + 1;
          var xy = cellXY(L, cell[0], cell[1]);
          var cx = xy[0] + L.cell / 2, cy = xy[1] + L.cell / 2;
          var miss = j === cl.hit;
          var live = U.span(t, at, at + 0.25);
          var current = t < at + step || miss;
          if (current) {
            var lx = view.narrow ? px + L.pw / 2 : px;
            var ly = view.narrow ? py : py + L.ph / 2;
            U.line(ctx, lx, ly, U.lerp(lx, cx, live), U.lerp(ly, cy, live), { color: miss ? C.signal : C.ink3, lw: 1 });
          }
          if (live >= 1) {
            U.rect(ctx, xy[0] - 2, xy[1] - 2, L.cell + 4, L.cell + 4, { stroke: miss ? C.signal : C.ink, lw: 1.5 });
          }
        });
        var detected = done > cl.hit && t >= t0 + cl.hit * step + 0.25;
        var fs = view.narrow ? 11 : 12, cxp = px + L.pw / 2;
        U.text(ctx, 'client ' + (ci + 1), cxp, py + 22, { size: fs, color: C.ink3, align: 'center' });
        U.text(ctx, done + ' of 16', cxp, py + 46, { size: 16, weight: 600, align: 'center' });
        U.text(ctx, 'samples', cxp, py + 62, { size: fs, color: C.ink3, align: 'center' });
        if (t >= t0) {
          U.text(ctx, detected ? 'missing share' : 'all available', cxp, py + L.ph - 28,
            { size: fs, weight: detected ? 600 : 400, color: detected ? C.signal : C.ink2, align: 'center' });
          if (detected) U.text(ctx, 'Pulse rejected', cxp, py + L.ph - 13, { size: fs, color: C.signal, align: 'center' });
        }
      });

      /* what the picture says */
      var T = L.text, fs2 = view.narrow ? 12 : 14, lh = view.narrow ? 17 : 21;
      var line1 = hold > 0 ? 'Withheld: (k + 1)\u00b2 = 81 of 4k\u00b2 = 256 shares, more than a quarter.'
        : (commit > 0 ? 'Every row and every column gets a Merkle root. No trusted setup.' : 'k \u00d7 k data, extended to 2k \u00d7 2k with a two-dimensional Reed\u2013Solomon code.');
      var used = U.para(ctx, line1, T.x, T.y, T.w, { size: fs2, lh: lh, color: hold > 0 ? C.signal : C.ink, weight: view.narrow ? 400 : 600 });
      if (!view.narrow || t > 6.5) {
        var y2 = T.y + used + (view.narrow ? 4 : 14);
        var lead = view.narrow ? 'One client misses it with probability at most ' : 'One client taking s samples misses it with probability at most';
        var leadH = U.para(ctx, lead, T.x, y2, T.w, { size: fs2, lh: lh, color: C.ink2 });
        var y3 = view.narrow ? y2 : y2 + leadH;
        var x3 = view.narrow ? T.x + U.measure(ctx, lead, fs2) + 2 : T.x;
        sup(ctx, '(3/4)', 's', x3, y3, view.narrow ? fs2 : 18, { color: C.ink, weight: 600 });
        var y4 = y3 + (view.narrow ? lh : 30);
        if (!view.narrow) {
          U.text(ctx, 's = 16: about 1%', T.x, y4, { size: fs2, color: C.ink2 });
          U.text(ctx, 's = 30: about 0.02%', T.x, y4 + lh, { size: fs2, color: C.ink2 });
          U.para(ctx, 'With many independent clients sampling, undetected withholding becomes vanishingly unlikely.',
            T.x, y4 + 2 * lh + 14, T.w, { size: 13, lh: 19, color: C.ink3 });
        } else {
          U.text(ctx, 's = 16: about 1%;  s = 30: about 0.02%', T.x, y4, { size: fs2, color: C.ink2 });
        }
      }
      if (L.legend) {
        var lg = L.legend;
        U.rect(ctx, lg.x, lg.y - 10, 11, 11, { fill: C.ink });
        U.text(ctx, 'data share', lg.x + 18, lg.y, { size: 12, color: C.ink2 });
        U.rect(ctx, lg.x + 110, lg.y - 10, 11, 11, { fill: C.ink, alpha: 0.42 });
        U.text(ctx, 'extension share', lg.x + 128, lg.y, { size: 12, color: C.ink2 });
        U.rect(ctx, lg.x + 250, lg.y - 10, 11, 11, { stroke: C.signal, dash: [2, 2] });
        U.text(ctx, 'withheld', lg.x + 268, lg.y, { size: 12, color: C.ink2 });
        U.rect(ctx, lg.x + 340, lg.y - 9, 6, 6, { fill: C.ink2 });
        U.text(ctx, 'row and column roots', lg.x + 352, lg.y, { size: 12, color: C.ink2 });
      }
      ctx.restore();
    }
  });
})();
