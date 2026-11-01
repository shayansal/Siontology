/* Your team: the landing page's first figure, an illustration of a graph
 * of agents (agent/graphs.md).
 *
 * You give your lead agent, an artist manager, a job in plain words. It
 * splits the job and hands each part to an agent of its team: the CMO
 * drafts the launch posts, the CTO builds the pre-save page, the CFO books
 * the studio. In a graph each is a node with instructions of its own (and,
 * if the graph says so, its own tools or model); they run side by side,
 * each in a session of its own, pass their answers on, and the lead puts
 * them together. The roles and the job are examples.
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;
  var D = 17;

  var STEPS = [
    { at: 0.2, text: 'You give your lead agent a job, in plain words.' },
    { at: 2.3, text: 'It splits the job and hands each part to an agent of its team.' },
    { at: 3.8, text: 'They work side by side, each in a conversation of its own.' },
    { at: 7.4, text: 'Each passes its answer on when its part is done.' },
    { at: 9.6, text: 'Your lead puts it together and answers you.' },
    { at: 12.0, text: 'Build a team like this: a role for each part, with its own tools or model if you like.' }
  ];

  var TEAM = [
    { role: 'CMO', task: 'launch posts', work: 'drafting the launch posts', short: 'drafting posts', done: '3 posts drafted', doneShort: '3 posts drafted', at: 7.4,
      eyes: 'square', mouth: 'smile', off: 0.7 },
    { role: 'CTO', task: 'pre-save page', work: 'building the pre-save page', short: 'building the page', done: 'page built, tests pass', doneShort: 'page built', at: 8.2,
      eyes: 'goggles', mouth: 'flat', off: 2.1 },
    { role: 'CFO', task: 'studio booking', work: 'booking the studio', short: 'booking studio', done: 'studio booked, in budget', doneShort: 'studio booked', at: 9.0,
      eyes: 'square', mouth: 'flat', off: 3.4 }
  ];

  function layout(narrow) {
    if (!narrow) {
      return {
        N: false,
        msg: { x: 20, y: 18, w: 380, h: 62 },
        reply: { x: 20, y: 334, w: 380, h: 78 },
        lead: { x: 200, y: 214, r: 44 },
        mates: TEAM.map(function (m, i) { return { x: 560, y: 92 + i * 128, r: 32 }; }),
        rule: 432, story: { x: 20, y: 458, w: 760, size: 15 },
        text: 14, small: 12
      };
    }
    return {
      N: true,
      msg: { x: 10, y: 10, w: 340, h: 60 },
      reply: { x: 10, y: 396, w: 340, h: 76 },
      lead: { x: 180, y: 150, r: 32 },
      mates: TEAM.map(function (m, i) { return { x: 60 + i * 120, y: 300, r: 24 }; }),
      rule: 500, story: { x: 10, y: 524, w: 340, size: 13 },
      text: 13, small: 11
    };
  }

  function bubble(ctx, b, who, text, L, alpha, o) {
    if (alpha <= 0) return;
    o = o || {};
    U.rect(ctx, b.x, b.y, b.w, b.h, { stroke: o.lead ? C.ink : C.rule2, fill: o.lead ? null : C.raised, lw: 1, alpha: alpha });
    U.text(ctx, who, b.x + 12, b.y + 20, { size: L.small, weight: 600, color: o.lead ? C.ink : C.ink2, alpha: alpha });
    U.para(ctx, text, b.x + 12, b.y + 39, b.w - 24, { size: L.text, alpha: alpha, lh: L.text * 1.3, color: C.ink });
  }

  S.scene('team', {
    duration: D,
    still: 12.4,
    wide: { w: 800, h: 500 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var L = layout(view.narrow), N = L.N;
      ctx.save();
      ctx.globalAlpha = 1 - U.span(t, D - 1.0, D - 0.2);

      /* your message, and later the lead's answer */
      bubble(ctx, L.msg, 'you', 'We release the single on Friday. Get everything ready.', L, U.span(t, 0.3, 0.6));
      bubble(ctx, L.reply, 'your lead agent', 'All set for Friday: posts drafted, page built, studio booked.', L, U.span(t, 9.6, 9.9), { lead: true });

      /* the lead */
      var A = L.lead;
      var reading = U.win(t, 0.8, 2.2, 0.3);
      var talking = Math.max(U.win(t, 2.3, 3.4, 0.1), U.win(t, 9.6, 10.6, 0.1));
      var done = t > 9.4;
      U.face(ctx, A.x, A.y, A.r, {
        lw: 1.7, look: N ? 0 : -reading * 0.6, blink: U.blink(t, 4.3, 0.4),
        mouth: done ? 'smile' : 'flat', talk: talking > 0 ? (Math.sin(t * 18) * 0.5 + 0.5) * talking : 0,
        think: U.win(t, 1.0, 2.3, 0.15), thinkT: t
      });
      if (N) {
        U.text(ctx, 'Artist manager', A.x + A.r + 14, A.y - 4, { size: 13, weight: 600 });
        U.text(ctx, 'your lead agent', A.x + A.r + 14, A.y + 12, { size: 11, color: C.ink3 });
      } else {
        U.text(ctx, 'Artist manager', A.x, A.y + A.r + 22, { size: 14, weight: 600, align: 'center' });
        U.text(ctx, 'your lead agent', A.x, A.y + A.r + 39, { size: 12, color: C.ink3, align: 'center' });
      }

      /* the team */
      TEAM.forEach(function (m, i) {
        var P = L.mates[i];
        var from = N ? { x: A.x, y: A.y + A.r + 4 } : { x: A.x + A.r + 4, y: A.y };
        var to = N ? { x: P.x, y: P.y - P.r - 4 } : { x: P.x - P.r - 4, y: P.y };
        var link = U.span(t, 2.6, 3.2);
        if (link > 0) U.line(ctx, from.x, from.y, U.lerp(from.x, to.x, link), U.lerp(from.y, to.y, link), { color: C.rule2 });
        var out = U.span(t, 3.0 + i * 0.15, 3.7 + i * 0.15, U.linear);
        if (out > 0 && out < 1) U.packet(ctx, from.x, from.y, to.x, to.y, out, { size: 7 });
        if (!N && out > 0) {
          U.text(ctx, m.task, U.lerp(from.x, to.x, 0.55), U.lerp(from.y, to.y, 0.55) - 8,
            { size: 11, color: C.ink2, align: 'center', alpha: U.win(t, 3.0, 4.4, 0.2) });
        }
        var back = U.span(t, m.at, m.at + 0.6, U.linear);
        if (back > 0 && back < 1) U.packet(ctx, to.x, to.y, from.x, from.y, back, { size: 7 });

        var arrived = U.span(t, 3.7 + i * 0.15, 3.9 + i * 0.15);
        var working = t > 3.7 && t < m.at;
        var finished = t >= m.at;
        U.face(ctx, P.x, P.y, P.r, {
          alpha: 0.35 + 0.65 * Math.max(arrived, link),
          eyes: m.eyes, mouth: finished ? 'smile' : m.mouth,
          look: working ? Math.sin(t * 1.3 + i) * 0.5 : 0,
          blink: U.blink(t, 3.6 + i * 0.7, m.off),
          think: working ? 1 : 0, thinkT: t + i * 0.3
        });
        var status = finished ? (N ? m.doneShort : m.done) : (working ? (N ? m.short : m.work) + '…' : '');
        if (N) {
          U.text(ctx, m.role, P.x, P.y + P.r + 18, { size: 12, weight: 600, align: 'center' });
          if (status) U.text(ctx, status, P.x, P.y + P.r + 34, { size: 11, color: finished ? C.ink : C.ink2, align: 'center' });
          if (finished) U.check(ctx, P.x + P.r - 2, P.y - P.r + 4, 9);
        } else {
          U.text(ctx, m.role, P.x + P.r + 16, P.y - 4, { size: 14, weight: 600 });
          if (status) U.text(ctx, status, P.x + P.r + 16, P.y + 14, { size: 12, color: finished ? C.ink : C.ink2 });
          if (finished) U.check(ctx, P.x + P.r + 30 + U.measure(ctx, m.role, 14, 600), P.y - 8, 10);
        }
      });

      /* what is happening, in words */
      U.line(ctx, L.story.x, L.rule, L.story.x + L.story.w, L.rule, { color: C.rule });
      var step = STEPS[0];
      STEPS.forEach(function (s) { if (t >= s.at) step = s; });
      U.para(ctx, step.text, L.story.x, L.story.y, L.story.w,
        { size: L.story.size, alpha: U.span(t, step.at, step.at + 0.3), lh: L.story.size * 1.35 });
      ctx.restore();
    }
  });
})();
