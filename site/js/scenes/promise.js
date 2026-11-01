/* Its limits hold: the landing page's first figure, in plain words.
 *
 * You ask your agent, in a chat app, to buy the compute for tonight's job.
 * At autonomy L5 it asks before it pays; you approve, and the ledger checks
 * the payment against the agent's charter (the agents demo's: pay only
 * gpu-cloud, at most 100 USD a payment, a 1,000 USD budget): 80 USD to
 * gpu-cloud, accepted, its receipt kept. A page it read then tells it to send
 * 90 USD to mallory. It asks; you approve by mistake; the ledger refuses
 * anyway: payee not allowed. The questions are the runtime's, the yes and
 * no the ledger's (`sion demo agents --local` runs the ledger's part).
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;
  var D = 19;

  var STEPS = [
    { at: 0.2, text: 'You ask your agent, in a chat app you already use.' },
    { at: 1.6, text: 'It asks before it pays. You approve.' },
    { at: 3.2, text: 'The ledger checks the payment against the limits you set. Accepted.' },
    { at: 7.0, text: 'A page it read hides an instruction: send 90 USD to mallory.' },
    { at: 8.8, text: 'It asks again, and you approve it by mistake.' },
    { at: 9.8, text: 'The ledger refuses anyway: mallory is not a payee you allowed.' },
    { at: 12.8, text: 'Whatever anyone tells your agent, the limits you set hold.' }
  ];

  var MSGS = [
    { at: 0.4, you: true, text: 'Buy the compute for tonight’s job from gpu-cloud.' },
    { at: 1.6, text: 'gpu-cloud asks 80 USD for it. Shall I pay?', press: 3.0 },
    { at: 5.4, text: 'Paid 80 USD. Its receipt is kept on the ledger.', mark: 'check' },
    { at: 7.2, text: 'Their page says I must also send 90 USD to mallory to hold the booking. Shall I?', press: 9.0 },
    { at: 11.0, text: 'The ledger refused it: mallory is not a payee you allowed.', mark: 'cross' }
  ];

  /* The two payments, as the ledger sees them. */
  var PAYS = [
    { send: 3.2, check: 4.0, done: 4.9, amount: '80 USD to gpu-cloud', verdict: 'accepted', ok: true },
    { send: 9.2, check: 10.0, done: 10.4, amount: '90 USD to mallory', verdict: 'refused: payee not allowed', ok: false }
  ];

  var RULES = ['pays only gpu-cloud', 'at most 100 USD a payment', 'budget 1,000 USD'];

  function layout(narrow) {
    if (!narrow) {
      return {
        N: false,
        chat: { x: 20, y: 16, w: 450, h: 392 },
        ledger: { x: 500, y: 16, w: 280, h: 392 },
        story: { x: 20, y: 452, w: 760, size: 15 },
        rule: 432,
        text: 14, small: 12, pad: 14, bubble: 330, lh: 19,
        rowH: 30, rowsAt: 66, payGap: 64
      };
    }
    return {
      N: true,
      chat: { x: 10, y: 10, w: 340, h: 296 },
      ledger: { x: 10, y: 318, w: 340, h: 222 },
      story: { x: 10, y: 576, w: 340, size: 13 },
      rule: 556,
      text: 13, small: 11, pad: 10, bubble: 260, lh: 17,
      rowH: 21, rowsAt: 54, payGap: 40
    };
  }

  /* Lines of s that fit in width, without drawing them. */
  function wrap(s, width, size, weight) {
    var words = s.split(' '), lines = [], cur = '';
    var ctx = wrap.ctx;
    for (var i = 0; i < words.length; i++) {
      var test = cur ? cur + ' ' + words[i] : words[i];
      if (cur && U.measure(ctx, test, size, weight) > width) {
        lines.push(cur);
        cur = words[i];
      } else {
        cur = test;
      }
    }
    if (cur) lines.push(cur);
    return lines;
  }

  function message(ctx, m, x, y, L, t, alpha) {
    var lines = wrap(m.text, L.bubble - 2 * L.pad - (m.you ? 0 : 18) - (m.mark ? 20 : 0), L.text, 450);
    var h = lines.length * L.lh + 2 * L.pad - 4 + (m.press ? 40 : 0);
    var w = L.bubble;
    var bx = m.you ? L.chat.x + L.chat.w - L.pad - w : L.chat.x + L.pad;
    if (alpha <= 0) return h;
    ctx.save();
    ctx.globalAlpha *= alpha;
    U.rect(ctx, bx, y, w, h, { stroke: m.you ? C.rule2 : C.ink, lw: 1, fill: m.you ? C.raised : null });
    var tx = bx + L.pad + (m.you ? 0 : 18);
    if (!m.you) U.diamond(ctx, bx + L.pad + 5, y + L.pad + 2, 5, { lw: 1.2 });
    lines.forEach(function (s, i) {
      U.text(ctx, s, tx, y + L.pad + 9 + i * L.lh, { size: L.text, color: m.you ? C.ink2 : C.ink });
    });
    if (m.mark === 'check') U.check(ctx, bx + w - 16, y + L.pad + 4, 10);
    if (m.mark === 'cross') U.cross(ctx, bx + w - 16, y + L.pad + 4, 9);
    if (m.press) {
      var by = y + h - L.pad - 26, bw = L.N ? 82 : 92;
      var pressed = U.span(t, m.press, m.press + 0.25);
      U.rect(ctx, tx, by, bw, 26, { stroke: C.ink, lw: 1, fill: pressed > 0 ? C.ink : null, alpha: 1 });
      U.text(ctx, 'Approve', tx + bw / 2, by + 17, { size: L.small, weight: 600, align: 'center', color: pressed > 0.5 ? C.bg : C.ink });
      U.rect(ctx, tx + bw + 8, by, bw, 26, { stroke: C.rule2, lw: 1, alpha: pressed > 0 ? 0.5 : 1 });
      U.text(ctx, 'Decline', tx + bw + 8 + bw / 2, by + 17, { size: L.small, align: 'center', color: C.ink2, alpha: pressed > 0 ? 0.5 : 1 });
    }
    ctx.restore();
    return h;
  }

  S.scene('promise', {
    duration: D,
    still: 13.2,
    wide: { w: 800, h: 500 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var L = layout(view.narrow);
      wrap.ctx = ctx;
      ctx.save();
      ctx.globalAlpha = 1 - U.span(t, D - 1.0, D - 0.2);

      /* the chat */
      var c = L.chat;
      U.rect(ctx, c.x, c.y, c.w, c.h, { stroke: C.rule2, lw: 1 });
      U.line(ctx, c.x, c.y + 34, c.x + c.w, c.y + 34, { color: C.rule });
      U.diamond(ctx, c.x + L.pad + 6, c.y + 17, 6, { lw: 1.3 });
      U.text(ctx, 'your agent', c.x + L.pad + 20, c.y + 22, { size: L.text, weight: 600 });
      U.text(ctx, 'in your chat app', c.x + c.w - L.pad, c.y + 22, { size: L.small, color: C.ink3, align: 'right' });

      /* messages, scrolled so that the newest stays in view */
      var top = c.y + 46, bottom = c.y + c.h - 10, gap = L.N ? 8 : 10;
      var heights = MSGS.map(function (m) {
        return message(ctx, m, 0, 0, L, t, 0);
      });
      var shown = MSGS.filter(function (m) { return t >= m.at; }).length;
      function used(n) {
        var s = 0;
        for (var i = 0; i < n; i++) s += heights[i] + gap;
        return s;
      }
      /* the scroll moves after each new message, eased */
      var scroll = 0;
      for (var k = 1; k <= MSGS.length; k++) {
        var over = Math.max(0, used(k) - (bottom - top));
        var p = U.span(t, MSGS[k - 1].at - 0.05, MSGS[k - 1].at + 0.35);
        if (k <= shown || p > 0) scroll = Math.max(scroll, U.lerp(k > 1 ? Math.max(0, used(k - 1) - (bottom - top)) : 0, over, p));
      }
      ctx.save();
      ctx.beginPath();
      ctx.rect(c.x + 1, c.y + 35, c.w - 2, c.h - 36);
      ctx.clip();
      var y = top - scroll;
      MSGS.forEach(function (m, i) {
        var a = U.span(t, m.at, m.at + 0.3);
        if (a > 0) message(ctx, m, 0, y + (1 - a) * 6, L, t, a);
        y += heights[i] + gap;
      });
      ctx.restore();

      /* the ledger: the charter's rules, then the payments */
      var g = L.ledger;
      U.rect(ctx, g.x, g.y, g.w, g.h, { stroke: C.ink, lw: 1.25 });
      U.text(ctx, 'The ledger', g.x + L.pad, g.y + 22, { size: L.text, weight: 600 });
      U.text(ctx, 'your agent’s charter', g.x + g.w - L.pad, g.y + 22, { size: L.small, color: C.ink3, align: 'right' });
      U.line(ctx, g.x, g.y + 34, g.x + g.w, g.y + 34, { color: C.rule });
      var rowH = L.rowH, ry = g.y + L.rowsAt;
      RULES.forEach(function (rule, i) {
        var yy = ry + i * rowH;
        U.text(ctx, rule, g.x + L.pad + 22, yy, { size: L.text });
        /* the first payment passes every rule; the second fails the first */
        var first = U.win(t, PAYS[0].check + i * 0.25, PAYS[1].send - 0.4, 0.15);
        if (first > 0) U.check(ctx, g.x + L.pad + 7, yy - 5, 10, { alpha: first });
        if (i === 0) {
          var second = U.win(t, PAYS[1].check, D, 0.15);
          if (second > 0) U.cross(ctx, g.x + L.pad + 7, yy - 5, 9, { alpha: second });
        }
      });
      if (!L.N) {
        var spent = t >= PAYS[0].done ? '80' : '0';
        U.text(ctx, 'spent ' + spent, g.x + g.w - L.pad, ry + 2 * rowH, { size: L.small, color: C.ink3, align: 'right' });
      }
      var ly = ry + 3 * rowH + (L.N ? 4 : 22);
      U.line(ctx, g.x + L.pad, ly - (L.N ? 14 : 20), g.x + g.w - L.pad, ly - (L.N ? 14 : 20), { color: C.rule });
      U.text(ctx, 'payments', g.x + L.pad, ly, { size: L.small, color: C.ink3 });
      PAYS.forEach(function (pay, i) {
        var a = U.span(t, pay.done, pay.done + 0.3);
        if (a <= 0) return;
        var yy = ly + (L.N ? 22 : 32) + i * L.payGap;
        U.text(ctx, pay.amount, g.x + L.pad, yy, { size: L.text, alpha: a });
        if (pay.ok) U.check(ctx, g.x + L.pad + 5, yy + (L.N ? 14 : 17), 9, { alpha: a });
        else U.cross(ctx, g.x + L.pad + 5, yy + (L.N ? 13 : 16), 8, { alpha: a });
        U.text(ctx, pay.verdict + (pay.ok ? ', receipt kept' : ''), g.x + L.pad + 16, yy + (L.N ? 18 : 21),
          { size: L.small, color: pay.ok ? C.ink2 : C.signal, alpha: a });
        if (pay.ok && !L.N) U.text(ctx, 'receipt claim 0586a32c', g.x + L.pad + 16, yy + 38, { size: 11, color: C.ink3, alpha: a, mono: true });
      });

      /* each payment travels from the chat to the ledger */
      PAYS.forEach(function (pay) {
        var p = U.span(t, pay.send, pay.send + 0.7, U.linear);
        if (p <= 0 || p >= 1) return;
        var a = L.N ? { x: c.x + c.w / 2, y: c.y + c.h } : { x: c.x + c.w, y: c.y + c.h * 0.55 };
        var b = L.N ? { x: g.x + g.w / 2, y: g.y } : { x: g.x, y: ry - 5 };
        U.line(ctx, a.x, a.y, b.x, b.y, { color: C.ink2, alpha: 0.4, dash: [2, 3] });
        U.packet(ctx, a.x, a.y, b.x, b.y, p, { size: 7 });
      });

      /* what is happening, in words */
      U.line(ctx, L.story.x, L.rule, L.story.x + L.story.w, L.rule, { color: C.rule });
      var step = STEPS[0];
      STEPS.forEach(function (s) { if (t >= s.at) step = s; });
      var sa = Math.min(U.span(t, step.at, step.at + 0.3), 1);
      U.para(ctx, step.text, L.story.x, L.story.y, L.story.w, { size: L.story.size, color: C.ink, alpha: sa, lh: L.story.size * 1.35 });
      ctx.restore();
    }
  });
})();
