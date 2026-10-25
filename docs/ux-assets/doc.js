/* ============================================================================
   UX 规范阅读器
   - 主文档只含概览与外壳；每章正文由 ux-data/<id>.js 按需注入
   - 数据是 JSON 对象，外层两行赋值只为绕开 file:// 下的 fetch 限制
   ========================================================================== */
(function () {
  'use strict';

  var MANIFEST = window.UX_MANIFEST || [];
  var PREVIEWS = window.UX_PREVIEWS || {};
  var ANCHORS = window.UX_ANCHORS || {};
  var BY_ID = {};
  MANIFEST.forEach(function (c) { BY_ID[c.id] = c; });

  var view = document.getElementById('view');
  var overview = document.getElementById('overview');
  var chapterEl = document.getElementById('chapter');
  var bodyEl = document.getElementById('chapter-body');
  var tocList = document.getElementById('toc-list');
  var filterInput = document.getElementById('toc-filter');
  var progress = document.getElementById('read-progress');
  var root = document.documentElement;

  var current = null;
  var reqSeq = 0;
  var engineMode = 'avalonia';
  var mountedPreviews = false;

  /* ---------------------------------------------------------------- 主题 */
  function paintTokens() {
    var cs = getComputedStyle(root);
    var set = function (id, key) {
      var el = document.getElementById(id);
      if (el) el.textContent = cs.getPropertyValue(key).trim();
    };
    set('v-bg', '--bg-homepage'); set('v-content', '--surface-content');
    set('v-elev', '--surface-elevated'); set('v-strong', '--surface-elevated-strong');
    set('v-t1', '--text-1'); set('v-t3', '--text-3'); set('v-t4', '--text-4');
    set('v-bp', '--border-popover');
  }

  function applyTheme(t) {
    root.dataset.theme = t;
    try { localStorage.setItem('yk-ux-theme', t); } catch (e) { /* 忽略 */ }
    ['dark', 'light'].forEach(function (k) {
      var b = document.getElementById('th-' + k);
      if (b) b.setAttribute('aria-pressed', String(k === t));
    });
    paintTokens();
  }

  function applyEngine(e) {
    engineMode = e;
    root.dataset.engine = e;
    ['avalonia', 'ideal'].forEach(function (k) {
      var b = document.getElementById('eng-' + k);
      if (b) b.setAttribute('aria-pressed', String(k === e));
    });
    document.querySelectorAll('.topbar').forEach(function (el) {
      el.style.background = e === 'ideal' ? 'var(--surface-elevated)' : 'var(--surface-elevated-strong)';
    });
  }

  (function bootPrefs() {
    var t = root.dataset.theme;
    try { var saved = localStorage.getItem('yk-ux-theme'); if (saved) t = saved; } catch (e) { }
    applyTheme(t === 'light' ? 'light' : 'dark');
    applyEngine(root.dataset.engine === 'ideal' ? 'ideal' : 'avalonia');
  })();

  document.getElementById('th-dark').addEventListener('click', function () { applyTheme('dark'); });
  document.getElementById('th-light').addEventListener('click', function () { applyTheme('light'); });
  document.getElementById('eng-avalonia').addEventListener('click', function () { applyEngine('avalonia'); });
  document.getElementById('eng-ideal').addEventListener('click', function () { applyEngine('ideal'); });

  /* ------------------------------------------------------------ 目录 / 卡片 */
  function esc(s) {
    return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  function groupNames() {
    var seen = {}, out = [];
    MANIFEST.forEach(function (c) { if (!seen[c.group]) { seen[c.group] = 1; out.push(c.group); } });
    return out;
  }

  function buildNav() {
    tocList.innerHTML = groupNames().map(function (g) {
      var links = MANIFEST.filter(function (c) { return c.group === g; }).map(function (c) {
        return '<a href="#' + c.id + '" data-id="' + c.id + '" title="' + esc(c.summary) + '">' +
          '<span class="n">' + esc(c.num) + '</span>' + esc(c.title) + '</a>';
      }).join('');
      return '<div class="toc-group" data-group="' + esc(g) + '"><h5>' + esc(g) + '</h5>' + links + '</div>';
    }).join('');

    var totalKB = MANIFEST.reduce(function (n, c) { return n + c.kb; }, 0);
    document.getElementById('home-stat-chapters').textContent = MANIFEST.length + ' 章独立文档';
    document.getElementById('home-stat-size').textContent = (totalKB / 1024).toFixed(1) + ' MB 按需加载';
  }

  function buildHomeCards() {
    var host = document.getElementById('chapter-groups');
    host.innerHTML = groupNames().map(function (g) {
      var cards = MANIFEST.filter(function (c) { return c.group === g; }).map(function (c) {
        return '<a class="chapter-card" href="#' + c.id + '" data-id="' + c.id + '">' +
          '<span class="n">' + esc(c.num) + '</span>' +
          '<span class="t">' + esc(c.title) + '</span>' +
          '<span class="s">' + esc(c.summary) + '</span>' +
          '<span class="m">' + c.sections + ' 节 · ' + c.kb + ' KB</span></a>';
      }).join('');
      return '<div class="card-group" data-group="' + esc(g) + '"><h3>' + esc(g) + '</h3>' +
        '<div class="card-grid">' + cards + '</div></div>';
    }).join('');
  }

  /* ---------------------------------------------------------------- 预览 */
  function mountPreviews() {
    if (mountedPreviews) return;
    mountedPreviews = true;
    var mounts = document.querySelectorAll('[data-preview]');
    Array.prototype.forEach.call(mounts, function (el) {
      var key = el.getAttribute('data-preview');
      var html = PREVIEWS[key];
      if (!html) { el.innerHTML = '<p class="home-hint">预览缺失：' + esc(key) + '</p>'; return; }
      el.innerHTML = html;
    });
    fitCols(overview);
    paintTokens();
    applyEngine(engineMode);
  }

  /* 首页网格列数（原文档脚本的 fitCols 的 scoped 版） */
  function fitCols(scope) {
    if (!scope) return;
    var rail = scope.querySelector('#rail');
    if (!rail) return;
    var w = rail.clientWidth || (rail.parentElement ? rail.parentElement.clientWidth : 0);
    if (!w) return;
    var cols = Math.max(2, Math.min(12, Math.floor(w / 208)));
    rail.style.setProperty('--cols', cols);
  }

  /* ------------------------------------------------------------ 章节路由 */
  function hashId() {
    var h = decodeURIComponent(location.hash.replace(/^#/, ''));
    return h.replace(/^\//, '');
  }

  function scrollToY(y) {
    var de = document.documentElement;
    var prev = de.style.scrollBehavior;
    de.style.scrollBehavior = 'auto';
    window.scrollTo(0, Math.max(0, y));
    de.style.scrollBehavior = prev;
  }

  function scrollToEl(id) {
    var el = document.getElementById(id);
    if (!el) return;
    scrollToY(el.getBoundingClientRect().top + window.pageYOffset - 118);
  }

  function setActive(id) {
    Array.prototype.forEach.call(tocList.querySelectorAll('a[data-id]'), function (a) {
      var on = a.getAttribute('data-id') === id;
      a.classList.toggle('on', on);
      if (on) a.setAttribute('aria-current', 'page');
      else a.removeAttribute('aria-current');
    });
  }

  function showHome() {
    reqSeq++;
    current = null;
    chapterEl.hidden = true;
    overview.hidden = false;
    setActive(null);
    document.title = 'YKPili · UX 设计规范';
    progress.style.width = '0';
    mountPreviews();
    scrollToY(0);
  }

  function loadChapter(id) {
    return new Promise(function (resolve, reject) {
      if (window.UX_CHAPTERS && window.UX_CHAPTERS[id]) { resolve(window.UX_CHAPTERS[id]); return; }
      var s = document.createElement('script');
      s.src = 'ux-data/' + id + '.js';
      s.onload = function () {
        var d = window.UX_CHAPTERS && window.UX_CHAPTERS[id];
        if (d) resolve(d); else reject(new Error('数据为空'));
      };
      s.onerror = function () { reject(new Error('文件未找到：ux-data/' + id + '.js')); };
      document.head.appendChild(s);
    });
  }

  function showChapter(meta, anchor) {
    if (current === meta.id) {
      if (anchor) scrollToEl(anchor); else scrollToY(0);
      return;
    }
    var seq = ++reqSeq;
    current = meta.id;
    overview.hidden = true;
    chapterEl.hidden = false;
    setActive(meta.id);
    document.title = meta.num + ' ' + meta.title + ' · YKPili UX';
    document.getElementById('reader-title').textContent = meta.num + ' · ' + meta.title;
    document.getElementById('reader-meta').textContent = (meta.sections ? meta.sections + ' 节 · ' : '单页 · ') + meta.kb + ' KB';
    bodyEl.innerHTML = '<div class="chapter-loading">正在载入 ' + esc(meta.num + ' ' + meta.title) + '…</div>';
    scrollToY(0);

    loadChapter(meta.id).then(function (data) {
      if (seq !== reqSeq) return;
      bodyEl.innerHTML = data.html;
      afterRender(meta);
      if (anchor) setTimeout(function () { scrollToEl(anchor); }, 40);
    }).catch(function (err) {
      if (seq !== reqSeq) return;
      bodyEl.innerHTML = '<div class="chapter-error"><b>这一章没有载入。</b><br>' +
        esc(err.message) + '<br><br>如果是从本地打开的，请确认 <code>ux-data/</code> 与主文档在同一目录；' +
        '也可以查看归档的完整版：<a href="archive/ux-design-preview.full-2026-10-25.html">ux-design-preview.full</a>。' +
        '</div>';
    });
  }

  function afterRender(meta) {
    paintTokens();
    applyEngine(engineMode);
    fitCols(bodyEl);
    bindToggles(bodyEl);
    initPlayer(bodyEl);

    /* 给 h3 编号，生成「本章目录」 */
    var hs = Array.prototype.slice.call(bodyEl.querySelectorAll('h3'));
    hs.forEach(function (h, i) { if (!h.id) h.id = 'h-' + i; });
    if (hs.length > 1) {
      var nav = document.createElement('details');
      nav.className = 'chapter-toc';
      var chips = hs.map(function (h, i) {
        return '<button type="button" data-target="' + h.id + '">' + esc(h.textContent.trim()) + '</button>';
      }).join('');
      nav.innerHTML = '<summary>本章目录 · ' + hs.length + ' 节</summary>' +
        '<div class="chapter-toc-links">' + chips + '</div>';
      nav.addEventListener('click', function (e) {
        var b = e.target.closest('button[data-target]');
        if (!b) return;
        var t = document.getElementById(b.getAttribute('data-target'));
        if (t) scrollToEl(t.id);
      });
      var head = bodyEl.querySelector('.sec-head');
      if (head) head.insertAdjacentElement('afterend', nav);
    }

    /* 上一章 / 下一章 */
    var idx = MANIFEST.findIndex(function (c) { return c.id === meta.id; });
    var prev = MANIFEST[idx - 1], next = MANIFEST[idx + 1];
    var pager = document.getElementById('chapter-pager');
    pager.innerHTML =
      (prev ? '<a href="#' + prev.id + '"><span class="k">← 上一章</span><span class="v">' +
        esc(prev.num + ' ' + prev.title) + '</span></a>' : '<span class="spacer-slot"></span>') +
      (next ? '<a class="next" href="#' + next.id + '"><span class="k">下一章 →</span><span class="v">' +
        esc(next.num + ' ' + next.title) + '</span></a>' : '');

    progress.style.width = '0';
  }

  /* 章节内的轻交互：Tab / 排序胶囊（原文档里的静态演示） */
  function bindToggles(scope) {
    Array.prototype.forEach.call(scope.querySelectorAll('.tabs .tab'), function (tab) {
      tab.addEventListener('click', function () {
        Array.prototype.forEach.call(scope.querySelectorAll('.tabs .tab'), function (t) {
          t.setAttribute('aria-selected', 'false');
        });
        tab.setAttribute('aria-selected', 'true');
      });
    });
    Array.prototype.forEach.call(scope.querySelectorAll('.pw-tab'), function (tab) {
      tab.addEventListener('click', function () {
        Array.prototype.forEach.call(scope.querySelectorAll('.pw-tab'), function (t) {
          t.setAttribute('aria-selected', 'false');
        });
        tab.setAttribute('aria-selected', 'true');
      });
    });
    Array.prototype.forEach.call(scope.querySelectorAll('.pw-sort span'), function (s) {
      if (s.textContent.trim() === '|') return;
      s.addEventListener('click', function () {
        Array.prototype.forEach.call(scope.querySelectorAll('.pw-sort span'), function (x) {
          x.classList.remove('on');
        });
        s.classList.add('on');
      });
    });
  }

  /* 播放浮窗演示：最小可用版（播放/暂停、面板展开收起） */
  function initPlayer(scope) {
    var pw = scope.querySelector('#pw');
    if (!pw) return;
    var url = scope.querySelector('#pw-url');
    var side = scope.querySelector('#pw-side');
    var toggle = scope.querySelector('#pw-toggle');
    var xIcon = scope.querySelector('#pw-x-icon');
    var collapsed = pw.classList.contains('collapsed');

    function setCollapsed(next, why) {
      collapsed = next;
      pw.classList.toggle('collapsed', collapsed);
      if (url) {
        url.textContent = collapsed
          ? '播放浮窗 · 面板收起 · 右侧 hover 唤出' + (why ? '（' + why + '）' : '')
          : '播放浮窗 · 面板展开' + (why ? '（' + why + '）' : '');
      }
      if (xIcon) xIcon.setAttribute('href', collapsed ? '#i-chevron-left' : '#i-close');
      var closeBtn = scope.querySelector('#pw-close');
      if (closeBtn) closeBtn.title = collapsed ? '展开信息面板' : '收起面板';
      syncIcon();
    }
    function syncIcon() {
      if (!toggle) return;
      toggle.innerHTML = pw.classList.contains('playing')
        ? '<svg class="icon icon-sm"><use href="#i-pause"/></svg>'
        : '<svg class="icon icon-sm"><use href="#i-play"/></svg>';
    }
    function play() { pw.classList.add('playing'); setCollapsed(true, '开始播放'); }
    function pause(why) { pw.classList.remove('playing'); syncIcon(); if (why) setCollapsed(false, why); }

    ['#pw-play'].forEach(function (sel) {
      var b = scope.querySelector(sel);
      if (b) b.addEventListener('click', play);
    });
    var sim = scope.querySelector('#pw-simulate');
    if (sim) sim.addEventListener('click', function () {
      if (pw.classList.contains('playing')) pause('暂停'); else play();
    });
    if (toggle) toggle.addEventListener('click', function () {
      pw.classList.toggle('playing'); syncIcon();
    });
    ['#pw-expand', '#pw-collapse', '#pw-close'].forEach(function (sel) {
      var b = scope.querySelector(sel);
      if (b) b.addEventListener('click', function () { setCollapsed(sel !== '#pw-expand', '按钮'); });
    });
    if (side) {
      var t = null;
      side.addEventListener('mouseenter', function () { clearTimeout(t); setCollapsed(false, '鼠标进入'); });
      side.addEventListener('mouseleave', function () {
        clearTimeout(t);
        t = setTimeout(function () {
          if (side.contains(document.activeElement)) return;
          if (pw.classList.contains('playing')) setCollapsed(true, '鼠标移出');
        }, 220);
      });
    }
    syncIcon();
  }

  /* ------------------------------------------------------------ 搜索过滤 */
  function matches(meta, q) {
    if (!q) return true;
    if ((meta.num + ' ' + meta.title).toLowerCase().indexOf(q) >= 0) return true;
    if (meta.summary.toLowerCase().indexOf(q) >= 0) return true;
    return (meta.h3 || []).some(function (h) { return h.toLowerCase().indexOf(q) >= 0; });
  }

  function applyFilter() {
    var q = (filterInput.value || '').trim().toLowerCase();
    Array.prototype.forEach.call(tocList.querySelectorAll('a[data-id]'), function (a) {
      var m = BY_ID[a.getAttribute('data-id')];
      a.hidden = !matches(m, q);
    });
    Array.prototype.forEach.call(tocList.querySelectorAll('.toc-group'), function (g) {
      var links = g.querySelectorAll('a[data-id]');
      var visible = Array.prototype.some.call(links, function (a) { return !a.hidden; });
      g.hidden = !visible;
    });
    Array.prototype.forEach.call(document.querySelectorAll('.chapter-card'), function (c) {
      c.hidden = !matches(BY_ID[c.getAttribute('data-id')], q);
    });
    Array.prototype.forEach.call(document.querySelectorAll('.card-group'), function (g) {
      var cards = g.querySelectorAll('.chapter-card');
      var visible = Array.prototype.some.call(cards, function (c) { return !c.hidden; });
      g.hidden = !visible;
    });
  }

  filterInput.addEventListener('input', applyFilter);

  /* ---------------------------------------------------------------- 路由 */
  function route() {
    var id = hashId();
    if (!id || id === 'overview') { showHome(); return; }
    if (BY_ID[id]) { showChapter(BY_ID[id], null); return; }
    /* 章节内的小节锚点：索引里记了它属于哪一章 */
    if (ANCHORS[id] && BY_ID[ANCHORS[id]]) { showChapter(BY_ID[ANCHORS[id]], id); return; }
    var el = document.getElementById(id);
    if (el) {
      if (current) scrollToY(el.getBoundingClientRect().top + window.pageYOffset - 118);
      return;
    }
    if (!current) showHome();
  }

  window.addEventListener('hashchange', route);

  /* `href="#"` 的占位链接不该把页面重置回总览 */
  document.addEventListener('click', function (e) {
    var a = e.target.closest && e.target.closest('a[href="#"]');
    if (a) e.preventDefault();
  });

  /* ------------------------------------------------------------ 进度 / 键盘 */
  function onScroll() {
    if (!current) { progress.style.width = '0'; return; }
    var d = document.documentElement;
    var max = d.scrollHeight - d.clientHeight;
    var p = max > 0 ? (d.scrollTop / max) * 100 : 0;
    progress.style.width = Math.min(100, Math.max(0, p)) + '%';
  }
  window.addEventListener('scroll', onScroll, { passive: true });
  window.addEventListener('resize', function () { fitCols(overview); fitCols(bodyEl); });

  document.addEventListener('keydown', function (e) {
    if (e.key === '/' && document.activeElement !== filterInput &&
        !/^(INPUT|TEXTAREA|SELECT)$/.test((document.activeElement || {}).tagName || '')) {
      e.preventDefault(); filterInput.focus(); return;
    }
    if (e.key === 'Escape') {
      if (document.activeElement === filterInput) {
        filterInput.value = ''; applyFilter(); filterInput.blur();
      } else if (current) {
        location.hash = 'overview';
      }
    }
  });

  /* ---------------------------------------------------------------- 启动 */
  buildNav();
  buildHomeCards();
  mountPreviews();
  route();
})();
