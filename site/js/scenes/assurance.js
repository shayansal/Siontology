/* The Assurance Algebra: the worked example from the architecture paper.
 *
 * Invoice #1042 payable = delivery ∧ arithmetic ∧ supplier identity.
 *   delivery   = W1 ∨ W2 ∨ W3, each witness bonded at 20,000 USD   -> 60,000 (sum)
 *   arithmetic = (stack A ∧ circuit A) ∨ (stack B ∧ circuit B)      -> ∞ (trusted)
 *   identity   = a bank's 1,000,000 USD bond
 * CoC = min(60,000, ∞, 1,000,000) = 60,000. With V = 25,000 and κ = 2 the
 * agent needs 50,000: accept, and reserve 50,000 down the formula (each ∧
 * input carries it all; the ∨ of witnesses shares it in proportion to
 * capacity; trusted routes absorb it). A second, equal reliance is refused:
 * the delivery route has 10,000 unreserved.
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;

  var D = 20;

  function wideLayout() {
    return {
      root: { x: 350, y: 34, w: 260, h: 46 },
      branches: [
        { x: 60, y: 150, w: 230, h: 42, label: 'Delivery confirmed', op: '∨', value: '60,000', vlabel: 'sum' },
        { x: 365, y: 150, w: 230, h: 42, label: 'Arithmetic proven', op: '∨', value: '∞', vlabel: 'trusted' },
        { x: 670, y: 150, w: 230, h: 42, label: 'Supplier identity', op: '', value: '1,000,000', vlabel: '' }
      ],
      atoms: [
        { b: 0, x: 60, y: 262, w: 70, h: 54, label: 'W1', cost: '20,000', bond: 20000 },
        { b: 0, x: 140, y: 262, w: 70, h: 54, label: 'W2', cost: '20,000', bond: 20000 },
        { b: 0, x: 220, y: 262, w: 70, h: 54, label: 'W3', cost: '20,000', bond: 20000 },
        { b: 1, x: 365, y: 262, w: 110, h: 54, label: 'stack A ∧ circuit A', cost: '∞', small: true },
        { b: 1, x: 485, y: 262, w: 110, h: 54, label: 'stack B ∧ circuit B', cost: '∞', small: true },
        { b: 2, x: 670, y: 262, w: 230, h: 54, label: 'bank, bonded', cost: '1,000,000', bond: 1000000 }
      ],
      panel: { x: 60, y: 404, w: 840 }
    };
  }

  function narrowLayout() {
    return {
      root: { x: 16, y: 14, w: 328, h: 44 },
      branches: [
        { x: 40, y: 92, w: 304, h: 34, label: 'Delivery confirmed', op: '∨', value: '60,000', vlabel: 'sum' },
        { x: 40, y: 232, w: 304, h: 34, label: 'Arithmetic proven', op: '∨', value: '∞', vlabel: 'trusted' },
        { x: 40, y: 330, w: 304, h: 34, label: 'Supplier identity', op: '', value: '1,000,000', vlabel: '' }
      ],
      atoms: [
        { b: 0, x: 40, y: 136, w: 96, h: 46, label: 'W1', cost: '20,000', bond: 20000 },
        { b: 0, x: 144, y: 136, w: 96, h: 46, label: 'W2', cost: '20,000', bond: 20000 },
        { b: 0, x: 248, y: 136, w: 96, h: 46, label: 'W3', cost: '20,000', bond: 20000 },
        { b: 1, x: 40, y: 276, w: 148, h: 40, label: 'stack A ∧ circuit A', cost: '∞', small: true },
        { b: 1, x: 196, y: 276, w: 148, h: 40, label: 'stack B ∧ circuit B', cost: '∞', small: true },
        { b: 2, x: 40, y: 374, w: 304, h: 40, label: 'bank, bonded', cost: '1,000,000', bond: 1000000 }
      ],
      panel: { x: 20, y: 462, w: 320 },
      narrow: true
    };
  }

  function elbow(ctx, x1, y1, x2, y2, o) {
    var my = (y1 + y2) / 2;
    U.poly(ctx, [[x1, y1], [x1, my], [x2, my], [x2, y2]], o);
  }

  S.scene('assurance', {
    duration: D,
    still: 11.6,
    wide: { w: 960, h: 540 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var L = view.narrow ? narrowLayout() : wideLayout();
      var fadeAll = 1 - U.span(t, D - 0.8, D);
      ctx.save();
      ctx.globalAlpha = fadeAll;

      var appear = function (i) { return U.span(t, 0.1 + i * 0.12, 0.6 + i * 0.12); };
      var evalB = function (b) { return U.span(t, 3.4 + b * 0.45, 3.9 + b * 0.45); };
      var evalRoot = U.span(t, 5.0, 5.5);
      var decide = U.span(t, 5.9, 6.4);
      var reserve = U.span(t, 7.2, 9.2);
      var second = U.span(t, 10.0, 10.5);
      var r = L.root;
      var rx = r.x + r.w / 2;

      /* edges */
      L.branches.forEach(function (b, bi) {
        var a = appear(1 + bi);
        var weak = bi === 0 ? evalRoot : 0;
        if (L.narrow) {
          var sx = 26;
          U.poly(ctx, [[sx, r.y + r.h], [sx, b.y + b.h / 2], [b.x, b.y + b.h / 2]],
            { color: weak ? C.ink : C.rule2, lw: weak ? 2 : 1, alpha: a });
        } else {
          elbow(ctx, rx, r.y + r.h, b.x + b.w / 2, b.y, { color: weak ? C.ink : C.rule2, lw: 1 + weak, alpha: a });
        }
      });
      L.atoms.forEach(function (at, i) {
        var b = L.branches[at.b];
        var a = appear(4 + i * 0.5);
        if (!L.narrow) elbow(ctx, b.x + b.w / 2, b.y + b.h, at.x + at.w / 2, at.y, { color: C.rule2, alpha: a });
      });

      /* root */
      var ra = appear(0);
      U.rect(ctx, r.x, r.y, r.w, r.h, { stroke: C.ink, lw: 1.5, alpha: ra });
      U.text(ctx, 'Invoice #1042 payable', r.x + 14, r.y + r.h / 2 + 5, { size: 14, weight: 600, alpha: ra });
      U.text(ctx, '∧', r.x + r.w - 16, r.y + r.h / 2 + 6, { size: 18, align: 'right', alpha: ra });
      if (evalRoot > 0) {
        U.text(ctx, 'min = 60,000', L.narrow ? r.x + r.w : r.x + r.w + 16, L.narrow ? r.y + r.h + 18 : r.y + r.h / 2 + 5,
          { size: 13, weight: 600, align: L.narrow ? 'right' : 'left', alpha: evalRoot * ra });
      }

      /* branches */
      L.branches.forEach(function (b, bi) {
        var a = appear(1 + bi);
        var refused = bi === 0 ? second * (1 - U.span(t, 12.8, 13.6) * 0.6) : 0;
        U.rect(ctx, b.x, b.y, b.w, b.h, { stroke: refused > 0.5 ? C.signal : C.ink, lw: 1.25, alpha: a });
        U.text(ctx, b.label, b.x + 12, b.y + b.h / 2 + 5, { size: 13, weight: 600, alpha: a });
        if (b.op) U.text(ctx, b.op, b.x + b.w - 12, b.y + b.h / 2 + 6, { size: 16, align: 'right', alpha: a });
        var e = evalB(bi);
        if (e > 0) {
          var vx = b.x + b.w - (b.op ? 34 : 12);
          var vy = b.y + b.h / 2 + 5;
          var label = (b.vlabel === 'sum' ? 'Σ ' : '') + b.value;
          U.text(ctx, label, vx, vy, { size: 13, weight: 600, align: 'right', alpha: e * a, color: C.ink });
        }
      });

      /* atoms, with their capacity bars */
      L.atoms.forEach(function (at, i) {
        var a = appear(4 + i * 0.5);
        var cost = U.span(t, 1.4 + i * 0.25, 1.8 + i * 0.25);
        U.rect(ctx, at.x, at.y, at.w, at.h, { stroke: C.ink, lw: 1, alpha: a, dash: at.bond ? null : [3, 3] });
        U.text(ctx, at.label, at.x + 8, at.y + 18, { size: at.small ? 11 : 12, weight: 600, alpha: a, max: at.w - 14 });
        if (cost > 0) {
          var absorbed = !at.bond && reserve > 0.95 ? U.span(t, 9.0, 9.4) : 0;
          U.text(ctx, at.cost, at.x + 8, at.y + at.h - 10, { size: at.small ? 13 : 12, alpha: cost * a, color: C.ink2 });
          if (absorbed > 0) U.text(ctx, 'absorbed', at.x + 26, at.y + at.h - 10, { size: 11, color: C.ink3, alpha: absorbed * a });
        }
        if (at.bond) {
          var by = at.y + at.h + 8;
          U.rect(ctx, at.x, by, at.w, 6, { stroke: C.rule2, lw: 1, alpha: a });
          var reserved = at.b === 0 ? 50000 / 3 : 50000;
          var frac = (reserved / at.bond) * reserve;
          U.rect(ctx, at.x, by, at.w * frac, 6, { fill: C.ink, alpha: a });
          var first = at.b === 2 || at.label === 'W1';
          if (reserve > 0.95 && first) {
            U.text(ctx, at.b === 0 ? '16,667 reserved on each witness' : '50,000 reserved', at.x, by + 20,
              { size: 11, color: C.ink3, alpha: U.span(t, 9.0, 9.4) });
          }
        }
      });

      /* reservation flowing down the formula */
      if (reserve > 0 && reserve < 1) {
        L.atoms.forEach(function (at) {
          var b = L.branches[at.b];
          var p1 = Math.min(1, reserve * 2), p2 = Math.max(0, reserve * 2 - 1);
          if (L.narrow) {
            U.packet(ctx, 26, r.y + r.h, 26, b.y + b.h / 2, p1, { size: 5 });
            U.packet(ctx, at.x + at.w / 2, b.y + b.h, at.x + at.w / 2, at.y, p2, { size: 5 });
          } else {
            U.packet(ctx, rx, r.y + r.h, b.x + b.w / 2, b.y, p1, { size: 5 });
            U.packet(ctx, b.x + b.w / 2, b.y + b.h, at.x + at.w / 2, at.y, p2, { size: 5 });
          }
        });
      }

      /* the decision */
      var P = L.panel, lh = 22, y = P.y;
      U.line(ctx, P.x, y - 14, P.x + P.w, y - 14, { color: C.rule, alpha: decide });
      var s1 = 'Financier 1 relies on V = 25,000 USD at κ = 2. It needs κ·V = 50,000; the cost of corruption is 60,000.';
      var s2 = 'Accept, and reserve 50,000 of the backing: every ∧ input carries all of it, the witnesses share it.';
      var s3 = 'Financier 2 asks the same. The delivery route has 10,000 unreserved: refused.';
      if (L.narrow) {
        var used = U.para(ctx, s1, P.x, y + 4, P.w, { size: 12, lh: 17, alpha: decide });
        used += U.para(ctx, s2, P.x, y + 10 + used, P.w, { size: 12, lh: 17, color: C.ink2, alpha: U.span(t, 7.0, 7.4) });
        U.para(ctx, s3, P.x, y + 18 + used, P.w, { size: 12, lh: 17, color: C.signal, weight: 600, alpha: second });
      } else {
        U.text(ctx, s1, P.x, y + 4, { size: 14, alpha: decide });
        U.text(ctx, s2, P.x, y + 4 + lh, { size: 14, color: C.ink2, alpha: U.span(t, 7.0, 7.4) });
        U.text(ctx, s3, P.x, y + 4 + 2 * lh + 10, { size: 14, weight: 600, color: C.signal, alpha: second });
        U.text(ctx, 'min(δ·CoC, CoC of the adjudicator) ≥ κ·V', P.x + P.w, P.y - 24,
          { size: 12, mono: true, color: C.ink3, align: 'right', alpha: decide });
      }
      ctx.restore();
    }
  });
})();
