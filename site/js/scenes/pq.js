/* Two signature families, as in crates/sion-crypto.
 *
 * Every authorization carries an ML-DSA-44 signature (FIPS 204, module
 * lattices) and an SLH-DSA signature (FIPS 205, hash-based); a verifier
 * requires both. Breaking lattices forges only the first, breaking hash
 * functions only the second: forgery needs both families broken, so the
 * trust formula of an authorization is lattice ∨ hash. Sizes are the
 * repository's: ML-DSA-44 2,420 bytes, SLH-DSA-SHA2-128f 17,088 bytes (the
 * default second signature), against 64 for Ed25519, which is not used.
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;
  var D = 16;

  var SCEN = [
    { title: 'An honest signer', short: 'an honest signer', forged: [false, false], verdict: 'accepted' },
    { title: 'An attacker who breaks lattices', short: 'lattices broken', forged: [true, false], verdict: 'refused' },
    { title: 'An attacker who breaks hash functions', short: 'hash functions broken', forged: [false, true], verdict: 'refused' },
    { title: 'An attacker who breaks both families', short: 'both families broken', forged: [true, true], verdict: 'forged' }
  ];
  var SIGS = [
    { name: 'ML-DSA-44', family: 'module lattices, FIPS 204', bytes: 2420 },
    { name: 'SLH-DSA-SHA2-128f', family: 'hash-based, FIPS 205', bytes: 17088 }
  ];

  function layout(narrow) {
    if (!narrow) {
      return { title: [40, 40], digest: { x: 40, y: 86, w: 220, h: 76 },
        sigs: [{ x: 330, y: 70, w: 290, h: 50 }, { x: 330, y: 132, w: 290, h: 50 }],
        and: { x: 680, y: 98, s: 56 }, verdict: { x: 780, y: 96, w: 140, h: 60 },
        formula: { x: 40, y: 240, w: 880 }, bars: { x: 40, lx: 180, y: 322, maxW: 600, gap: 42 },
        story: { x: 40, y: 512, w: 880 } };
    }
    return { title: [16, 24], digest: { x: 16, y: 44, w: 328, h: 50 },
      sigs: [{ x: 16, y: 108, w: 328, h: 46 }, { x: 16, y: 162, w: 328, h: 46 }],
      and: null, verdict: { x: 16, y: 222, w: 328, h: 40 },
      formula: { x: 16, y: 292, w: 328 }, bars: { x: 16, lx: 16, y: 372, maxW: 328, gap: 46, stacked: true },
      story: { x: 16, y: 560, w: 328 }, narrow: true };
  }

  S.scene('pq', {
    duration: D,
    still: 6.2,
    wide: { w: 960, h: 540 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var L = layout(view.narrow);
      var k = Math.min(3, Math.floor(t / 4)), lt = t - k * 4;
      var sc = SCEN[k];
      var inA = U.win(lt, 0, 3.95, 0.25);

      U.text(ctx, 'Scenario ' + (k + 1) + ' of 4: ' + (L.narrow ? sc.short : sc.title), L.title[0], L.title[1],
        { size: L.narrow ? 13 : 15, weight: 600, alpha: inA, color: k > 0 ? (k === 3 ? C.signal : C.ink) : C.ink });

      /* the authorization */
      var Dg = L.digest;
      U.rect(ctx, Dg.x, Dg.y, Dg.w, Dg.h, { stroke: C.ink, lw: 1.25 });
      U.text(ctx, 'authorization', Dg.x + 12, Dg.y + 20, { size: 12, color: C.ink3 });
      U.text(ctx, 'transaction digest', Dg.x + 12, Dg.y + (L.narrow ? 38 : 42), { size: 13, weight: 600 });
      if (!L.narrow) U.text(ctx, '9c4e1f07a2d8…', Dg.x + 12, Dg.y + 62, { size: 12, mono: true, color: C.ink2 });

      /* the two signatures and their checks */
      L.sigs.forEach(function (b, i) {
        var forged = sc.forged[i];
        var genuine = k === 0;
        var arrive = U.span(lt, 0.2 + i * 0.15, 0.7 + i * 0.15);
        var checkA = U.span(lt, 0.9 + i * 0.2, 1.2 + i * 0.2) * inA;
        var passes = genuine || forged;
        if (!L.narrow) {
          U.line(ctx, Dg.x + Dg.w, Dg.y + Dg.h / 2, b.x, b.y + b.h / 2, { color: C.rule2 });
          U.packet(ctx, Dg.x + Dg.w, Dg.y + Dg.h / 2, b.x, b.y + b.h / 2, U.span(lt, 0.1, 0.6, U.linear), { size: 6 });
        }
        var stroke = forged ? C.signal : (passes ? C.ink : C.ink3);
        U.rect(ctx, b.x, b.y, b.w, b.h, { stroke: stroke, lw: 1.25, dash: passes ? null : [4, 3], alpha: Math.max(0.35, arrive) });
        U.text(ctx, SIGS[i].name, b.x + 12, b.y + 20, { size: 13, weight: 600 });
        U.text(ctx, SIGS[i].family, b.x + 12, b.y + b.h - 10, { size: 11, color: C.ink3 });
        var tag = forged ? 'forged' : (passes ? 'genuine' : 'cannot be produced');
        U.text(ctx, tag, b.x + b.w - (L.narrow ? 40 : 12), b.y + b.h - 10, { size: 11, weight: 600, align: 'right',
          color: forged ? C.signal : (passes ? C.ink2 : C.ink3), alpha: arrive * inA });
        var cx = L.narrow ? b.x + b.w - 20 : b.x + b.w + 30, cy = b.y + b.h / 2;
        if (checkA > 0) {
          if (passes) U.check(ctx, cx, cy, 14, { alpha: checkA, color: forged ? C.signal : C.ink });
          else U.cross(ctx, cx, cy, 12, { alpha: checkA });
        }
        if (L.and && checkA > 0) U.line(ctx, cx + 14, cy, L.and.x, L.and.y + L.and.s / 2, { color: C.rule2, alpha: checkA });
      });

      /* both are required */
      var vA = U.span(lt, 1.5, 1.9) * inA;
      var V = L.verdict;
      var vColor = sc.verdict === 'accepted' ? C.ink : C.signal;
      if (L.and) {
        var A = L.and;
        U.rect(ctx, A.x, A.y, A.s, A.s, { stroke: C.ink, lw: 1.25 });
        U.text(ctx, '∧', A.x + A.s / 2, A.y + A.s / 2 + 8, { size: 22, align: 'center' });
        U.text(ctx, 'both required', A.x + A.s / 2, A.y + A.s + 18, { size: 11, color: C.ink3, align: 'center' });
        U.arrow(ctx, A.x + A.s, A.y + A.s / 2, V.x - 4, V.y + V.h / 2, { color: C.ink2, head: 6, p: vA });
      }
      U.rect(ctx, V.x, V.y, V.w, V.h, { stroke: vColor, lw: 1.5, alpha: vA });
      var vText = sc.verdict === 'accepted' ? 'accepted' : (sc.verdict === 'refused' ? 'refused' : 'forgery accepted');
      if (L.narrow) vText = 'both required: ' + vText;
      U.text(ctx, vText, V.x + V.w / 2, V.y + V.h / 2 + 5, { size: 14, weight: 600, align: 'center', color: vColor, alpha: vA });

      /* the trust formula */
      var F = L.formula;
      var ftext = 'Valid only if both signatures verify. A forger must break both families, so the trust formula of every authorization is lattice ∨ hash.';
      U.para(ctx, ftext, F.x, F.y, F.w, { size: L.narrow ? 12 : 14, lh: L.narrow ? 17 : 20, color: C.ink2 });

      /* the price in bytes */
      var B = L.bars;
      U.text(ctx, 'Signature bytes per authorization', B.x, B.y - 18, { size: 12, color: C.ink3 });
      var rows = [
        { name: 'ML-DSA-44', bytes: 2420, color: C.ink },
        { name: 'SLH-DSA-SHA2-128f', bytes: 17088, color: C.ink },
        { name: 'Ed25519', bytes: 64, color: C.ink3, note: ', not post-quantum and not used' }
      ];
      rows.forEach(function (r, i) {
        var y = B.y + i * B.gap;
        var w = Math.max(2, (r.bytes / 17088) * B.maxW);
        if (B.stacked) {
          U.text(ctx, r.name, B.x, y, { size: 11, color: C.ink2 });
          U.rect(ctx, B.x, y + 6, w, 10, { fill: r.color, alpha: i === 2 ? 0.6 : 1 });
          U.text(ctx, U.fmt(r.bytes) + ' bytes' + (r.note || ''), Math.min(B.x + w + 6, B.x + B.maxW - 70), y + (w > B.maxW - 80 ? 30 : 15), { size: 11, color: C.ink2 });
        } else {
          U.text(ctx, r.name, B.x, y + 12, { size: 12, color: C.ink2, max: B.lx - B.x - 12 });
          U.rect(ctx, B.lx, y, w, 16, { fill: r.color, alpha: i === 2 ? 0.6 : 1 });
          U.text(ctx, U.fmt(r.bytes) + ' bytes' + (r.note || ''), B.lx + w + 8, y + 12, { size: 12, weight: 600, color: C.ink2 });
        }
      });
      var note = 'SLH-DSA-SHA2-128s, 7,856 bytes, is available for accounts that sign rarely. Validators sign one Merkle root per batch of approvals, not each transaction.';
      U.para(ctx, note, B.x, B.y + 3 * B.gap + (B.stacked ? 8 : 4), L.narrow ? 328 : 880, { size: 12, lh: 17, color: C.ink3 });
    }
  });
})();
