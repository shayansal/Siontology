/* Staking, epochs and slashing, as in `sion demo staking`
 * (specification v1.3, sections 5.26 to 5.30).
 *
 * A standby validator stakes 50,000 USD with a proof that it holds its keys
 * and joins the active set at the next epoch boundary; committees are
 * re-derived from the state. It then signs two consensus headers for one
 * round; a watchdog reports the signed evidence. Its whole stake is
 * forfeited, 5 % of it (2,500 USD) goes to the reporter on the devnet, and it
 * loses its seat at the next boundary. Epochs here are 24 Pulses long, as in
 * the demo's devnet.
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;
  var D = 21;
  var EPOCH = 24;

  /* Pulse shown on the timeline at time t */
  function pulseAt(t) {
    if (t < 3.0) return 10 + Math.floor(t / 3.0 * 8);
    if (t < 4.6) return 18 + Math.floor((t - 3.0) / 1.6 * 8);
    if (t < 12.0) return 26 + Math.floor((t - 4.6) / 7.4 * 18);
    if (t < 13.6) return 44 + Math.floor((t - 12.0) / 1.6 * 6);
    return 50 + Math.floor(Math.min(t - 13.6, 5) / 5 * 6);
  }
  function epochOf(p) { return Math.floor((p - 1) / EPOCH); }

  var COMMITTEES = [
    [['V1', 'V2', 'V3', 'V4'], ['V1', 'V2', 'V3', 'V4']],
    [['V1', 'V2', 'V4', 'V5'], ['V2', 'V3', 'V4', 'V5']],
    [['V1', 'V2', 'V3', 'V4'], ['V1', 'V2', 'V3', 'V4']]
  ];

  function layout(narrow) {
    if (!narrow) {
      return { tl: { x: 40, y: 64, w: 880 }, setY: 132, setX: 150, box: { w: 120, h: 62, gap: 14 },
        shards: [{ x: 40, y: 262, w: 420 }, { x: 500, y: 262, w: 420 }], seatW: 92,
        eq: { x: 40, y: 372 }, story: { x: 40, y: 512, w: 880 }, fs: 12 };
    }
    return { tl: { x: 16, y: 44, w: 328 }, setY: 100, setX: 16, box: { w: 60, h: 58, gap: 7 },
      shards: [{ x: 16, y: 210, w: 328 }, { x: 16, y: 270, w: 328 }], seatW: 76,
      eq: { x: 16, y: 344 }, story: { x: 16, y: 488, w: 328 }, fs: 11, narrow: true };
  }

  S.scene('staking', {
    duration: D,
    still: 11.2,
    wide: { w: 960, h: 540 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var L = layout(view.narrow);
      ctx.save();
      ctx.globalAlpha = 1 - U.span(t, D - 0.8, D);
      var p = pulseAt(t), ep = epochOf(p);
      var fs = L.fs;

      /* epochs of 24 Pulses */
      var T = L.tl, span = 72;
      var X = function (pulse) { return T.x + ((pulse - 1) / span) * T.w; };
      for (var e = 0; e < 3; e++) {
        var x0 = X(e * EPOCH + 1), x1 = X((e + 1) * EPOCH + 1);
        U.rect(ctx, x0, T.y, x1 - x0, 10, { fill: C.ink, alpha: e === ep ? 0.28 : 0.08 });
        U.line(ctx, x0, T.y - 6, x0, T.y + 16, { color: C.ink2 });
        U.text(ctx, 'epoch ' + e, x0 + 6, T.y - 10, { size: fs, weight: e === ep ? 600 : 400, color: e === ep ? C.ink : C.ink3 });
      }
      U.line(ctx, X(73), T.y - 6, X(73), T.y + 16, { color: C.ink2 });
      var px = X(p);
      U.line(ctx, px, T.y - 2, px, T.y + 14, { color: C.ink, lw: 2 });
      U.text(ctx, 'Pulse ' + p, px, T.y + 30, { size: fs, color: C.ink2, align: 'center' });

      /* the validator set */
      if (!L.narrow) U.text(ctx, 'active set', 40, L.setY + 26, { size: fs, color: C.ink3 });
      var staked = U.span(t, 1.0, 1.8);
      var joined = ep >= 1;
      var slashed = U.span(t, 9.8, 10.6);
      var left = ep >= 2;
      for (var i = 0; i < 5; i++) {
        var bx = L.setX + i * (L.box.w + L.box.gap), by = L.setY;
        var five = i === 4;
        var isActive = !five || (joined && !left);
        var stroke = five && slashed > 0.5 ? C.signal : C.ink;
        U.rect(ctx, bx, by, L.box.w, L.box.h, { stroke: stroke, lw: 1.25, dash: isActive ? null : [4, 3], alpha: five && left ? 0.45 : 1 });
        U.text(ctx, 'V' + (i + 1), bx + 8, by + 18, { size: 13, weight: 600, alpha: five && left ? 0.45 : 1 });
        var status = 'active';
        if (five) {
          if (!joined) status = staked > 0 ? (L.narrow ? 'joins next' : 'joins next epoch') : 'standby';
          else if (slashed > 0.5 && !left) status = 'slashed';
          else if (left) status = L.narrow ? 'no seat' : 'lost its seat';
        }
        U.text(ctx, status, bx + 8, by + 36, { size: fs, color: five && slashed > 0.5 ? C.signal : C.ink2, alpha: five && left ? 0.6 : 1 });
        /* stake bar */
        var frac = five ? staked * (1 - slashed) : 1;
        U.rect(ctx, bx + 8, by + L.box.h - 14, L.box.w - 16, 5, { stroke: C.rule2, lw: 1 });
        U.rect(ctx, bx + 8, by + L.box.h - 14, (L.box.w - 16) * frac, 5, { fill: five && slashed > 0 ? C.signal : C.ink2 });
      }
      if (staked > 0 && staked < 1) {
        var vx = L.setX + 4 * (L.box.w + L.box.gap) + L.box.w / 2;
        U.text(ctx, L.narrow ? 'stakes 50,000' : 'stakes 50,000 USD with a proof of key possession', L.narrow ? vx : vx, L.setY - 12,
          { size: fs, weight: 600, align: L.narrow ? 'center' : 'right', alpha: U.win(staked, 0, 1, 0.2) });
      }

      /* committees, re-derived from the state at each boundary */
      var com = COMMITTEES[Math.min(2, ep)];
      L.shards.forEach(function (sh, s) {
        U.text(ctx, 'shard ' + s + ' committee', sh.x, sh.y - 10, { size: fs, color: C.ink3 });
        var flash = (ep >= 1 && t < 6.0 && t > 4.6) || (ep >= 2 && t < 15.0 && t > 13.6) ? 1 : 0;
        com[s].forEach(function (name, k) {
          var sx = sh.x + k * (L.seatW + 8);
          var mark = name === 'V5';
          U.rect(ctx, sx, sh.y, L.seatW, 30, { stroke: flash ? C.ink : C.rule2, lw: flash ? 1.5 : 1 });
          U.text(ctx, name, sx + L.seatW / 2, sh.y + 20, { size: 12, weight: mark ? 600 : 400, align: 'center',
            color: mark && slashed > 0.5 ? C.signal : C.ink });
        });
      });
      var note = '';
      if (t > 4.6 && t < 7.0) note = 'committees re-derived from the state';
      if (t > 13.6 && t < 16.4) note = 'committees re-derived without V5';
      if (note) U.text(ctx, note, L.narrow ? L.shards[1].x : L.shards[1].x + L.shards[1].w, L.shards[1].y + (L.narrow ? 46 : -10),
        { size: fs, weight: 600, align: L.narrow ? 'left' : 'right' });

      /* equivocation: two headers for one round */
      var E = L.eq;
      var eqA = U.span(t, 6.6, 7.2);
      if (eqA > 0) {
        var hw = L.narrow ? 100 : 160;
        ['header A', 'header B'].forEach(function (h, k) {
          var hx = E.x + k * (hw + 12);
          U.rect(ctx, hx, E.y, hw, 44, { stroke: C.signal, lw: 1.25, alpha: eqA });
          U.text(ctx, h, hx + 8, E.y + 18, { size: 12, weight: 600, alpha: eqA });
          U.text(ctx, L.narrow ? 'same round, V5' : 'same round, signed by V5', hx + 8, E.y + 34, { size: L.narrow ? 10 : 11, color: C.ink2, alpha: eqA });
        });
        var rep = U.span(t, 8.2, 8.8);
        var wx = E.x + (L.narrow ? 0 : 2 * (hw + 12) + 30), wy = L.narrow ? E.y + 58 : E.y;
        if (rep > 0) {
          U.rect(ctx, wx, wy, L.narrow ? 150 : 150, 44, { stroke: C.ink, lw: 1.25, alpha: rep });
          U.text(ctx, 'watchdog', wx + 8, wy + 18, { size: 12, weight: 600, alpha: rep });
          U.text(ctx, 'reports the evidence', wx + 8, wy + 34, { size: 11, color: C.ink2, alpha: rep });
        }
        var res = U.span(t, 9.8, 10.4);
        if (res > 0) {
          var rx = L.narrow ? E.x + 162 : wx + 180, ry = wy;
          U.para(ctx, '50,000 USD forfeited; 2,500 USD of it to the watchdog.', rx, ry + 16, L.narrow ? 166 : 300,
            { size: L.narrow ? 11 : 13, lh: L.narrow ? 15 : 18, weight: 600, color: C.signal, alpha: res });
        }
      }

      /* narration */
      var story;
      if (t < 3.0) story = 'A standby validator stakes 50,000 USD with a proof that it holds its keys.';
      else if (t < 6.6) story = 'At the next epoch boundary it joins the active set, and committees are re-derived from the state.';
      else if (t < 9.8) story = 'It signs two consensus headers for one round. A watchdog reports the signed evidence.';
      else if (t < 13.6) story = 'Its whole stake is forfeited; 5 % of it goes to the watchdog that reported it.';
      else story = 'At the next boundary it loses its seat. Every change follows the rules in the state, so the World Proof covers it.';
      U.line(ctx, L.story.x, L.story.y - 22, L.story.x + L.story.w, L.story.y - 22, { color: C.rule });
      if (L.narrow) U.para(ctx, story, L.story.x, L.story.y, L.story.w, { size: 13, lh: 18 });
      else U.text(ctx, story, L.story.x, L.story.y, { size: 15 });
      ctx.restore();
    }
  });
})();
