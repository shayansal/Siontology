/* Contention Consensus: a certified DAG in the Bullshark family, as in
 * crates/sion-consensus/src/core.rs.
 *
 * Every round, each of four validators proposes a header that references at
 * least three certificates (more than two thirds of the stake) from the round
 * before; a header with votes from more than two thirds becomes a
 * certificate. Even rounds have a round-robin leader, the anchor. An anchor
 * commits once certificates of the next round holding at least the validity
 * stake (more than one third: here two of four) reference it; committing
 * orders its not-yet-ordered causal history by round, then by author.
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;
  var NV = 4;

  function leader(r) { return (r / 2) % NV; }
  function isAnchorRound(r) { return r % 2 === 0; }

  var cache = {};
  function present(r) {
    if (cache['p' + r]) return cache['p' + r];
    var list = [], missing = -1;
    var rr = U.rng(r * 131 + 7);
    if (r > 2 && rr() < 0.3) {
      missing = Math.floor(rr() * NV);
      if (isAnchorRound(r) && missing === leader(r)) missing = -1;
    }
    for (var v = 0; v < NV; v++) if (v !== missing) list.push(v);
    cache['p' + r] = list;
    return list;
  }
  function appear(r, v) { return (r - 1) + 0.08 + U.rng(r * 977 + v * 31)() * 0.28; }
  function certified(r, v) { return appear(r, v) + 0.3; }
  function parents(r, v) {
    var key = 'q' + r + ':' + v;
    if (cache[key]) return cache[key];
    var out = [];
    if (r > 1) {
      var prev = present(r - 1);
      out = prev.slice();
      var rr = U.rng(r * 7 + v * 13 + 1);
      if (prev.length === 4 && rr() < 0.45) {
        var cand = prev.filter(function (u) { return !(isAnchorRound(r - 1) && u === leader(r - 1)); });
        var drop = cand[Math.floor(rr() * cand.length)];
        out = prev.filter(function (u) { return u !== drop; });
      }
    }
    cache[key] = out;
    return out;
  }
  function commitTime(r) {
    var refs = present(r + 1).filter(function (v) { return parents(r + 1, v).indexOf(leader(r)) >= 0; })
      .map(function (v) { return certified(r + 1, v); }).sort(function (a, b) { return a - b; });
    return refs.length >= 2 ? refs[1] : Infinity;
  }

  /* replay commits up to time t */
  function replay(t) {
    var maxR = Math.floor(t) + 1;
    var done = {}, seq = [], anchors = {};
    for (var r = 2; r <= maxR; r += 2) {
      var ct = commitTime(r);
      if (ct > t) continue;
      anchors[r] = ct;
      var stack = [[r, leader(r)]], hist = [];
      while (stack.length) {
        var n = stack.pop(), k = n[0] + ':' + n[1];
        if (done[k]) continue;
        done[k] = ct;
        hist.push(n);
        parents(n[0], n[1]).forEach(function (u) { stack.push([n[0] - 1, u]); });
      }
      hist.sort(function (a, b) { return a[0] - b[0] || a[1] - b[1]; });
      hist.forEach(function (n) { seq.push({ r: n[0], v: n[1], at: ct }); });
    }
    return { done: done, seq: seq, anchors: anchors };
  }

  function layout(narrow) {
    if (!narrow) {
      return {
        pos: function (r, v, t) { return [860 - (t - (r - 1)) * 96, 112 + v * 74]; },
        clip: [96, 70, 830, 330], size: 26, names: function (v) { return [40, 112 + v * 74 + 18]; },
        fade: function (p) { return U.clamp((p[0] - 96) / 70); },
        strip: { x: 40, y: 452, w: 880, cell: 11, gap: 3 }, story: { x: 40, y: 512, w: 880 },
        roundLabel: function (x, r) { return [x + 13, 92]; }
      };
    }
    return {
      pos: function (r, v, t) { return [76 + v * 72, 410 - (t - (r - 1)) * 64]; },
      clip: [40, 52, 320, 392], size: 22, names: function (v) { return [76 + v * 72 + 11, 40]; },
      fade: function (p) { return U.clamp((p[1] - 52) / 60); },
      strip: { x: 16, y: 470, w: 328, cell: 8, gap: 3 }, story: { x: 16, y: 528, w: 328 },
      roundLabel: function (y, r) { return [16, y + 15]; },
      narrow: true
    };
  }

  S.scene('dag', {
    duration: 0,
    start: 6.0,
    still: 10.75,
    wide: { w: 960, h: 540 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var L = layout(view.narrow);
      var R = replay(t);
      var maxR = Math.floor(t) + 1;
      var s = L.size;

      for (var v = 0; v < NV; v++) {
        var nm = L.names(v);
        U.text(ctx, 'V' + (v + 1), nm[0], nm[1], { size: 13, weight: 600, align: L.narrow ? 'center' : 'left' });
      }

      ctx.save();
      ctx.beginPath();
      ctx.rect(L.clip[0], L.clip[1], L.clip[2], L.clip[3]);
      ctx.clip();
      var lo = Math.max(1, maxR - 10);
      var r, i;
      /* edges first */
      for (r = lo + 1; r <= maxR; r++) {
        present(r).forEach(function (v) {
          if (t < appear(r, v)) return;
          var p = L.pos(r, v, t);
          var a = U.span(t, appear(r, v), appear(r, v) + 0.25) * L.fade(L.pos(r - 1, 0, t));
          parents(r, v).forEach(function (u) {
            var q = L.pos(r - 1, u, t);
            var toAnchor = isAnchorRound(r - 1) && u === leader(r - 1);
            var x1 = L.narrow ? p[0] + s / 2 : p[0], y1 = L.narrow ? p[1] : p[1] + s / 2;
            var x2 = L.narrow ? q[0] + s / 2 : q[0] + s, y2 = L.narrow ? q[1] + s : q[1] + s / 2;
            U.line(ctx, x1, y1, x2, y2, { color: toAnchor ? C.ink2 : C.rule2, lw: toAnchor ? 1.25 : 1, alpha: a });
          });
        });
      }
      /* nodes */
      for (r = lo; r <= maxR; r++) {
        var anchorRound = isAnchorRound(r);
        var cols = L.pos(r, 0, t);
        var fr = L.fade(cols);
        if (!L.narrow) {
          var rl = L.roundLabel(cols[0], r);
          U.text(ctx, 'round ' + r, rl[0], rl[1], { size: 11, color: C.ink3, align: 'center', alpha: fr });
        }
        present(r).forEach(function (v) {
          var ta = appear(r, v);
          if (t < ta) return;
          var p = L.pos(r, v, t);
          var a = U.span(t, ta, ta + 0.2) * fr;
          var cert = U.span(t, certified(r, v) - 0.1, certified(r, v));
          var key = r + ':' + v;
          var committed = R.done[key] !== undefined ? U.span(t, R.done[key], R.done[key] + 0.35) : 0;
          U.rect(ctx, p[0], p[1], s, s, { stroke: cert > 0.5 ? C.ink : C.ink3, lw: 1.25, dash: cert > 0.5 ? null : [3, 3], alpha: a });
          U.rect(ctx, p[0], p[1], s, s, { fill: C.ink, alpha: a * (0.16 * cert + 0.84 * committed) });
          if (anchorRound && v === leader(r)) {
            U.rect(ctx, p[0] - 5, p[1] - 5, s + 10, s + 10, { stroke: C.ink, lw: 1.25, alpha: a });
            var ct = R.anchors[r];
            if (ct !== undefined) {
              var flash = U.win(t, ct, ct + 1.6, 0.25);
              if (flash > 0) U.rect(ctx, p[0] - 10, p[1] - 10, s + 20, s + 20, { stroke: C.ink, lw: 1, alpha: flash * 0.6 });
            }
          }
        });
      }
      ctx.restore();
      if (L.narrow) {
        for (r = lo; r <= maxR; r++) {
          var py = L.pos(r, 0, t)[1];
          if (py < 52 || py > 444) continue;
          U.text(ctx, 'r' + r, 16, py + 15, { size: 11, color: C.ink3, alpha: L.fade([0, py]) });
        }
      }

      /* committed sequence */
      var St = L.strip;
      U.text(ctx, 'committed sequence, by round and then author', St.x, St.y - 10, { size: 12, color: C.ink3 });
      var per = Math.floor((St.w + St.gap) / (St.cell + St.gap));
      var start = Math.max(0, R.seq.length - per);
      for (i = start; i < R.seq.length; i++) {
        var it = R.seq[i];
        var a2 = U.span(t, it.at, it.at + 0.4);
        var x = St.x + (i - start) * (St.cell + St.gap);
        U.rect(ctx, x, St.y, St.cell, St.cell, { fill: it.r % 2 === 0 && it.v === leader(it.r) ? C.ink : C.ink2, alpha: a2 });
      }

      /* narration */
      var lastAnchor = 0;
      Object.keys(R.anchors).forEach(function (k) { if (R.anchors[k] > lastAnchor) lastAnchor = R.anchors[k]; });
      var recent = t - lastAnchor < 0.9;
      var story = recent
        ? 'An anchor committed: two certificates of the next round reference it, so its history joins the sequence.'
        : 'Every validator proposes every round, referencing at least three certificates from the round before.';
      U.line(ctx, L.story.x, L.story.y - 22, L.story.x + L.story.w, L.story.y - 22, { color: C.rule });
      if (L.narrow) U.para(ctx, story, L.story.x, L.story.y, L.story.w, { size: 13, lh: 18 });
      else U.text(ctx, story, L.story.x, L.story.y, { size: 15 });
    }
  });
})();
