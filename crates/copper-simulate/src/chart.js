(function () {
  'use strict';
  // The interactive figures of a copper sim bench page (see chart.rs): each
  // figure.chart holds a static SVG and its data; this draws the data again
  // with zoom, pan, cursors, curve toggles and axis scales.
  var NS = 'http://www.w3.org/2000/svg';
  var W = 940, LEFT = 76, RIGHT = 28, PW = W - LEFT - RIGHT, LABEL = 24, GAP = 18;
  var INK = '#1f2933', MUTED = '#52606d', GRID = '#d9e0e7', MAJOR = '#aab4be', LIMIT = '#111827', REF = '#7b8794';

  function esc(s) {
    return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }
  function sig(v, d) {
    if (!isFinite(v)) return '-';
    if (v === 0) return '0';
    var a = Math.abs(v);
    if (a >= 1e6 || a < 1e-4) {
      var e = v.toExponential(d - 1).split('e');
      return e[0].replace(/\.?0+$/, '') + 'e' + (+e[1]);
    }
    return String(+v.toPrecision(d));
  }
  // A value in the units of its panel (see chart::units): the largest named
  // unit not above it, percent, or the base text.
  function unitFor(U, v) {
    if (U.pct) return ['%', 0.01];
    if (!U.u.length) return [U.base && U.base !== '1' ? U.base : '', 1];
    var a = Math.abs(v), pick = null;
    if (a > 0 && isFinite(a)) {
      for (var i = U.u.length - 1; i >= 0; i--) if (a >= U.u[i][1] * 0.9995) { pick = U.u[i]; break; }
      if (!pick) pick = U.u[0];
    } else {
      pick = U.u.filter(function (u) { return u[1] === 1; })[0] || U.u[0];
    }
    return pick;
  }
  function fmt(v, U, d) {
    if (v === null || v === undefined || !isFinite(v)) return '-';
    var u = unitFor(U, v);
    return sig(v / u[1], d || 4) + (u[0] ? ' ' + u[0] : '');
  }
  function lower(xs, x) {
    var lo = 0, hi = xs.length;
    while (lo < hi) { var m = (lo + hi) >> 1; if (xs[m] < x) lo = m + 1; else hi = m; }
    return lo;
  }
  function niceStep(span, target) {
    var raw = span / target, mag = Math.pow(10, Math.floor(Math.log10(raw)));
    var steps = [1, 2, 2.5, 5, 10];
    for (var i = 0; i < steps.length; i++) if (span / (steps[i] * mag) <= target + 0.5) return steps[i] * mag;
    return 10 * mag;
  }
  function linTicks(lo, hi, target, degrees) {
    var span = Math.max(hi - lo, 1e-300), step;
    if (degrees && span >= 30) {
      step = [15, 30, 45, 90, 180].filter(function (s) { return span / s <= target + 0.5; })[0] || 180;
    } else step = niceStep(span, target);
    var out = [], first = Math.ceil(lo / step - 1e-9), last = Math.floor(hi / step + 1e-9);
    for (var k = first; k <= last && out.length < 50; k++) out.push(k * step);
    return { ticks: out, step: step };
  }
  function logTicks(lo, hi) {
    var out = [], d0 = Math.floor(Math.log10(lo)), d1 = Math.ceil(Math.log10(hi));
    var decades = 0;
    for (var d = d0; d <= d1; d++) if (Math.pow(10, d) >= lo * (1 - 1e-9) && Math.pow(10, d) <= hi * (1 + 1e-9)) decades++;
    var every = Math.max(1, Math.ceil(decades / 8));
    for (d = d0; d <= d1; d++) {
      [1, 2, 5].forEach(function (f) {
        var v = f * Math.pow(10, d);
        if (v >= lo * (1 - 1e-9) && v <= hi * (1 + 1e-9)) out.push({ v: v, major: f === 1, label: (f === 1 && (d - d0) % every === 0) || decades < 2 });
      });
    }
    return out;
  }

  function Chart(figure, data) {
    var self = this;
    this.figure = figure;
    this.data = data;
    this.n = data.steps.length;
    this.hidden = data.steps.map(function () { return false; });
    this.xlog = data.x.log;
    this.ylog = data.panels.map(function (p) { return p.log; });
    this.cursors = [];
    this.hover = null;
    var many = data.panels.length > 6;
    this.ph = data.panels.map(function (p) { return p.stem !== undefined && p.floor ? 240 : (many ? 140 : 200); });
    // The smallest positive x, the start of a logarithmic axis.
    var pos = Infinity;
    for (var p = 0; p < this.n; p++) {
      data.panels.forEach(function (panel) {
        var xs = self.xsOf(panel, p);
        for (var i = 0; i < xs.length; i++) if (xs[i] > 0) { pos = Math.min(pos, xs[i]); break; }
      });
    }
    this.logLo = data.x.log ? data.x.lo : Math.max(pos, (data.x.full[1] - Math.max(0, data.x.full[0])) * 1e-6);
    this.view = this.fullView();
    this.build();
    this.render();
  }

  Chart.prototype.xsOf = function (panel, p) {
    return panel.xs ? panel.xs[p] : this.data.xs[p];
  };
  Chart.prototype.fullView = function () {
    var x = this.data.x;
    return this.xlog ? [Math.max(this.logLo, x.lo), x.full[1]] : [x.full[0], x.full[1]];
  };
  Chart.prototype.X = function (x) {
    var v = this.view;
    if (this.xlog) return LEFT + (Math.log10(Math.max(x, 1e-300)) - Math.log10(v[0])) / (Math.log10(v[1]) - Math.log10(v[0])) * PW;
    return LEFT + (x - v[0]) / (v[1] - v[0]) * PW;
  };
  Chart.prototype.invX = function (px) {
    var v = this.view, t = (px - LEFT) / PW;
    if (this.xlog) return Math.pow(10, Math.log10(v[0]) + t * (Math.log10(v[1]) - Math.log10(v[0])));
    return v[0] + t * (v[1] - v[0]);
  };
  Chart.prototype.xText = function (x) {
    return fmt(x, this.data.x.units, 4);
  };

  Chart.prototype.build = function () {
    var self = this, d = this.data, f = this.figure;
    var live = document.createElement('div');
    live.className = 'live';
    var head = '<div class="bar"><b style="color:' + INK + ';font-size:14px">' + esc(d.title) + '</b></div>';
    if (d.subtitle) head += '<div class="hint">' + esc(d.subtitle) + '</div>';
    (d.notes || []).forEach(function (note) { head += '<div class="hint">' + esc(note) + '</div>'; });
    live.innerHTML = head;
    var bar = document.createElement('div');
    bar.className = 'bar';
    function button(text, title, act) {
      var b = document.createElement('button');
      b.type = 'button';
      b.textContent = text;
      b.title = title;
      b.addEventListener('click', act);
      bar.appendChild(b);
      return b;
    }
    button('Reset', 'show the whole x axis (or double-click the plot)', function () { self.view = self.fullView(); self.render(); });
    if (d.x.canLog) {
      this.xButton = button('x: ' + (this.xlog ? 'log' : 'linear'), 'switch the x axis between linear and logarithmic', function () {
        self.xlog = !self.xlog;
        var v = self.view;
        if (self.xlog) self.view = [Math.max(v[0], self.logLo), v[1]];
        if (self.view[1] <= self.view[0]) self.view = self.fullView();
        self.xButton.textContent = 'x: ' + (self.xlog ? 'log' : 'linear');
        self.render();
      });
    }
    button('Clear cursors', 'remove cursors A and B', function () { self.cursors = []; self.overlay(); });
    if (this.n > 1) {
      button('All', 'show every step', function () { self.hidden = self.hidden.map(function () { return false; }); self.render(); });
      button('None', 'hide every step', function () { self.hidden = self.hidden.map(function () { return true; }); self.render(); });
    }
    var hint = document.createElement('span');
    hint.className = 'hint';
    hint.textContent = 'drag: zoom x; shift-drag or arrow keys: pan; double-click: whole axis; click: cursor A, then B';
    bar.appendChild(hint);
    live.appendChild(bar);
    // The legend: by stepped value (when steps are styled by their params), and per step.
    this.legend = document.createElement('div');
    live.appendChild(this.legend);
    if (this.n > 1) this.buildLegend();
    var svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('class', 'plot');
    svg.setAttribute('tabindex', '0');
    svg.setAttribute('font-family', 'Helvetica, Arial, sans-serif');
    this.svg = svg;
    live.appendChild(svg);
    this.readout = document.createElement('div');
    this.readout.className = 'readout';
    live.appendChild(this.readout);
    if (d.footer) {
      var foot = document.createElement('div');
      foot.className = 'hint';
      foot.textContent = d.footer;
      live.appendChild(foot);
    }
    f.appendChild(live);
    f.classList.add('ready');
    this.events();
  };

  function swatch(color, dash) {
    return '<svg width="22" height="8"><line x1="1" y1="4" x2="21" y2="4" stroke="' + color + '" stroke-width="2"' +
      (dash ? ' stroke-dasharray="' + dash + '"' : '') + '/></svg>';
  }

  Chart.prototype.buildLegend = function () {
    var self = this, d = this.data, html = '';
    this.items = [];
    d.legend.forEach(function (group, g) {
      html += '<div class="legend"><span class="title">' + esc(group.title) + ':</span>';
      group.items.forEach(function (item, i) {
        html += '<span class="item" data-group="' + g + '" data-item="' + i + '" title="show or hide these steps">' +
          swatch(item.color, item.dash) + esc(item.label) + '</span>';
      });
      html += '</div>';
    });
    var steps = '';
    d.steps.forEach(function (s, p) {
      steps += '<span class="item" data-step="' + p + '" title="' + esc(s.label) + '">' + swatch(s.color, s.dash) + esc(s.short) + '</span>';
    });
    if (d.legend.length) html += '<details><summary>' + this.n + ' steps, one by one</summary><div class="legend">' + steps + '</div></details>';
    else html += '<div class="legend">' + steps + '</div>';
    this.legend.innerHTML = html;
    this.legend.addEventListener('click', function (e) {
      var item = e.target.closest('.item');
      if (!item) return;
      var members;
      if (item.dataset.step !== undefined) members = [+item.dataset.step];
      else members = d.legend[+item.dataset.group].items[+item.dataset.item].steps;
      var show = members.some(function (p) { return self.hidden[p]; });
      members.forEach(function (p) { self.hidden[p] = !show; });
      self.render();
    });
  };

  Chart.prototype.syncLegend = function () {
    var self = this, d = this.data;
    if (!this.legend) return;
    Array.prototype.forEach.call(this.legend.querySelectorAll('.item'), function (item) {
      var members = item.dataset.step !== undefined ? [+item.dataset.step] : d.legend[+item.dataset.group].items[+item.dataset.item].steps;
      var off = members.every(function (p) { return self.hidden[p]; });
      item.classList.toggle('off', off);
    });
  };

  // The visible y range of a panel: the visible data, and at the whole x axis
  // the limits drawn on it (the range the static figure uses).
  Chart.prototype.yRange = function (k) {
    var self = this, panel = this.data.panels[k], log = this.ylog[k];
    var lo = Infinity, hi = -Infinity, v0 = this.view[0], v1 = this.view[1];
    for (var p = 0; p < this.n; p++) {
      if (this.hidden[p]) continue;
      var xs = this.xsOf(panel, p), ys = panel.ys[p];
      if (!xs || !ys) continue;
      var a = Math.max(0, lower(xs, v0) - 1), b = Math.min(xs.length, lower(xs, v1) + 1);
      for (var i = a; i < b; i++) {
        var y = ys[i];
        if (y === null || !isFinite(y) || (log && y <= 0)) continue;
        if (panel.skip !== undefined && panel.skip !== null && Math.abs(xs[i] - panel.skip) < 1e-9) continue;
        if (y < lo) lo = y;
        if (y > hi) hi = y;
      }
    }
    var full = this.view[0] <= this.fullView()[0] * (1 + 1e-9) && this.view[1] >= this.fullView()[1] * (1 - 1e-9);
    if (panel.floor && full) {
      // A spectrum keeps the static figure's dB range and steps.
      return { lo: panel.range[0], hi: panel.range[1], step: panel.range[1] - panel.range[0] > 120 ? 20 : 10 };
    }
    if (full && log === panel.log) {
      if (log) { lo = Math.min(lo, panel.range[0]); hi = Math.max(hi, panel.range[1]); }
      else { lo = Math.min(lo, panel.range[0]); hi = Math.max(hi, panel.range[1]); }
    }
    if (!isFinite(lo)) { lo = log ? 1e-12 : -1; hi = log ? 1 : 1; }
    if (log) {
      lo = Math.pow(10, Math.floor(Math.log10(lo)));
      hi = Math.pow(10, Math.ceil(Math.log10(hi)));
      if (hi <= lo) hi = lo * 10;
      return { lo: lo, hi: hi };
    }
    var scale = Math.max(Math.abs(lo), Math.abs(hi));
    if (hi - lo <= scale * 1e-9) { var pad = scale === 0 ? 1 : scale * 0.01; lo -= pad; hi += pad; }
    if (!(full && !panel.floor)) { var m = (hi - lo) * 0.04; lo -= m; hi += m; }
    var t = linTicks(lo, hi, 5, panel.phase);
    var step = t.step;
    return { lo: Math.floor(lo / step + 1e-9) * step, hi: Math.ceil(hi / step - 1e-9) * step, step: step };
  };

  Chart.prototype.render = function () {
    var self = this, d = this.data, out = [], y = 8;
    this.layout = [];
    var v = this.view;
    var xgrid = [];
    if (this.xlog) {
      xgrid = logTicks(v[0], v[1]);
      // Harmonic numbers are short: every 1-2-5 line is labelled.
      if (d.x.kind === 'harmonic') xgrid.forEach(function (t) { t.label = true; });
    }
    else linTicks(v[0], v[1], 8, false).ticks.forEach(function (t) { xgrid.push({ v: t, major: true, label: true }); });
    d.panels.forEach(function (panel, k) {
      var h = self.ph[k], top = y, y0 = top + LABEL, log = self.ylog[k], r = self.yRange(k);
      var largest = Math.max(Math.abs(r.lo), Math.abs(r.hi));
      var unit = unitFor(panel.units, largest);
      var Y = function (val) {
        var t = log ? (Math.log10(Math.max(val, 1e-300)) - Math.log10(r.lo)) / (Math.log10(r.hi) - Math.log10(r.lo)) : (val - r.lo) / (r.hi - r.lo);
        return y0 + h - t * h;
      };
      self.layout.push({ top: y0, h: h, Y: Y, r: r, log: log });
      var label = panel.label + (!log && unit[0] ? ' (' + unit[0] + ')' : '');
      out.push('<text x="' + LEFT + '" y="' + (top + 15) + '" font-size="12" font-weight="bold" fill="' + INK + '">' + esc(label) + '</text>');
      if (panel.canLog) {
        out.push('<text class="ylog" data-panel="' + k + '" x="' + (LEFT + PW) + '" y="' + (top + 15) + '" font-size="11" text-anchor="end" fill="#1d5fa8" style="cursor:pointer">y: ' +
          (log ? 'log' : 'linear') + ' (click to switch)</text>');
      }
      out.push('<clipPath id="c' + self.id + '_' + k + '"><rect x="' + LEFT + '" y="' + y0 + '" width="' + PW + '" height="' + h + '"/></clipPath>');
      // Value ticks.
      if (log) {
        logTicks(r.lo, r.hi).forEach(function (t) {
          if (!t.major) return;
          var yy = Y(t.v);
          out.push('<line x1="' + LEFT + '" y1="' + yy + '" x2="' + (LEFT + PW) + '" y2="' + yy + '" stroke="' + GRID + '"/>');
          if (t.label) out.push('<text x="' + (LEFT - 6) + '" y="' + (yy + 4) + '" font-size="11" text-anchor="end" fill="' + MUTED + '">' + esc(fmt(t.v, panel.units, 3)) + '</text>');
        });
      } else {
        // The ticks of the range's own step, so that they meet its ends.
        var ticks = { ticks: [], step: r.step };
        for (var q = Math.round(r.lo / r.step); q <= Math.round(r.hi / r.step) && ticks.ticks.length < 60; q++) ticks.ticks.push(q * r.step);
        var dec = Math.max(0, Math.min(9, -Math.floor(Math.log10(ticks.step / unit[1]) + 1e-9) + (Math.abs(ticks.step / unit[1] / Math.pow(10, Math.floor(Math.log10(ticks.step / unit[1]))) - 2.5) < 1e-6 ? 1 : 0)));
        ticks.ticks.forEach(function (t) {
          var yy = Y(t);
          if (yy < y0 - 0.5 || yy > y0 + h + 0.5) return;
          var text = (t / unit[1]).toFixed(dec);
          if (/^-?[0.]+$/.test(text)) text = text.replace('-', '');
          out.push('<line x1="' + LEFT + '" y1="' + yy + '" x2="' + (LEFT + PW) + '" y2="' + yy + '" stroke="' + GRID + '"/>');
          out.push('<text x="' + (LEFT - 6) + '" y="' + (yy + 4) + '" font-size="11" text-anchor="end" fill="' + MUTED + '">' + text + '</text>');
        });
      }
      xgrid.forEach(function (t) {
        var xx = self.X(t.v);
        if (xx < LEFT - 0.5 || xx > LEFT + PW + 0.5) return;
        out.push('<line x1="' + xx + '" y1="' + y0 + '" x2="' + xx + '" y2="' + (y0 + h) + '" stroke="' + (t.major ? MAJOR : GRID) + '"/>');
        if (k === d.panels.length - 1 && t.label) out.push('<text x="' + xx + '" y="' + (y0 + h + 16) + '" font-size="11" text-anchor="middle" fill="' + MUTED + '">' + esc(self.xText(t.v)) + '</text>');
      });
      out.push('<g clip-path="url(#c' + self.id + '_' + k + ')">');
      if (panel.floor) {
        var fy = Y(panel.floor[0]);
        out.push('<rect x="' + LEFT + '" y="' + fy + '" width="' + PW + '" height="' + Math.max(0, y0 + h - fy) + '" fill="#eef1f4"/>');
        out.push('<text x="' + (LEFT + 6) + '" y="' + (y0 + h - 6) + '" font-size="11" fill="' + MUTED + '">' + esc(panel.floor[1]) + '</text>');
      }
      panel.refs.forEach(function (val) {
        var yy = Y(val);
        out.push('<line x1="' + LEFT + '" y1="' + yy + '" x2="' + (LEFT + PW) + '" y2="' + yy + '" stroke="' + REF + '" stroke-width="1.5"/>');
      });
      for (var p = 0; p < self.n; p++) {
        if (self.hidden[p]) continue;
        out.push(self.curve(panel, k, p, Y, log, y0 + h));
      }
      panel.lines.forEach(function (line) {
        var yy = Y(line[0]);
        out.push('<line x1="' + LEFT + '" y1="' + yy + '" x2="' + (LEFT + PW) + '" y2="' + yy + '" stroke="' + LIMIT + '" stroke-width="1.5" stroke-dasharray="6 4"/>');
        out.push('<text x="' + (LEFT + PW - 6) + '" y="' + (yy - y0 < 16 ? yy + 14 : yy - 5) + '" font-size="11" text-anchor="end" fill="' + INK + '" stroke="#fff" stroke-width="3.5" paint-order="stroke">' + esc(line[1]) + '</text>');
      });
      panel.windows.forEach(function (w) {
        var xx = self.X(w[0]), a = w[2] !== null ? Y(w[2]) : y0, b = w[1] !== null ? Y(w[1]) : y0 + h;
        out.push('<line x1="' + xx + '" y1="' + y0 + '" x2="' + xx + '" y2="' + (y0 + h) + '" stroke="' + LIMIT + '" stroke-opacity=".5" stroke-dasharray="2 3"/>');
        out.push('<path d="M' + xx + ' ' + a + 'L' + xx + ' ' + b + 'M' + (xx - 6) + ' ' + a + 'L' + (xx + 6) + ' ' + a + 'M' + (xx - 6) + ' ' + b + 'L' + (xx + 6) + ' ' + b + '" stroke="' + LIMIT + '" stroke-width="2" fill="none"/>');
        w[4].forEach(function (pt) {
          if (self.hidden[pt[0]]) return;
          out.push('<circle cx="' + xx + '" cy="' + Y(pt[1]) + '" r="4" fill="#fff" stroke="' + d.steps[pt[0]].color + '" stroke-width="2"/>');
        });
        var right = xx + 12 + w[3].length * 6.2 < LEFT + PW;
        // The label at the end of the panel away from the window: under it when the window sits high.
        var ly = Math.min(a, b) < y0 + h / 2 ? y0 + h - 6 : y0 + 14;
        out.push('<text x="' + (right ? xx + 12 : xx - 12) + '" y="' + ly + '" font-size="11" text-anchor="' + (right ? 'start' : 'end') + '" fill="' + INK + '" stroke="#fff" stroke-width="3.5" paint-order="stroke">' + esc(w[3]) + '</text>');
      });
      panel.points.forEach(function (pt) {
        if (self.hidden[pt[2]]) return;
        out.push('<circle cx="' + self.X(pt[0]) + '" cy="' + Y(pt[1]) + '" r="4" fill="#fff" stroke="' + d.steps[pt[2]].color + '" stroke-width="2"/>');
      });
      panel.segments.forEach(function (s) {
        if (s[3] >= 0 && self.hidden[s[3]]) return;
        var xx = self.X(s[0]), a = s[1] === null ? y0 + h : Y(s[1]), b = Y(s[2]);
        var color = s[3] >= 0 ? d.steps[s[3]].color : INK;
        out.push('<path d="M' + xx + ' ' + a + 'L' + xx + ' ' + b + (s[1] === null ? '' : 'M' + (xx - 5) + ' ' + b + 'L' + (xx + 5) + ' ' + b) + '" stroke="' + color + '" stroke-width="2" fill="none"/>');
        if (s[4]) {
          var right = xx + 10 + s[4].length * 6.2 < LEFT + PW;
          out.push('<text x="' + (right ? xx + 10 : xx - 10) + '" y="' + Math.max(y0 + 12, Math.min(y0 + h - 4, (a + b) / 2)) + '" font-size="11" text-anchor="' + (right ? 'start' : 'end') + '" fill="' + INK + '" stroke="#fff" stroke-width="3.5" paint-order="stroke">' + esc(s[4]) + '</text>');
        }
      });
      out.push('</g>');
      out.push('<rect x="' + LEFT + '" y="' + y0 + '" width="' + PW + '" height="' + h + '" fill="none" stroke="' + MUTED + '"/>');
      y = y0 + h + GAP;
    });
    var axisY = y - GAP + 34;
    out.push('<text x="' + (LEFT + PW / 2) + '" y="' + axisY + '" font-size="11" text-anchor="middle" fill="' + MUTED + '">' + esc(d.x.title + (this.xlog ? ' (logarithmic)' : '')) + '</text>');
    this.height = axisY + 10;
    out.push('<g class="overlay" pointer-events="none"></g>');
    this.svg.setAttribute('viewBox', '0 0 ' + W + ' ' + this.height);
    this.svg.innerHTML = '<rect width="' + W + '" height="' + this.height + '" fill="#fff"/>' + out.join('');
    this.over = this.svg.querySelector('g.overlay');
    this.syncLegend();
    this.overlay();
  };

  // One step's curve in one panel: its visible samples, at most the lowest and
  // highest of each pixel column; stems for a line spectrum.
  Chart.prototype.curve = function (panel, k, p, Y, log, bottom) {
    var xs = this.xsOf(panel, p), ys = panel.ys[p], s = this.data.steps[p];
    if (!xs || !ys || !xs.length) return '';
    var a = Math.max(0, lower(xs, this.view[0]) - 1), b = Math.min(xs.length, lower(xs, this.view[1]) + 1);
    if (this.xlog) while (a < b && xs[a] <= 0) a++;
    var skip = panel.skip !== undefined && panel.skip !== null ? panel.skip : null;
    if (panel.stem) {
      var spread = this.n > 1 ? Math.min(2, 12 / (this.n - 1)) : 0, off = (p - (this.n - 1) / 2) * spread;
      var path = '', lo = this.layout[k].r.lo;
      var col = null, best = -Infinity;
      for (var i = a; i < b; i++) {
        if (skip !== null && Math.abs(xs[i] - skip) < 1e-9) continue;
        var cx = Math.round(this.X(xs[i]));
        if (col !== null && cx !== col) { if (best > lo) path += 'M' + (col + off) + ' ' + bottom + 'V' + Y(best); best = -Infinity; }
        col = cx;
        if (ys[i] !== null && ys[i] > best) best = ys[i];
      }
      if (col !== null && best > lo) path += 'M' + (col + off) + ' ' + bottom + 'V' + Y(best);
      return path ? '<path d="' + path + '" stroke="' + s.color + '" stroke-width="1.5" fill="none"/>' : '';
    }
    var pts = [], runs = [], count = b - a;
    var push = function (i) {
      var y = ys[i];
      if (y === null || !isFinite(y) || (log && y <= 0)) { if (pts.length) { runs.push(pts); pts = []; } return; }
      pts.push(this.X(xs[i]).toFixed(1) + ',' + Y(y).toFixed(1));
    }.bind(this);
    if (count <= 2 * PW) {
      for (var j = a; j < b; j++) push(j);
    } else {
      var j0 = a;
      while (j0 < b) {
        var c0 = Math.floor(this.X(xs[j0])), j1 = j0 + 1, mn = j0, mx = j0;
        while (j1 < b && Math.floor(this.X(xs[j1])) === c0) {
          if (ys[j1] !== null && (ys[mn] === null || ys[j1] < ys[mn])) mn = j1;
          if (ys[j1] !== null && (ys[mx] === null || ys[j1] > ys[mx])) mx = j1;
          j1++;
        }
        if (mn <= mx) { push(mn); if (mx !== mn) push(mx); } else { push(mx); push(mn); }
        j0 = j1;
      }
    }
    if (pts.length) runs.push(pts);
    return runs.filter(function (r) { return r.length > 1; }).map(function (r) {
      return '<polyline points="' + r.join(' ') + '" fill="none" stroke="' + s.color + '" stroke-width="2" stroke-linejoin="round"' + (s.dash ? ' stroke-dasharray="' + s.dash + '"' : '') + '/>';
    }).join('');
  };

  // A curve's value at x: linear between samples (in log x on a frequency or
  // harmonic axis); the nearest bin of a line spectrum.
  Chart.prototype.valueAt = function (panel, p, x) {
    var xs = this.xsOf(panel, p), ys = panel.ys[p];
    if (!xs || !ys || !xs.length) return null;
    var i = lower(xs, x);
    if (panel.stem) {
      if (i > 0 && (i === xs.length || Math.abs(xs[i - 1] - x) < Math.abs(xs[i] - x))) i--;
      return i < xs.length ? ys[i] : null;
    }
    if (i === 0) return xs[0] === x ? ys[0] : null;
    if (i >= xs.length) return null;
    var x0 = xs[i - 1], x1 = xs[i], y0 = ys[i - 1], y1 = ys[i];
    if (y0 === null || y1 === null) return null;
    var logx = (this.data.x.kind === 'frequency' || this.data.x.kind === 'harmonic') && x0 > 0;
    var t = logx ? Math.log(x / x0) / Math.log(x1 / x0) : (x - x0) / (x1 - x0);
    return y0 + t * (y1 - y0);
  };

  Chart.prototype.overlay = function () {
    var self = this, d = this.data, out = [];
    if (!this.layout || !this.layout.length) return;
    var top = this.layout[0].top, last = this.layout[this.layout.length - 1], bottom = last.top + last.h;
    var marks = this.cursors.slice();
    if (this.hover !== null && marks.length < 2) marks.push(null);
    var names = ['A', 'B'];
    this.cursors.forEach(function (x, c) {
      var xx = self.X(x);
      if (xx < LEFT || xx > LEFT + PW) return;
      out.push('<line x1="' + xx + '" y1="' + top + '" x2="' + xx + '" y2="' + bottom + '" stroke="' + INK + '" stroke-width="1.2" stroke-dasharray="' + (c ? '5 3' : '') + '"/>');
      out.push('<text x="' + (xx + 4) + '" y="' + (top - 4) + '" font-size="12" font-weight="bold" fill="' + INK + '">' + names[c] + '</text>');
      d.panels.forEach(function (panel, k) {
        var L = self.layout[k];
        for (var p = 0; p < self.n; p++) {
          if (self.hidden[p]) continue;
          var v = self.valueAt(panel, p, x);
          if (v === null || !isFinite(v) || (L.log && v <= 0)) continue;
          var yy = L.Y(v);
          if (yy < L.top || yy > L.top + L.h) continue;
          out.push('<circle cx="' + xx + '" cy="' + yy + '" r="3" fill="' + d.steps[p].color + '"/>');
        }
      });
    });
    if (this.hover !== null) {
      var hx = this.X(this.hover);
      out.push('<line x1="' + hx + '" y1="' + top + '" x2="' + hx + '" y2="' + bottom + '" stroke="' + MUTED + '" stroke-width="1" stroke-opacity=".6"/>');
    }
    if (this.drag && this.drag.zoom) {
      var x0 = Math.min(this.drag.x0, this.drag.x1), x1 = Math.max(this.drag.x0, this.drag.x1);
      out.push('<rect x="' + x0 + '" y="' + top + '" width="' + (x1 - x0) + '" height="' + (bottom - top) + '" fill="#1d5fa8" fill-opacity=".12" stroke="#1d5fa8" stroke-width="1"/>');
    }
    this.over.innerHTML = out.join('');
    this.table();
  };

  // The readout: every visible curve at the cursors (or at the pointer), with differences.
  Chart.prototype.table = function () {
    var self = this, d = this.data;
    var xs = this.cursors.length ? this.cursors.slice() : (this.hover !== null ? [this.hover] : []);
    if (!xs.length) {
      this.readout.innerHTML = '<span class="hint">Point at the plot for values; click to place cursor A, click again for cursor B.</span>';
      return;
    }
    var two = xs.length === 2, time = d.x.kind === 'time' || d.x.kind === 'periods';
    var head = '<tr><th></th><th>' + (this.cursors.length ? 'A' : 'at pointer') + ' ' + esc(this.xText(xs[0])) + '</th>';
    if (two) {
      var dx = xs[1] - xs[0];
      head += '<th>B ' + esc(this.xText(xs[1])) + '</th><th>B - A: ' + esc(this.xText(dx));
      if (time && dx !== 0) head += d.x.kind === 'time' ? ', 1/(B - A) ' + esc(fmt(1 / Math.abs(dx), { u: [['Hz', 1], ['kHz', 1e3], ['MHz', 1e6], ['GHz', 1e9]], pct: false, base: '' }, 4)) : ', 1/(B - A) ' + sig(1 / Math.abs(dx), 4) + ' f0';
      if ((d.x.kind === 'frequency' || d.x.kind === 'harmonic') && xs[0] > 0) head += ', B/A ' + sig(xs[1] / xs[0], 4);
      head += '</th>';
    }
    head += '</tr>';
    var rows = [], shown = 0, more = 0;
    d.panels.forEach(function (panel) {
      for (var p = 0; p < self.n; p++) {
        if (self.hidden[p]) continue;
        if (shown >= 48) { more++; continue; }
        shown++;
        var s = d.steps[p];
        var a = self.valueAt(panel, p, xs[0]);
        var name = '<span class="sw" style="border-color:' + s.color + (s.dash ? ';border-top-style:dashed' : '') + '"></span>' + esc(panel.label) + (self.n > 1 ? ' <span class="muted">' + esc(s.short) + '</span>' : '');
        var row = '<tr><td>' + name + '</td><td>' + esc(fmt(a, panel.units, 5)) + '</td>';
        if (two) {
          var b = self.valueAt(panel, p, xs[1]);
          row += '<td>' + esc(fmt(b, panel.units, 5)) + '</td><td>' + esc(a === null || b === null ? '-' : fmt(b - a, panel.units, 4)) + '</td>';
        }
        rows.push(row + '</tr>');
      }
    });
    var note = more ? '<div class="hint">' + more + ' more curves: hide steps in the legend to list them.</div>' : '';
    this.readout.innerHTML = '<table>' + head + rows.join('') + '</table>' + note;
  };

  Chart.prototype.events = function () {
    var self = this, svg = this.svg;
    // Client to drawing coordinates through the SVG's own transform (exact with any letterboxing).
    function local(e) {
      var m = svg.getScreenCTM();
      if (m) {
        var p = svg.createSVGPoint();
        p.x = e.clientX; p.y = e.clientY;
        p = p.matrixTransform(m.inverse());
        return { x: p.x, y: p.y };
      }
      var r = svg.getBoundingClientRect(), k = r.width / W;
      return { x: (e.clientX - r.left) / k, y: (e.clientY - r.top) / k };
    }
    function inside(pt) {
      if (!self.layout.length) return false;
      var last = self.layout[self.layout.length - 1];
      return pt.x >= LEFT && pt.x <= LEFT + PW && pt.y >= self.layout[0].top - 20 && pt.y <= last.top + last.h + 20;
    }
    svg.addEventListener('click', function (e) {
      var t = e.target.closest && e.target.closest('.ylog');
      if (t) {
        var k = +t.getAttribute('data-panel');
        self.ylog[k] = !self.ylog[k];
        self.render();
        e.stopPropagation();
      }
    });
    svg.addEventListener('pointerdown', function (e) {
      if (e.target.closest && e.target.closest('.ylog')) return;
      var pt = local(e);
      if (!inside(pt) || e.button !== 0) return;
      self.drag = { x0: pt.x, x1: pt.x, shift: e.shiftKey, view: self.view.slice(), moved: false, zoom: false };
      svg.setPointerCapture(e.pointerId);
    });
    svg.addEventListener('pointermove', function (e) {
      var pt = local(e);
      if (self.drag) {
        self.drag.x1 = pt.x;
        if (Math.abs(pt.x - self.drag.x0) > 4) self.drag.moved = true;
        if (self.drag.moved && self.drag.shift) {
          // Pan: the axis follows the pointer.
          var v = self.drag.view, dx = (pt.x - self.drag.x0) / PW;
          if (self.xlog) {
            var l0 = Math.log10(v[0]), l1 = Math.log10(v[1]), s = dx * (l1 - l0);
            self.view = [Math.pow(10, l0 - s), Math.pow(10, l1 - s)];
          } else {
            var sp = dx * (v[1] - v[0]);
            self.view = [v[0] - sp, v[1] - sp];
          }
          self.render();
        } else if (self.drag.moved) {
          self.drag.zoom = true;
          self.overlay();
        }
        return;
      }
      self.hover = inside(pt) ? self.invX(Math.max(LEFT, Math.min(LEFT + PW, pt.x))) : null;
      self.overlay();
    });
    svg.addEventListener('pointerup', function (e) {
      var drag = self.drag;
      self.drag = null;
      if (!drag) return;
      var pt = local(e);
      if (!drag.moved) {
        var now = Date.now(), last = self.lastClick;
        if (last && now - last.t < 450 && Math.abs(last.x - pt.x) < 6) {
          // A double click: undo the first click's cursor and show the whole axis.
          self.cursors = last.cursors;
          self.lastClick = null;
          self.view = self.fullView();
          self.render();
          return;
        }
        self.lastClick = { t: now, x: pt.x, cursors: self.cursors.slice() };
        // A click: cursor A, then B; later clicks move the nearer one.
        var x = self.invX(Math.max(LEFT, Math.min(LEFT + PW, pt.x)));
        if (self.cursors.length < 2) self.cursors.push(x);
        else {
          var da = Math.abs(self.X(self.cursors[0]) - pt.x), db = Math.abs(self.X(self.cursors[1]) - pt.x);
          self.cursors[da <= db ? 0 : 1] = x;
        }
        self.overlay();
        return;
      }
      if (drag.zoom) {
        var a = self.invX(Math.max(LEFT, Math.min(drag.x0, pt.x))), b = self.invX(Math.min(LEFT + PW, Math.max(drag.x0, pt.x)));
        if (b > a) self.view = [a, b];
        self.render();
      }
    });
    svg.addEventListener('pointerleave', function () { self.hover = null; if (!self.drag) self.overlay(); });
    // Double clicks are recognised in pointerup (the overlay is redrawn between the clicks).
    svg.addEventListener('dblclick', function (e) { e.preventDefault(); });
    svg.addEventListener('keydown', function (e) {
      if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
      e.preventDefault();
      var sgn = e.key === 'ArrowLeft' ? -0.1 : 0.1, v = self.view;
      if (self.xlog) {
        var l0 = Math.log10(v[0]), l1 = Math.log10(v[1]), s = sgn * (l1 - l0);
        self.view = [Math.pow(10, l0 + s), Math.pow(10, l1 + s)];
      } else {
        var sp = sgn * (v[1] - v[0]);
        self.view = [v[0] + sp, v[1] + sp];
      }
      self.render();
    });
  };

  var charts = [];
  Array.prototype.forEach.call(document.querySelectorAll('figure.chart'), function (figure, i) {
    var node = figure.querySelector('script.chart-data');
    if (!node) return;
    try {
      var data = JSON.parse(node.textContent);
      Chart.prototype.id = i;
      var chart = Object.create(Chart.prototype);
      chart.id = i;
      Chart.call(chart, figure, data);
      charts.push(chart);
    } catch (error) {
      // The static figure stays.
      figure.classList.remove('ready');
      if (window.console) console.error('copper sim chart', error);
    }
  });
  window.copperSimCharts = charts;
})();
