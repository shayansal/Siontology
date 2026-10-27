/* Linear Lanes: consensus-free certificates for owned objects.
 *
 * The owner spends version 7 of a coin with T1. Each validator locks
 * (coin, v7) to the first transaction it sees and signs its approvals in
 * batches (one dual signature per batch root). More than two thirds of the
 * committee's stake (3 of 4) forms a certificate: T1 is final. The owner then
 * signs T2 for the same version. V1 and V2 are locked to T1 and refuse; a
 * faulty V3 approves both; V4, which T1 had not reached, approves T2. T2 gets
 * 2 of 4 and never a certificate: any two quorums share an honest validator.
 * V3's two approvals are signed evidence of equivocation, and since
 * specification v1.3 its stake is slashed for them.
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;
  var D = 15;

  var V = [
    { name: 'V1' }, { name: 'V2' }, { name: 'V3', faulty: true }, { name: 'V4', late: true }
  ];

  function layout(narrow) {
    if (!narrow) {
      return {
        t1: { x: 40, y: 92, w: 220, h: 54 },
        t2: { x: 40, y: 300, w: 220, h: 54 },
        coin: { x: 40, y: 196, s: 44 },
        vals: [0, 1, 2, 3].map(function (i) { return { x: 350, y: 52 + i * 100, w: 250, h: 76 }; }),
        bars: [{ x: 670, y: 120, label: 'T1' }, { x: 670, y: 318, label: 'T2' }],
        seg: 52, segGap: 4,
        story: { x: 40, y: 500, w: 880 }
      };
    }
    return {
      t1: { x: 16, y: 12, w: 160, h: 46 },
      t2: { x: 184, y: 12, w: 160, h: 46 },
      coin: { x: 16, y: 72, s: 30 },
      vals: [0, 1, 2, 3].map(function (i) { return { x: 16 + (i % 2) * 168, y: 120 + Math.floor(i / 2) * 92, w: 160, h: 80 }; }),
      bars: [{ x: 16, y: 332, label: 'T1' }, { x: 16, y: 424, label: 'T2' }],
      seg: 48, segGap: 4,
      story: { x: 16, y: 528, w: 328 },
      narrow: true
    };
  }

  /* what happened at validator i by time t */
  function state(i, t) {
    var s = { t1: false, t2: false, refusedT2: false, lockedTo: null };
    if (i < 3 && t >= 1.5) { s.lockedTo = 'T1'; s.t1 = true; }
    if (t >= 5.6) {
      if (i < 2) s.refusedT2 = true;
      if (i === 2) s.t2 = true;
      if (i === 3) { s.t2 = true; s.lockedTo = 'T2'; }
    }
    return s;
  }

  S.scene('lanes', {
    duration: D,
    still: 10.2,
    wide: { w: 960, h: 540 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var L = layout(view.narrow);
      ctx.save();
      ctx.globalAlpha = 1 - U.span(t, D - 0.7, D);

      /* transactions and the coin */
      var T1 = L.t1, T2 = L.t2, K = L.coin;
      U.rect(ctx, T1.x, T1.y, T1.w, T1.h, { stroke: C.ink, lw: 1.25 });
      U.text(ctx, 'T1', T1.x + 10, T1.y + 20, { size: 13, weight: 600 });
      U.text(ctx, L.narrow ? 'spends coin v7' : 'spends coin version 7', T1.x + 10, T1.y + T1.h - 12, { size: 12, color: C.ink2 });
      var t2on = U.span(t, 4.4, 4.9);
      if (t2on > 0) {
        U.rect(ctx, T2.x, T2.y, T2.w, T2.h, { stroke: C.signal, lw: 1.25, alpha: t2on });
        U.text(ctx, L.narrow ? 'T2, same owner' : 'T2, signed by the same owner', T2.x + 10, T2.y + 20, { size: 13, weight: 600, color: C.signal, alpha: t2on });
        U.text(ctx, L.narrow ? 'spends v7 again' : 'spends version 7 again', T2.x + 10, T2.y + T2.h - 12, { size: 12, color: C.signal, alpha: t2on });
      }
      U.rect(ctx, K.x, K.y, K.s, K.s, { fill: C.ink });
      U.text(ctx, 'v7', K.x + K.s / 2, K.y + K.s / 2 + 4, { size: 12, weight: 600, color: C.bg, align: 'center' });
      U.text(ctx, L.narrow ? 'an owned coin' : 'an owned coin, version 7', K.x + K.s + 10, K.y + K.s / 2 + 4, { size: 12, color: C.ink2 });

      /* messages to the validators */
      L.vals.forEach(function (b, i) {
        var tx = T1.x + T1.w, ty = T1.y + T1.h / 2;
        if (L.narrow) { tx = T1.x + T1.w / 2; ty = T1.y + T1.h; }
        if (i < 3) {
          var p = U.span(t, 0.5 + i * 0.12, 1.3 + i * 0.12, U.linear);
          U.packet(ctx, tx, ty, b.x, b.y + b.h / 2, p, { size: 6 });
        } else if (t < 4.0) {
          var pl = U.span(t, 0.6, 4.0, U.linear) * 0.35;
          U.line(ctx, tx, ty, U.lerp(tx, b.x, pl), U.lerp(ty, b.y + b.h / 2, pl), { color: C.ink3, dash: [2, 4] });
        }
        if (t2on > 0) {
          var ux = L.narrow ? T2.x + T2.w / 2 : T2.x + T2.w, uy = L.narrow ? T2.y + T2.h : T2.y + T2.h / 2;
          var q = U.span(t, 4.7 + i * 0.1, 5.5 + i * 0.1, U.linear);
          U.packet(ctx, ux, uy, b.x, b.y + b.h / 2, q, { size: 6, color: C.signal });
        }
      });

      /* validators */
      var bracket = U.win(t, 8.0, 12.6, 0.4);
      L.vals.forEach(function (b, i) {
        var v = V[i], s = state(i, t);
        var hl = bracket > 0 && i < 2;
        U.rect(ctx, b.x, b.y, b.w, b.h, { stroke: hl ? C.ink : C.ink2, lw: hl ? 2 : 1.25, dash: v.faulty ? [4, 3] : null });
        U.text(ctx, v.name + (v.faulty ? ', faulty' : ''), b.x + 10, b.y + 20, { size: 13, weight: 600 });
        var line = '', color = C.ink2;
        if (s.lockedTo) line = 'locked v7 → ' + s.lockedTo;
        if (s.refusedT2) { line = L.narrow ? 'refuses T2' : 'refuses T2: v7 is locked to T1'; color = C.signal; }
        if (v.faulty && s.t2) { line = 'approves T1 and T2'; color = C.signal; }
        if (i === 3 && !s.lockedTo) { line = 'T1 has not arrived'; color = C.ink3; }
        if (line) U.text(ctx, line, b.x + 10, b.y + 42, { size: 12, color: color });
        if (line && L.narrow && s.refusedT2) U.text(ctx, 'v7 is locked to T1', b.x + 10, b.y + 57, { size: 11, color: C.signal });
        /* the batch: approvals collected, then one signature over their root */
        var bx = b.x + 10, by = b.y + b.h - 18, n = L.narrow ? 6 : 8;
        for (var k = 0; k < n; k++) {
          var fillAt = 1.5 + k * 0.12;
          var mine = k === 3;
          var a = U.span(t, fillAt, fillAt + 0.2);
          U.rect(ctx, bx + k * 11, by, 8, 8, { stroke: C.rule2, lw: 1 });
          if (i < 3 && a > 0) U.rect(ctx, bx + k * 11, by, 8, 8, { fill: mine ? C.ink : C.ink3, alpha: a });
          if (i === 3 && mine && t > 5.6) U.rect(ctx, bx + k * 11, by, 8, 8, { fill: C.signal });
          if (i === 2 && k === n - 1 && t > 5.6) U.rect(ctx, bx + k * 11, by, 8, 8, { fill: C.signal });
        }
        var signed = U.span(t, 2.4, 2.7);
        if (i < 3 && signed > 0) {
          U.text(ctx, L.narrow ? 'signed' : 'batch signed once', bx + n * 11 + 6, by + 8, { size: 11, color: C.ink3, alpha: signed });
        }
      });

      /* quorum intersection */
      if (bracket > 0 && !L.narrow) {
        var b0 = L.vals[0], b1 = L.vals[1];
        var x = b0.x - 14;
        U.poly(ctx, [[x + 6, b0.y], [x, b0.y], [x, b1.y + b1.h], [x + 6, b1.y + b1.h]], { color: C.ink, lw: 1.5, alpha: bracket });
      }

      /* tallies against the quorum line */
      L.bars.forEach(function (bar, j) {
        var count = 0;
        if (j === 0) {
          for (var i = 0; i < 3; i++) if (t >= 2.8 + i * 0.35) count++;
          for (var i2 = 0; i2 < 3; i2++) {
            var p = U.span(t, 2.5 + i2 * 0.35, 2.8 + i2 * 0.35, U.linear);
            var b = L.vals[i2];
            if (!L.narrow) U.packet(ctx, b.x + b.w, b.y + b.h / 2, bar.x, bar.y + 14, p, { size: 6 });
          }
        } else {
          if (t >= 6.8) count++;
          if (t >= 7.1) count++;
        }
        var on = j === 0 ? 1 : t2on;
        U.text(ctx, bar.label + ' approvals', bar.x, bar.y - 10, { size: 12, color: C.ink3, alpha: on });
        for (var k = 0; k < 4; k++) {
          var x = bar.x + k * (L.seg + L.segGap);
          U.rect(ctx, x, bar.y, L.seg, 28, { stroke: C.rule2, lw: 1, alpha: on });
          if (k < count) U.rect(ctx, x, bar.y, L.seg, 28, { fill: j === 0 ? C.ink : C.signal, alpha: on });
        }
        /* more than two thirds of the stake: the line sits at 2/3, so 3 of 4 cross it */
        var qx = bar.x + 2 * (L.seg + L.segGap) + L.seg * (2 / 3);
        U.line(ctx, qx, bar.y - 6, qx, bar.y + 34, { color: C.ink, lw: 1.5, alpha: on });
        if (j === 0) {
          U.text(ctx, 'quorum: more than 2/3', bar.x + 4 * L.seg + 3 * L.segGap, bar.y - 10, { size: 11, color: C.ink2, align: 'right' });
        }
        var verdict = '';
        if (j === 0 && count >= 3) verdict = '3 of 4: certificate. T1 is final.';
        if (j === 1 && count >= 2) verdict = '2 of 4: no certificate, ever.';
        if (verdict) U.text(ctx, verdict, bar.x, bar.y + 50, { size: 13, weight: 600, color: j === 0 ? C.ink : C.signal });
      });

      /* narration */
      var story = '';
      if (t < 4.4) story = 'Each validator locks version 7 to the first transaction it sees, and signs its approvals in batches.';
      else if (t < 8.0) story = 'The owner signs a conflicting T2. Locked validators refuse it; even with a faulty V3 it reaches 2 of 4.';
      else story = 'Any two quorums share an honest validator, here V1 and V2, so two certificates for one version cannot exist.';
      if (t >= 10.0) story = 'V3\u2019s two approvals are signed evidence of equivocation: its stake is slashed. T1 stays final, with no consensus round.';
      var St = L.story;
      U.line(ctx, St.x, St.y - 22, St.x + St.w, St.y - 22, { color: C.rule });
      if (L.narrow) U.para(ctx, story, St.x, St.y, St.w, { size: 13, lh: 18 });
      else U.text(ctx, story, St.x, St.y, { size: 15 });
      ctx.restore();
    }
  });
})();
