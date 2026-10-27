/* x402 on Siontology, as in `sion demo x402`.
 *
 * The agent requests a resource; the API answers 402 Payment Required with a
 * quote (0.25 USD to carol, scheme exact, fresh nonce). The agent signs a
 * payment under its charter whose purpose names the quote and retries with
 * PAYMENT-SIGNATURE. carol's facilitator submits it, waits for execution and
 * verifies the receipt as a light client would (committee certificate,
 * certified order, inclusion proof); only then does the API answer 200 OK.
 * Replaying the same payment is refused: each quote pays for one request.
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;
  var D = 14.5;

  var STEPS = [
    { from: 0, to: 1, label: 'GET /v1/forecast', short: 'GET /v1/forecast' },
    { from: 1, to: 0, label: '402 Payment Required', short: '402, with a quote', sub: 'PAYMENT-REQUIRED: 0.25 USD to carol, scheme exact, fresh nonce' },
    { self: 0, label: 'signs a payment under its charter', short: 'signs a charter payment', sub: 'purpose x402:<nonce>, within 1 USD per payment', subShort: 'purpose x402:<nonce>' },
    { from: 0, to: 1, label: 'GET /v1/forecast with PAYMENT-SIGNATURE', short: 'GET, with the payment' },
    { from: 1, to: 2, label: 'submits the payment', short: 'submits it', sub: 'lane certificate, then a Pulse executes it' },
    { from: 2, to: 1, label: 'receipt and proofs', short: 'receipt, proofs' },
    { self: 1, label: 'verifies it as a light client', short: 'verifies as a light client', sub: 'certificate, certified order, inclusion proof', subShort: 'certificate, order, inclusion' },
    { from: 1, to: 0, label: '200 OK with PAYMENT-RESPONSE', short: '200 OK, and the data', sub: '{"region":"eu-west","gpu_hours":18250,"confidence":0.82}', mono: true },
    { from: 0, to: 1, label: 'the same PAYMENT-SIGNATURE again', short: 'the same payment again', bad: true },
    { from: 1, to: 0, label: '402: unknown quote, or one already used', short: '402: quote already used', bad: true }
  ];

  function layout(narrow) {
    if (!narrow) {
      return { xs: [130, 480, 830], top: 14, boxW: 230, boxH: 44, y0: 98, step: 44, life1: 506,
        names: [['AI agent', 'under an Agent Charter'], ['carol’s API', '0.25 USD per request'], ['Siontology', 'devnet']], fs: 13 };
    }
    return { xs: [44, 180, 316], top: 10, boxW: 84, boxH: 40, y0: 90, step: 49, life1: 566,
      names: [['agent', ''], ['carol’s API', ''], ['Siontology', '']], fs: 11, narrow: true };
  }

  S.scene('x402', {
    duration: D,
    still: 10.7,
    wide: { w: 960, h: 540 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var L = layout(view.narrow);
      ctx.save();
      ctx.globalAlpha = 1 - U.span(t, D - 0.8, D);

      L.xs.forEach(function (x, i) {
        var bw = L.boxW;
        U.rect(ctx, x - bw / 2, L.top, bw, L.boxH, { stroke: C.ink, lw: 1.25 });
        var nm = L.names[i];
        U.text(ctx, nm[0], x, L.top + (nm[1] ? 20 : L.boxH / 2 + 5), { size: L.narrow ? 12 : 14, weight: 600, align: 'center' });
        if (nm[1]) U.text(ctx, nm[1], x, L.top + 37, { size: 11, color: C.ink3, align: 'center' });
        U.line(ctx, x, L.top + L.boxH, x, L.life1, { color: C.rule2, dash: [3, 4] });
      });

      STEPS.forEach(function (st, i) {
        var at = 0.5 + i * 1.0;
        if (t < at) return;
        var y = L.y0 + i * L.step + (L.narrow ? 0 : (i >= 8 ? 4 : 0));
        var p = U.span(t, at, at + 0.55, U.linear);
        var a = U.span(t, at, at + 0.3);
        var color = st.bad ? C.signal : C.ink;
        if (st.self !== undefined) {
          var sx = L.xs[st.self];
          var bw = L.narrow ? 160 : 250, bx = sx + 10;
          if (L.narrow && st.self === 1) bx = sx - bw / 2;
          var bh = L.narrow ? 36 : 34;
          U.rect(ctx, bx, y - 16, bw, bh, { fill: C.bg, alpha: a });
          U.rect(ctx, bx, y - 16, bw, bh, { stroke: C.ink2, lw: 1, alpha: a });
          U.text(ctx, L.narrow ? st.short : st.label, bx + 8, y, { size: L.narrow ? 11 : 12, weight: 600, alpha: a });
          if (st.sub) U.text(ctx, L.narrow ? st.subShort : st.sub, bx + 8, y + 13, { size: L.narrow ? 10 : 11, color: C.ink3, alpha: a });
          return;
        }
        var x1 = L.xs[st.from], x2 = L.xs[st.to];
        var dir = x2 > x1 ? 1 : -1;
        U.arrow(ctx, x1 + dir * 4, y, x2 - dir * 4, y, { color: color, lw: 1.25, p: p, head: 6 });
        if (p < 1) U.packet(ctx, x1, y, x2, y, p, { size: 7, color: color });
        var mid = (x1 + x2) / 2, span = Math.abs(x2 - x1) - 12;
        if (L.narrow) {
          U.text(ctx, st.short, mid, y - 7, { size: 11, weight: 600, color: color, align: 'center', alpha: a });
        } else {
          U.text(ctx, st.label, mid, y - 8, { size: 13, weight: 600, color: color, align: 'center', alpha: a });
          if (st.sub) U.text(ctx, st.sub, mid, y + 15, { size: 11, color: st.bad ? color : C.ink3, align: 'center', alpha: a, mono: st.mono });
        }
      });

      if (!L.narrow) {
        var done = U.span(t, 10.6, 11.0);
        U.text(ctx, 'No accounts and no API keys: every request is paid and proven before it is served.', 40, 528,
          { size: 13, color: C.ink2, alpha: done });
      }
      ctx.restore();
    }
  });
})();
