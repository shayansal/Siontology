/* Two sorts of state.
 *
 * Linear objects (coins, allotments) are consumed at most once: a spend turns
 * its inputs into tombstones, which stay in state (nothing is deleted), and
 * creates new objects; every spend carries a lane certificate. Knowledge is a
 * grow-only log: claims are appended, never conflict and need no ordering;
 * the knowledge root and size grow with each one.
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;
  var D = 16;

  var START = [50, 20, 35, 12, 80, 5];
  /* spends: time, input slots, output slots with amounts */
  var SPENDS = [
    { at: 1.0, ins: [0], outs: [[6, 20], [7, 30]] },
    { at: 3.0, ins: [1, 3], outs: [[8, 32]] },
    { at: 5.0, ins: [4], outs: [[9, 60], [10, 20]] },
    { at: 7.0, ins: [6, 2], outs: [[11, 55]] },
    { at: 9.0, ins: [7], outs: [[12, 10], [13, 20]] },
    { at: 11.0, ins: [9, 5], outs: [[14, 65]] },
    { at: 13.0, ins: [10, 12], outs: [[15, 25], [16, 5]] }
  ];
  var CLAIMS0 = [
    'AI inference receipt, model M',
    'delivery of invoice #1042 confirmed',
    'agent payment receipt',
    'sensor reading, cold chain 4 °C',
    'supplier credential issued',
    'price feed, GPU-hour'
  ];
  var CLAIMS = [
    'output marked as AI-generated',
    'shipment received, witness 2',
    'audit attestation',
    'AI inference receipt, model M',
    'agent payment receipt',
    'proof of delivery, parcel 88',
    'price feed, GPU-hour',
    'media manifest anchored',
    'KYC credential issued',
    'AI inference receipt, model N',
    'sensor reading, cold chain 5 °C',
    'agent payment receipt'
  ];
  var FIRST_INDEX = 31;

  function layout(narrow) {
    if (!narrow) {
      return { lx: 40, ly: 44, gx: 40, gy: 96, cols: 6, cell: 46, gap: 16, story: 372, tally: 400,
        rx: 520, ry: 44, logY: 96, rowH: 28, rows: 9, rootY: 372, divider: [480, 30, 480, 420],
        foot: 470, footR: 520, narrow: false };
    }
    return { lx: 16, ly: 28, gx: 30, gy: 66, cols: 6, cell: 40, gap: 12, story: 232, tally: 252,
      rx: 16, ry: 302, logY: 340, rowH: 24, rows: 7, rootY: 528, divider: [16, 276, 344, 276],
      foot: 562, footR: 16, narrow: true };
  }

  function slotXY(L, i) {
    var c = i % L.cols, r = Math.floor(i / L.cols);
    return [L.gx + c * (L.cell + L.gap), L.gy + r * (L.cell + L.gap)];
  }

  S.scene('two-sorted', {
    duration: D,
    still: 7.95,
    wide: { w: 960, h: 540 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var L = layout(view.narrow);
      ctx.save();
      ctx.globalAlpha = 1 - U.span(t, D - 0.7, D);

      /* titles */
      U.text(ctx, 'Linear objects', L.lx, L.ly, { size: 15, weight: 600 });
      U.text(ctx, 'consumed at most once; tombstoned, never deleted', L.lx, L.ly + 20, { size: 12, color: C.ink3 });
      U.text(ctx, 'Knowledge log', L.rx, L.ry, { size: 15, weight: 600 });
      U.text(ctx, 'claims are appended and never conflict', L.rx, L.ry + 20, { size: 12, color: C.ink3 });
      var d = L.divider;
      U.line(ctx, d[0], d[1], d[2], d[3], { color: C.rule });

      /* replay the spends up to t */
      var slots = [];
      START.forEach(function (a, i) { slots[i] = { amount: a, born: -1, dead: 1e9 }; });
      SPENDS.forEach(function (sp) {
        sp.ins.forEach(function (i) { slots[i].dead = sp.at + 0.8; });
        sp.outs.forEach(function (o) { slots[o[0]] = { amount: o[1], born: sp.at + 0.8, dead: 1e9 }; });
      });
      var total = L.narrow ? 18 : 24;
      var cur = null;
      SPENDS.forEach(function (sp) { if (t >= sp.at && t < sp.at + 1.9) cur = sp; });

      for (var i = 0; i < total; i++) {
        var xy = slotXY(L, i), x = xy[0], y = xy[1], s = L.cell;
        var o = slots[i];
        if (!o || t < o.born) {
          U.rect(ctx, x, y, s, s, { stroke: C.rule, lw: 1 });
          continue;
        }
        var bornA = U.span(t, o.born, o.born + 0.4);
        var deadA = U.span(t, o.dead - 0.4, o.dead);
        if (deadA < 1) {
          var a = (o.born < 0 ? 1 : bornA) * (1 - deadA);
          var inset = (1 - (o.born < 0 ? 1 : bornA)) * s * 0.3;
          U.rect(ctx, x + inset, y + inset, s - 2 * inset, s - 2 * inset, { fill: C.ink, alpha: a });
          U.text(ctx, String(o.amount), x + s / 2, y + s / 2 + 4, { size: 12, weight: 600, color: C.bg, align: 'center', alpha: a });
        }
        if (deadA > 0) {
          U.rect(ctx, x, y, s, s, { stroke: C.ink3, lw: 1.25, dash: [3, 3], alpha: deadA });
          U.text(ctx, String(o.amount), x + s / 2, y + s / 2 + 4, { size: 11, color: C.ink3, align: 'center', alpha: deadA });
        }
        /* inputs of the spend in progress */
        if (cur && cur.ins.indexOf(i) >= 0) {
          var hl = U.win(t, cur.at, cur.at + 1.2, 0.2);
          U.rect(ctx, x - 4, y - 4, s + 8, s + 8, { stroke: C.ink, lw: 1.25, alpha: hl });
        }
      }

      /* the spend in progress: certificate, then execution */
      if (cur) {
        var p = U.win(t, cur.at, cur.at + 1.9, 0.2);
        var ins = cur.ins.length, outs = cur.outs.length;
        U.text(ctx, 'A spend consumes ' + ins + (ins === 1 ? ' object' : ' objects') + ' and creates ' + outs + '.',
          L.lx, L.story, { size: 13, alpha: p });
        var votes = Math.min(3, Math.floor(U.span(t, cur.at + 0.2, cur.at + 0.8, U.linear) * 3.999));
        U.tally(ctx, L.lx, L.tally - 11, votes, { h: 13, gap: 6, alpha: p });
        U.text(ctx, votes >= 3 ? 'lane certificate: 3 of 4 validators' : 'collecting approvals', L.lx + 30, L.tally,
          { size: 12, color: votes >= 3 ? C.ink : C.ink3, alpha: p });
      }

      /* knowledge log */
      var appended = 0;
      CLAIMS.forEach(function (_, j) { if (t >= 0.6 + j * 1.2) appended = j + 1; });
      var all = CLAIMS0.concat(CLAIMS.slice(0, appended));
      var lastAt = appended > 0 ? 0.6 + (appended - 1) * 1.2 : -10;
      var enter = U.span(t, lastAt, lastAt + 0.45);
      var overflow = Math.max(0, all.length - L.rows);
      var scroll = overflow > 0 ? (overflow - 1 + enter) : 0;
      ctx.save();
      ctx.beginPath();
      ctx.rect(L.rx - 2, L.logY - 4, (L.narrow ? 330 : 420), L.rows * L.rowH + 2);
      ctx.clip();
      all.forEach(function (s, j) {
        var y = L.logY + (j - scroll) * L.rowH;
        var isNew = j === all.length - 1 && appended > 0;
        var a = isNew ? enter : 1;
        U.rect(ctx, L.rx, y + 2, 12, 12, { stroke: C.ink, lw: 1.25, alpha: a });
        U.text(ctx, '#' + (FIRST_INDEX + j), L.rx + 22, y + 12, { size: 12, color: C.ink3, alpha: a });
        U.text(ctx, s, L.rx + 62, y + 12, { size: 13, color: isNew ? C.ink : C.ink2, alpha: a });
      });
      ctx.restore();
      var size = FIRST_INDEX + all.length;
      U.text(ctx, 'knowledge root', L.rx, L.rootY, { size: 12, color: C.ink3 });
      U.text(ctx, U.hex(size * 977, 8) + '…', L.rx + 104, L.rootY, { size: 12, mono: true, color: C.ink2 });
      U.text(ctx, 'size ' + size, L.rx + (L.narrow ? 220 : 220), L.rootY, { size: 12, color: C.ink2 });

      /* what each sort needs */
      if (!L.narrow) {
        U.text(ctx, 'Needs ordering, only among contenders: a lane certificate per spend.', L.lx, L.foot, { size: 13, color: C.ink2 });
        U.text(ctx, 'Needs no coordination: availability and a proof check.', L.footR, L.foot, { size: 13, color: C.ink2 });
      } else {
        U.para(ctx, 'Consumption is ordered, only among contenders. Knowledge needs no coordination.', L.lx, L.foot, 328,
          { size: 12, lh: 16, color: C.ink2 });
      }
      ctx.restore();
    }
  });
})();
