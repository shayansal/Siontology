/* The Fold with two independent proof stacks (specification sections 8 and 9).
 *
 * A Pulse executes as a chain of chunks; each chunk statement binds the world
 * before and after it, and the last chunk is the deferred phase. Stack A (the
 * reference executor) and stack B (written only from the specification) each
 * verify every chunk; their proofs fold pairwise into two roots, and the
 * World Proof is the pair. A client accepts it only if both stacks accept.
 * In the second Pulse, stack A has a hypothetical soundness bug and accepts
 * an invalid chunk; stack B rejects it, so no World Proof exists.
 */
(function () {
  'use strict';
  var S = window.Sion, U = S.u, C = S.C;
  var D = 19, NCH = 8, BAD = 4;

  function geo(narrow) {
    var g = { narrow: narrow };
    if (!narrow) {
      g.chunk = function (i) { return { x: 56 + i * 80, y: 248, w: 70, h: 40 }; };
      g.leafA = function (i) { var c = g.chunk(i); return [c.x + c.w / 2, 204]; };
      g.leafB = function (i) { var c = g.chunk(i); return [c.x + c.w / 2, 332]; };
      g.levelA = [204, 160, 116, 72];
      g.levelB = [332, 376, 420, 464];
      g.panel = { x: 730, y: 196, w: 200, h: 150 };
      g.labelA = [56, 60]; g.labelB = [56, 500]; g.title = [56, 26];
    } else {
      g.chunk = function (i) { return { x: 152, y: 70 + i * 42, w: 56, h: 34 }; };
      g.leafA = function (i) { var c = g.chunk(i); return [130, c.y + c.h / 2]; };
      g.leafB = function (i) { var c = g.chunk(i); return [230, c.y + c.h / 2]; };
      g.levelA = [130, 98, 66, 32];
      g.levelB = [230, 262, 294, 328];
      g.panel = { x: 16, y: 428, w: 328, h: 98 };
      g.labelA = [16, 52]; g.labelB = [344, 52]; g.title = [180, 26];
    }
    return g;
  }

  /* position of node j at level l for stack s ('A' or 'B') */
  function node(g, s, l, j) {
    var span = Math.pow(2, l), first = j * span, last = first + span - 1;
    var a = s === 'A' ? g.leafA(first) : g.leafB(first);
    var b = s === 'A' ? g.leafA(last) : g.leafB(last);
    var lv = s === 'A' ? g.levelA[l] : g.levelB[l];
    return g.narrow ? [lv, (a[1] + b[1]) / 2] : [(a[0] + b[0]) / 2, lv];
  }

  function drawNode(ctx, s, p, size, a, color) {
    var h = size / 2;
    if (s === 'A') U.rect(ctx, p[0] - h, p[1] - h, size, size, { fill: color || C.ink, alpha: a });
    else U.rect(ctx, p[0] - h, p[1] - h, size, size, { stroke: color || C.ink, lw: 1.5, alpha: a });
  }

  S.scene('fold', {
    duration: D,
    still: 13.9,
    wide: { w: 960, h: 540 },
    narrow: { w: 360, h: 600 },
    draw: function (ctx, t, view) {
      var g = geo(view.narrow);
      var second = t >= 8.6;
      var base = second ? 8.8 : 0;
      var lt = t - base;
      var fade = second ? (1 - U.span(t, D - 0.8, D)) * U.span(t, 8.8, 9.2) : 1 - U.span(t, 8.0, 8.6);
      ctx.save();
      ctx.globalAlpha = fade;
      var i, l, j;

      U.text(ctx, second ? 'Pulse 413' : 'Pulse 412', g.title[0], g.title[1],
        { size: 15, weight: 600, align: view.narrow ? 'center' : 'left' });
      if (!view.narrow) {
        U.text(ctx, 'chunk statements, each one’s “after” is the next one’s “before”', 160, g.title[1], { size: 12, color: C.ink3 });
      }
      U.text(ctx, view.narrow ? 'stack A' : 'Stack A, the reference executor', g.labelA[0], g.labelA[1], { size: 13, weight: 600 });
      U.text(ctx, view.narrow ? 'stack B' : 'Stack B, written only from the specification', g.labelB[0], g.labelB[1],
        { size: 13, weight: 600, align: view.narrow ? 'right' : 'left' });

      /* chunks and their chain */
      for (i = 0; i < NCH; i++) {
        var c = g.chunk(i);
        var a = U.span(lt, i * 0.1, i * 0.1 + 0.35);
        var bad = second && i === BAD;
        U.rect(ctx, c.x, c.y, c.w, c.h, { stroke: bad ? C.signal : C.ink, lw: 1.25, alpha: a });
        var name = i === NCH - 1 ? (view.narrow ? 'def.' : 'deferred') : 'c' + i;
        U.text(ctx, name, c.x + c.w / 2, c.y + c.h / 2 + 4, { size: 12, align: 'center', color: bad ? C.signal : C.ink2, alpha: a });
        if (i < NCH - 1) {
          var n = g.chunk(i + 1);
          if (view.narrow) U.line(ctx, c.x + c.w / 2, c.y + c.h, n.x + n.w / 2, n.y, { color: C.rule2, alpha: a });
          else U.line(ctx, c.x + c.w, c.y + c.h / 2, n.x, n.y + n.h / 2, { color: C.rule2, alpha: a });
        }
      }
      if (second) {
        var cb = g.chunk(BAD);
        var ba = U.span(lt, 0.6, 1.0);
        if (view.narrow) U.text(ctx, 'invalid', cb.x + cb.w / 2, cb.y - 3, { size: 10, color: C.signal, align: 'center', alpha: ba });
        else U.text(ctx, 'invalid transition', cb.x + cb.w / 2, cb.y + cb.h + 16, { size: 11, color: C.signal, align: 'center', alpha: ba });
      }

      /* leaf proofs from both stacks */
      ['A', 'B'].forEach(function (s) {
        var ok = {};
        for (i = 0; i < NCH; i++) {
          var at = 1.0 + i * 0.12 + (s === 'B' ? 0.06 : 0);
          var a = U.span(lt, at, at + 0.3);
          var p = node(g, s, 0, i);
          var rejected = second && s === 'B' && i === BAD;
          var buggy = second && s === 'A' && i === BAD;
          ok[i] = !rejected;
          if (a <= 0) continue;
          if (rejected) U.cross(ctx, p[0], p[1], 12, { alpha: a });
          else drawNode(ctx, s, p, 12, a, buggy ? C.signal : null);
        }
        /* folding, level by level */
        for (l = 1; l <= 3; l++) {
          var from = 2.4 + (l - 1) * 0.6;
          var m = U.span(lt, from, from + 0.5);
          if (m <= 0) continue;
          var n = NCH / Math.pow(2, l);
          for (j = 0; j < n; j++) {
            var span = Math.pow(2, l), allOk = true;
            for (var q = j * span; q < (j + 1) * span; q++) if (!ok[q]) allOk = false;
            var pp = node(g, s, l, j);
            var kids = [node(g, s, l - 1, 2 * j), node(g, s, l - 1, 2 * j + 1)];
            kids.forEach(function (k) {
              U.line(ctx, k[0], k[1], U.lerp(k[0], pp[0], m), U.lerp(k[1], pp[1], m),
                { color: allOk ? C.ink2 : C.signal, lw: 1, alpha: allOk ? 1 : 0.6, dash: allOk ? null : [3, 3] });
            });
            if (m >= 1) {
              if (allOk) drawNode(ctx, s, pp, l === 3 ? 18 : 12, 1, second && s === 'A' && j * span <= BAD && BAD < (j + 1) * span ? C.signal : null);
              else U.rect(ctx, pp[0] - 6, pp[1] - 6, 12, 12, { stroke: C.signal, lw: 1, dash: [2, 2] });
            }
          }
        }
      });

      /* the World Proof is the pair of roots */
      var P = g.panel;
      var pa = U.span(lt, 4.2, 4.6);
      if (pa > 0) {
        U.rect(ctx, P.x, P.y, P.w, P.h, { stroke: C.ink, lw: 1.25, alpha: pa });
        U.text(ctx, 'World Proof for ' + (second ? 'Pulse 413' : 'Pulse 412'), P.x + 12, P.y + 22, { size: 13, weight: 600, alpha: pa });
        var rowY = view.narrow ? [P.y + 46, P.y + 46] : [P.y + 54, P.y + 80];
        var rowX = view.narrow ? [P.x + 12, P.x + 120] : [P.x + 12, P.x + 12];
        var bOk = !second;
        [['root A', true], ['root B', bOk]].forEach(function (r, k) {
          var y = rowY[k], x = rowX[k];
          U.text(ctx, r[0], x + 20, y + 4, { size: 12, color: C.ink2, alpha: pa });
          if (k === 0) U.rect(ctx, x, y - 6, 12, 12, { fill: C.ink, alpha: pa });
          else if (r[1]) U.rect(ctx, x, y - 6, 12, 12, { stroke: C.ink, lw: 1.5, alpha: pa });
          if (r[1]) U.check(ctx, x + 78, y, 11, { alpha: pa });
          else U.cross(ctx, x + 78, y, 10, { alpha: pa });
        });
        var verdict = second ? 'stack B rejects chunk 4:' : 'both stacks accept';
        var vy = view.narrow ? P.y + 70 : P.y + 116;
        var va = U.span(lt, 4.6, 5.0);
        U.text(ctx, verdict, P.x + 12, vy, { size: 12, weight: 600, color: second ? C.signal : C.ink, alpha: va });
        U.text(ctx, second ? 'no World Proof for this Pulse' : 'the World Proof is valid', P.x + 12, vy + 16,
          { size: 12, color: second ? C.signal : C.ink2, alpha: va });
      }
      /* roots travelling to the panel */
      var fly = U.span(lt, 4.0, 4.6, U.linear);
      if (fly > 0 && fly < 1) {
        var ra = node(g, 'A', 3, 0), rb = node(g, 'B', 3, 0);
        U.packet(ctx, ra[0], ra[1], P.x + 18, P.y + (view.narrow ? 40 : 48), fly, { size: 8 });
        if (!second) U.packet(ctx, rb[0], rb[1], P.x + (view.narrow ? 126 : 18), P.y + (view.narrow ? 40 : 74), fly, { size: 8 });
      }

      /* narration */
      var story = second
        ? 'A soundness bug in one stack cannot forge the world: the other stack rejects the invalid chunk.'
        : 'Two independent stacks verify every chunk and fold their proofs pairwise. The World Proof is the pair of roots.';
      if (view.narrow) U.para(ctx, story, 16, 552, 328, { size: 13, lh: 18, alpha: U.span(lt, 4.8, 5.3) });
      else U.para(ctx, story, 730, 384, 200, { size: 13, lh: 19, color: C.ink2, alpha: U.span(lt, 4.8, 5.3) });
      ctx.restore();
    }
  });
})();
