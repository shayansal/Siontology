/* Verifiable Clearing, as in `sion demo market`.
 *
 * Orders: agent-1 bids for 5 GPU-hours at up to 2.10 USD, agent-2 for 3 at up
 * to 1.90; lab-a offers 5 at 1.80 or more, lab-b 6 at 2.00 or more. The batch
 * seals with the Pulse beacon and is revealed. The welfare-maximising
 * allocation trades 5 GPU-hours; every uniform price from 1.90 to 2.00
 * supports it and the canonical price is the midpoint, 1.95 USD (welfare
 * 1.50 USD). The ledger checks the optimality certificate in linear time.
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;
  var D = 17;

  var ORDERS = [
    { who: 'agent-1', side: 'buy', q: 5, p: '2.10' },
    { who: 'agent-2', side: 'buy', q: 3, p: '1.90' },
    { who: 'lab-a', side: 'sell', q: 5, p: '1.80' },
    { who: 'lab-b', side: 'sell', q: 6, p: '2.00' }
  ];
  var CHECKS = [
    [['Ax* ≤ b'], 'the allocation is feasible'],
    [['x* ≥ 0'], 'and non-negative'],
    [['A', 'T', 'y* ≥ c'], 'the prices are feasible'],
    [['y* ≥ 0'], 'and non-negative'],
    [['c', 'T', 'x* ≥ b', 'T', 'y* − ε,  ε = 0'], 'no duality gap: optimal']
  ];

  function layout(narrow) {
    if (!narrow) {
      return {
        orders: ORDERS.map(function (o, i) { return { x: 40, y: 86 + i * 56, w: 270, h: 44 }; }),
        beacon: { x: 40, y: 334 },
        plot: { x0: 392, x1: 700, y0: 92, y1: 380 },
        checks: { x: 736, y: 112, step: 44, two: true },
        story: { x: 40, y: 470, w: 880 }
      };
    }
    return {
      orders: ORDERS.map(function (o, i) { return { x: 16 + (i % 2) * 168, y: 12 + Math.floor(i / 2) * 52, w: 160, h: 44 }; }),
      beacon: { x: 16, y: 130 },
      plot: { x0: 60, x1: 340, y0: 168, y1: 380 },
      checks: { x: 16, y: 430, step: 24, two: false },
      story: { x: 16, y: 540, w: 328 },
      narrow: true
    };
  }

  /* formula pieces alternate base text and superscripts: [base, sup, base, sup, ...] */
  function formula(ctx, parts, x, y, size, o) {
    var cx = x;
    parts.forEach(function (part, k) {
      var supr = k % 2 === 1;
      var s = supr ? size * 0.7 : size;
      U.text(ctx, part, cx, supr ? y - size * 0.42 : y, Object.assign({}, o, { size: s, mono: true }));
      cx += U.measure(ctx, part, s, 400, true) + (supr ? 1 : 0);
    });
  }

  S.scene('clearing', {
    duration: D,
    still: 11.2,
    wide: { w: 960, h: 540 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var L = layout(view.narrow);
      ctx.save();
      ctx.globalAlpha = 1 - U.span(t, D - 0.8, D);

      /* the sealed batch */
      if (!L.narrow) {
        U.text(ctx, 'Sealed batch', 40, 44, { size: 15, weight: 600 });
        U.text(ctx, 'orders collected during the batch window', 40, 64, { size: 12, color: C.ink3 });
      }
      L.orders.forEach(function (b, i) {
        var o = ORDERS[i];
        var a = U.span(t, 0.3 + i * 0.4, 0.7 + i * 0.4);
        var rev = U.span(t, 2.8 + i * 0.25, 3.2 + i * 0.25);
        U.rect(ctx, b.x, b.y, b.w, b.h, { stroke: C.ink, lw: 1.25, alpha: a });
        U.text(ctx, o.who, b.x + 10, b.y + 18, { size: 12, weight: 600, alpha: a });
        /* sealed: an envelope mark */
        var sealedA = a * (1 - rev);
        if (sealedA > 0) {
          var ex = b.x + b.w - 34, ey = b.y + 12;
          U.rect(ctx, ex, ey, 22, 16, { stroke: C.ink2, lw: 1, alpha: sealedA });
          U.poly(ctx, [[ex, ey], [ex + 11, ey + 9], [ex + 22, ey]], { color: C.ink2, lw: 1, alpha: sealedA });
          U.text(ctx, 'sealed', b.x + 10, b.y + b.h - 9, { size: 11, color: C.ink3, alpha: sealedA });
        }
        if (rev > 0) {
          var terms = o.side === 'buy'
            ? (L.narrow ? 'buy ' + o.q + ' at ≤ ' + o.p : 'bids for ' + o.q + ' GPU-hours at ≤ ' + o.p)
            : (L.narrow ? 'sell ' + o.q + ' at ≥ ' + o.p : 'offers ' + o.q + ' GPU-hours at ≥ ' + o.p);
          U.text(ctx, terms, b.x + 10, b.y + b.h - 9, { size: 12, color: C.ink2, alpha: rev });
        }
      });
      var beaconA = U.span(t, 2.0, 2.6);
      if (L.narrow) {
        U.text(ctx, 'sealed with the Pulse beacon: no reordering', L.beacon.x, L.beacon.y, { size: 12, color: C.ink2, alpha: beaconA });
      } else {
        U.para(ctx, 'Sealed with the Pulse beacon 1d9e205b, so nobody can reorder it.', L.beacon.x, L.beacon.y, 270,
          { size: 12, lh: 17, color: C.ink2, alpha: beaconA });
      }

      /* supply and demand */
      var P = L.plot;
      var X = function (q) { return P.x0 + (q / 12) * (P.x1 - P.x0); };
      var Y = function (p) { return P.y1 - ((p - 1.7) / 0.5) * (P.y1 - P.y0); };
      var ax = U.span(t, 4.0, 4.4);
      if (ax > 0) {
        U.line(ctx, P.x0, P.y1, P.x1, P.y1, { color: C.ink2, alpha: ax });
        U.line(ctx, P.x0, P.y0, P.x0, P.y1, { color: C.ink2, alpha: ax });
        [1.7, 1.8, 1.9, 2.0, 2.1, 2.2].forEach(function (p) {
          U.line(ctx, P.x0, Y(p), P.x1, Y(p), { color: C.rule, alpha: ax * 0.8 });
          U.text(ctx, p.toFixed(2), P.x0 - 8, Y(p) + 4, { size: 11, color: C.ink3, align: 'right', alpha: ax });
        });
        for (var q = 0; q <= 12; q += 2) {
          U.text(ctx, String(q), X(q), P.y1 + 16, { size: 11, color: C.ink3, align: 'center', alpha: ax });
        }
        U.text(ctx, 'USD per GPU-hour', P.x0, P.y0 - 12, { size: 11, color: C.ink3, alpha: ax });
        U.text(ctx, 'GPU-hours', P.x1, P.y1 + 32, { size: 11, color: C.ink3, align: 'right', alpha: ax });
      }
      var demand = [[0, 2.1], [5, 2.1], [5, 1.9], [8, 1.9], [8, 1.7]];
      var supply = [[0, 1.8], [5, 1.8], [5, 2.0], [11, 2.0], [11, 2.2]];
      function partial(pts, p) {
        var segs = pts.length - 1, upto = p * segs, out = [];
        for (var i = 0; i < pts.length; i++) {
          if (i <= upto) out.push([X(pts[i][0]), Y(pts[i][1])]);
          else {
            var f = upto - (i - 1);
            out.push([U.lerp(X(pts[i - 1][0]), X(pts[i][0]), f), U.lerp(Y(pts[i - 1][1]), Y(pts[i][1]), f)]);
            break;
          }
        }
        return out;
      }
      var dp = U.span(t, 4.4, 5.6, U.linear), sp = U.span(t, 5.0, 6.2, U.linear);
      if (dp > 0) {
        U.poly(ctx, partial(demand, dp), { color: C.ink, lw: 2 });
        U.text(ctx, 'bids', X(0.4), Y(2.1) - 8, { size: 12, weight: 600, alpha: U.span(t, 4.6, 5.0) });
      }
      if (sp > 0) {
        U.poly(ctx, partial(supply, sp), { color: C.ink2, lw: 2, dash: [6, 4] });
        U.text(ctx, 'offers', X(0.4), Y(1.8) + 18, { size: 12, weight: 600, color: C.ink2, alpha: U.span(t, 5.2, 5.6) });
      }
      var band = U.span(t, 6.4, 7.0);
      if (band > 0) {
        U.rect(ctx, P.x0, Y(2.0), P.x1 - P.x0, Y(1.9) - Y(2.0), { fill: C.ink, alpha: 0.1 * band });
        U.line(ctx, X(5), Y(2.0), X(5), Y(1.9), { color: C.ink, lw: 4, alpha: band });
        U.line(ctx, P.x0, Y(1.95), P.x1, Y(1.95), { color: C.ink, lw: 1, dash: [3, 3], alpha: band });
        U.line(ctx, X(5), Y(1.9), X(5), P.y1, { color: C.ink3, lw: 1, dash: [3, 3], alpha: band });
        U.text(ctx, 'one price: 1.95 USD', P.x1, Y(1.95) - 8, { size: 12, weight: 600, align: 'right', alpha: band });
        U.text(ctx, 'band 1.90 to 2.00', P.x1 - 2, Y(1.9) + 16, { size: 11, color: C.ink3, align: 'right', alpha: band });
        U.text(ctx, '5 traded', X(5) + 6, P.y1 - 8, { size: 11, color: C.ink2, alpha: band });
      }

      /* the certificate the ledger checks */
      var K = L.checks;
      if (!L.narrow) U.text(ctx, 'Optimality certificate', K.x, K.y - 24, { size: 13, weight: 600, alpha: U.span(t, 7.2, 7.6) });
      CHECKS.forEach(function (ck, i) {
        var at = 7.4 + i * 0.55;
        var a = U.span(t, at, at + 0.3);
        if (a <= 0) return;
        var x = K.x, y = K.y + i * K.step;
        if (L.narrow) {
          if (i < 4) { x = K.x + (i % 2) * 168; y = K.y + Math.floor(i / 2) * K.step; }
          else { y = K.y + 2 * K.step; }
        }
        U.check(ctx, x + 6, y - 4, 11, { alpha: a });
        formula(ctx, ck[0], x + 20, y, L.narrow ? 12 : 13, { alpha: a, color: C.ink });
        if (K.two) U.text(ctx, ck[1], x + 20, y + 17, { size: 11, color: C.ink3, alpha: a });
      });

      /* settlement */
      var st = U.span(t, 10.6, 11.2);
      var story = 'agent-1 receives 5 GPU-hours and pays 9.75 USD; lab-a is paid 9.75 USD; agent-2 and lab-b are refunded. Welfare: 1.50 USD.';
      if (t < 10.6) story = t < 2.8 ? 'Orders arrive sealed. Nobody, the solver included, can read them or choose their order.'
        : (t < 7.4 ? 'The batch is revealed and solved: the bids and offers cross at 5 GPU-hours.'
          : 'The ledger checks the certificate in linear time. A place in the batch is worth nothing.');
      U.line(ctx, L.story.x, L.story.y - 22, L.story.x + L.story.w, L.story.y - 22, { color: C.rule });
      if (L.narrow) U.para(ctx, story, L.story.x, L.story.y, L.story.w, { size: 13, lh: 18, alpha: t < 10.6 ? 1 : st });
      else U.para(ctx, story, L.story.x, L.story.y, L.story.w, { size: 15, lh: 21, alpha: t < 10.6 ? 1 : st });
      ctx.restore();
    }
  });
})();
