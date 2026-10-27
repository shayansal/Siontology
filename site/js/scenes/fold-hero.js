/* Hero: the Fold, one World Proof per Pulse.
 *
 * Operations (claims and transactions) stream into leaf batches. Every Pulse
 * (one second), each level of the aggregation tree folds pairwise into the
 * level above, as a pipeline: Pulse t+1 starts aggregating while Pulse t's
 * root finishes, so the World Proof lags the newest Pulse by the pipeline's
 * length. The root joins the World Proof, a pair of roots from the two
 * independent proof stacks (A solid, B outline). This follows "The Fold" in
 * the architecture paper; on today's devnet the proofs are checked by replay.
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;

  function layout(w, h) {
    var wide = w >= 768;
    var gut = wide ? 32 : 16;
    var side = Math.max(gut, (w - 1360) / 2 + gut);
    var N = w >= 1100 ? 32 : 16;
    var R = Math.round(Math.log(N) / Math.LN2) + 1;
    var cg = N >= 32 ? 4 : 3;
    var top = wide ? 92 : 84;
    var barH = 6, barGap = 4, wpBlock = barH * 2 + barGap;
    var below = wide ? 26 : 20;
    var bottom = wide ? 64 : 54;
    var rg = wide ? 11 : 8;
    var q = 0.88, sum = 0, i;
    for (i = 0; i < R; i++) sum += Math.pow(q, i);
    var avail = h - top - wpBlock - below - bottom - (R - 1) * rg;
    var base = Math.max(8, Math.min(wide ? 52 : 40, avail / sum));
    var used = base * sum + (R - 1) * rg;
    var Wt = w - 2 * side;
    var rows = new Array(R);
    var y = top + wpBlock + below + Math.max(0, (avail + (R - 1) * rg - used) / 2);
    for (var j = 0; j < R; j++) {
      var k = R - 1 - j;
      var hk = base * Math.pow(q, j);
      var n = N / Math.pow(2, k);
      rows[k] = { y: y, h: hk, n: n, cw: (Wt - (n - 1) * cg) / n };
      y += hk + rg;
    }
    return { wide: wide, side: side, N: N, R: R, cg: cg, top: top, barH: barH, barGap: barGap,
      wpBlock: wpBlock, rows: rows, Wt: Wt, leafBottom: rows[0].y + rows[0].h, h: h, w: w };
  }

  function cellX(L, k, i) {
    return L.side + i * (L.rows[k].cw + L.cg);
  }

  /* leaf kinds: 0 lane batch (solid), 1 claim batch (outline), 2 contended batch (hatched) */
  function kind(pulse, i) {
    var r = U.rng(pulse * 7919 + i * 104729)();
    return r < 0.5 ? 0 : r < 0.85 ? 1 : 2;
  }

  function drawLeaf(ctx, x, y, w, h, kd, fill, alpha) {
    if (fill <= 0) return;
    var hh = h * fill, yy = y + h - hh;
    if (kd === 0) U.rect(ctx, x, yy, w, hh, { fill: C.ink, alpha: alpha });
    else if (kd === 1) U.rect(ctx, x, yy, w, hh, { stroke: C.ink, lw: 1.25, alpha: alpha });
    else {
      U.rect(ctx, x, yy, w, hh, { stroke: C.ink, lw: 1.25, alpha: alpha });
      U.hatch(ctx, x, yy, w, hh, { color: C.ink, gap: 4, alpha: alpha * 0.85 });
    }
  }

  /* An aggregated proof: a light body, a hairline outline and a sealed top edge. */
  function drawProof(ctx, x, y, w, h, level, R, alpha) {
    var f = level / (R - 1);
    U.rect(ctx, x, y, w, h, { fill: C.ink, alpha: alpha * (0.08 + 0.2 * f) });
    U.rect(ctx, x, y, w, h, { stroke: C.ink, lw: 1, alpha: alpha * (0.35 + 0.35 * f) });
    U.rect(ctx, x, y, w, Math.min(2, h), { fill: C.ink, alpha: alpha * (0.7 + 0.3 * f) });
  }

  S.scene('fold-hero', {
    free: true,
    duration: 0,
    start: 5.6,
    still: 6.78,
    draw: function (ctx, T, view) {
      var L = layout(view.w, view.h);
      var s = Math.floor(T), p = T - s;
      var m = U.span(p, 0, 0.55);
      var k, i;

      /* slots: the structure, always visible */
      for (k = 0; k < L.R; k++) {
        var row = L.rows[k];
        for (i = 0; i < row.n; i++) {
          U.rect(ctx, cellX(L, k, i), row.y, row.cw, row.h, { stroke: C.rule, lw: 1 });
        }
        if (L.side >= 40) {
          U.text(ctx, String(row.n), L.side - 12, row.y + row.h / 2 + 4, { size: 11, color: C.ink3, align: 'right' });
        }
      }
      U.rect(ctx, L.side, L.top, L.Wt, L.barH, { stroke: C.rule, lw: 1 });
      U.rect(ctx, L.side, L.top + L.barH + L.barGap, L.Wt, L.barH, { stroke: C.rule, lw: 1 });
      if (L.side >= 40) {
        U.text(ctx, 'A', L.side - 12, L.top + L.barH - 0.5, { size: 10, color: C.ink3, align: 'right' });
        U.text(ctx, 'B', L.side - 12, L.top + 2 * L.barH + L.barGap - 0.5, { size: 10, color: C.ink3, align: 'right' });
      }

      /* the operations streaming into the current Pulse's leaves */
      var pulse = s + 1;
      var leaf = L.rows[0];
      var per = 4;
      for (i = 0; i < leaf.n; i++) {
        var r = U.rng(pulse * 31 + i * 977);
        var arrived = 0;
        for (var q = 0; q < per; q++) {
          var start = 0.28 + r() * 0.42;
          var tp = (p - start) / 0.38;
          var jitter = (r() - 0.5) * leaf.cw * 0.6;
          var cx = cellX(L, 0, i) + leaf.cw / 2 + jitter;
          if (tp >= 1) arrived++;
          else if (tp > 0) {
            var yy = U.lerp(L.h - 8, L.leafBottom + 2, U.easeOut(tp));
            U.rect(ctx, cx - 1.5, yy - 1.5, 3, 3, { fill: C.ink2 });
          }
        }
        drawLeaf(ctx, cellX(L, 0, i), leaf.y, leaf.cw, leaf.h, kind(pulse, i), arrived / per, 0.9);
      }

      /* every level folds pairwise into the level above, all at once */
      for (k = 1; k < L.R; k++) {
        var content = s + 1 - k;
        if (content < 1) continue;
        var from = L.rows[k - 1], to = L.rows[k];
        var y = U.lerp(from.y, to.y, m), hh = U.lerp(from.h, to.h, m);
        for (i = 0; i < to.n; i++) {
          var x0 = cellX(L, k, i), x1 = x0 + to.cw;
          var gap = L.cg * (1 - m);
          var mid = x0 + to.cw / 2;
          var lw = mid - gap / 2 - x0;
          if (k === 1 && m < 1) {
            /* leaves keep their kind for a moment, then become proofs */
            for (var c = 0; c < 2; c++) {
              var xx = c === 0 ? x0 : mid + gap / 2;
              drawLeaf(ctx, xx, y, lw, hh, kind(content, 2 * i + c), 1, 0.9 * (1 - m));
              drawProof(ctx, xx, y, lw, hh, k, L.R, m);
            }
          } else if (m < 1) {
            drawProof(ctx, x0, y, lw, hh, k, L.R, 1);
            drawProof(ctx, mid + gap / 2, y, x1 - mid - gap / 2, hh, k, L.R, 1);
          } else {
            drawProof(ctx, x0, y, to.cw, hh, k, L.R, 1);
          }
        }
      }

      /* the root joins the World Proof: a pair of roots, stack A and stack B */
      var rootRow = L.rows[L.R - 1];
      var incoming = s + 1 - L.R;
      var shown = m >= 1 ? incoming : incoming - 1;
      if (incoming >= 1 && m < 1) {
        var y2 = U.lerp(rootRow.y, L.top, m), h2 = U.lerp(rootRow.h, L.wpBlock, m);
        drawProof(ctx, L.side, y2, L.Wt, h2, L.R - 1, L.R, 1 - 0.3 * m);
      }
      if (shown >= 1) {
        U.rect(ctx, L.side, L.top, L.Wt, L.barH, { fill: C.ink });
        U.rect(ctx, L.side, L.top + L.barH + L.barGap, L.Wt, L.barH, { stroke: C.ink, lw: 1.5 });
        var flash = incoming >= 1 ? U.win(p, 0.55, 0.95, 0.12) : 0;
        if (flash > 0) {
          U.rect(ctx, L.side - 4, L.top - 4, L.Wt + 8, L.wpBlock + 8, { stroke: C.ink, lw: 1, alpha: flash * 0.5 });
        }
      }

      /* labels */
      U.text(ctx, 'World Proof', L.side, L.wide ? 38 : 34, { size: L.wide ? 15 : 13, weight: 600 });
      var sub;
      if (shown >= 1) {
        sub = L.wide ? 'for Pulse ' + shown + ', covering everything from genesis: stack A and stack B both accept'
          : 'for Pulse ' + shown + ': stacks A and B accept';
      } else {
        sub = 'folding the first Pulses';
      }
      U.text(ctx, sub, L.side, L.wide ? 60 : 54, { size: L.wide ? 13 : 12, color: C.ink3 });
      U.text(ctx, 'Pulse', L.w - L.side, L.wide ? 30 : 26, { size: 12, color: C.ink3, align: 'right' });
      U.text(ctx, String(pulse), L.w - L.side, L.wide ? 64 : 56, { size: L.wide ? 32 : 26, weight: 500, align: 'right' });
    }
  });
})();
