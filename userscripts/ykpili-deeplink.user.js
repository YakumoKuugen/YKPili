// ==UserScript==
// @name         YKPili 深链接管（B 站视频）
// @namespace    https://github.com/ykpili/ykpili
// @version      1.2.0
// @description  在浏览器打开 B 站视频时，用 ykpili:// 转交给 YKPili 客户端播放
// @author       YKPili
// @match        *://*/*
// @grant        GM_getValue
// @grant        GM_setValue
// @grant        GM_registerMenuCommand
// @grant        window.close
// @run-at       document-start
// @noframes
// ==/UserScript==

(function () {
    'use strict';

    // ======================= 配置 =======================

    const DEFAULT_SCHEME = 'ykpili';
    const MODES = ['auto', 'confirm', 'off'];
    const MODE_LABEL = {
        auto: '自动接管',
        confirm: '只拦截点击',
        off: '关闭',
    };

    function readSetting(key, fallback) {
        try {
            if (typeof GM_getValue === 'function') return GM_getValue(key, fallback);
            const value = localStorage.getItem('ykpili:' + key);
            return value === null ? fallback : value;
        } catch (_) {
            return fallback;
        }
    }

    function writeSetting(key, value) {
        try {
            if (typeof GM_setValue === 'function') GM_setValue(key, value);
            else localStorage.setItem('ykpili:' + key, value);
        } catch (_) {
            /* 隐私模式下写不进去就算了，本次会话仍然生效 */
        }
    }

    let mode = readSetting('mode', 'auto');
    if (!MODES.includes(mode)) mode = 'auto';
    let closeTab = readSetting('closeTab', true);
    if (closeTab === 'false' || closeTab === 0) closeTab = false;
    const scheme = String(readSetting('scheme', DEFAULT_SCHEME)).replace(/[^a-z0-9+.-]/gi, '') || DEFAULT_SCHEME;
    const APP_NAME = 'YKPili';

    // ======================= URL 识别 =======================

    function safeUrl(value) {
        try {
            return new URL(value, location.href);
        } catch (_) {
            return null;
        }
    }

    function isShortHost(host) {
        return host === 'b23.tv' || host === 'bili2233.cn';
    }

    function isBilibiliHost(host) {
        return host === 'bilibili.com' || host.endsWith('.bilibili.com');
    }

    function videoIdOf(url) {
        const match = url.pathname.match(/\/video\/(BV[0-9A-Za-z]{10}|av\d+)/i);
        if (match) return match[1];
        const queryBvid = url.searchParams.get('bvid');
        if (queryBvid && /^BV[0-9A-Za-z]{10}$/i.test(queryBvid)) return queryBvid;
        return null;
    }

    function isVideoUrl(value) {
        const url = safeUrl(value);
        if (!url) return false;
        if (isShortHost(url.hostname)) return true;
        if (!isBilibiliHost(url.hostname)) return false;
        return Boolean(videoIdOf(url));
    }

    /**
     * 把网页 URL 收敛成客户端深链。
     *
     * 普通视频：ykpili://video/BV...?p=2&t=90
     * 短链：ykpili://open?url=https%3A%2F%2Fb23.tv%2F...
     * 列表页的 ?bvid=：也能识别。
     */
    function buildDeepLink(value) {
        const url = safeUrl(value);
        if (!url) return null;

        if (isShortHost(url.hostname)) {
            return scheme + '://open?url=' + encodeURIComponent(url.href);
        }

        if (!isBilibiliHost(url.hostname)) return null;
        const id = videoIdOf(url);
        if (!id) return null;

        let link = scheme + '://video/' + id;
        const params = [];
        const page = url.searchParams.get('p');
        if (page && Number(page) > 1) params.push('p=' + encodeURIComponent(page));
        const time = url.searchParams.get('t') || url.searchParams.get('start');
        if (time) params.push('t=' + encodeURIComponent(time));
        if (params.length) link += '?' + params.join('&');
        return link;
    }

    // ======================= 启动动作 =======================

    function launchWithGesture(link) {
        // 用户点击的同一个事件循环里点一个隐藏 <a>，大多数浏览器会把它
        // 当成用户手势，允许唤起外部应用；未安装时也只是留在当前页。
        const anchor = document.createElement('a');
        anchor.href = link;
        anchor.rel = 'noopener';
        anchor.style.cssText = 'position:fixed;left:-9999px;top:-9999px;width:1px;height:1px;opacity:0;';
        (document.body || document.documentElement).appendChild(anchor);
        try {
            anchor.click();
        } finally {
            setTimeout(() => anchor.remove(), 0);
        }
        armCloseAfterLaunch();
    }

    function launchSilent(link) {
        // 页面级自动接管没有用户手势：用隐藏 iframe 尝试唤起，失败也不会
        // 把当前 B 站页面导航走。
        const frame = document.createElement('iframe');
        frame.src = link;
        frame.setAttribute('aria-hidden', 'true');
        frame.style.cssText =
            'position:fixed;left:-9999px;top:-9999px;width:1px;height:1px;border:0;opacity:0;pointer-events:none;';
        (document.body || document.documentElement).appendChild(frame);
        setTimeout(() => frame.remove(), 1800);
        armCloseAfterLaunch();
    }

    // ======================= 打开后关闭标签 =======================

    // 不能凭「发起唤起」就关标签：客户端没装 / 被拦时会把用户正在看的
    // 视频页也关掉。这里等浏览器窗口真的失焦（切到客户端）后再关；
    // 若 5 秒内没有切换，就撤掉监听，绝不误关。
    const CLOSE_AFTER_BLUR_MS = 500;
    const CLOSE_WATCH_MS = 5000;
    let closeWatch = null;

    function closeCurrentTab() {
        try {
            if (typeof GM_closeTab === 'function') {
                GM_closeTab();
                return;
            }
        } catch (_) {
            /* fall through */
        }
        try {
            window.close();
        } catch (_) {
            /* 浏览器不允许关闭；例如这是窗口里最后一个标签 */
        }
    }

    function disarmCloseWatch() {
        if (!closeWatch) return;
        clearTimeout(closeWatch.timer);
        clearTimeout(closeWatch.deadline);
        clearInterval(closeWatch.poll);
        window.removeEventListener('blur', closeWatch.onLeave);
        window.removeEventListener('focus', closeWatch.onReturn);
        document.removeEventListener('visibilitychange', closeWatch.onVisibility);
        closeWatch = null;
    }

    function armCloseAfterLaunch() {
        if (!closeTab) return;
        // 后台标签（用户没在看）不猜：否则 document.hasFocus() 一开始就是 false，
        // 会在客户端还没起来时把标签关掉。
        if (!document.hasFocus()) return;
        disarmCloseWatch();

        const state = { timer: 0, deadline: 0, poll: 0 };
        const leave = () => {
            if (!closeTab || closeWatch !== state || state.timer) return;
            state.timer = setTimeout(() => {
                if (closeWatch !== state) return;
                disarmCloseWatch();
                closeCurrentTab();
            }, CLOSE_AFTER_BLUR_MS);
        };
        const back = () => {
            if (closeWatch !== state) return;
            // 用户回到浏览器：撤销这次关闭，别把标签抢走。
            clearTimeout(state.timer);
            state.timer = 0;
        };
        const onVisibility = () => {
            if (document.visibilityState === 'hidden') leave();
            else back();
        };

        state.onLeave = leave;
        state.onReturn = back;
        state.onVisibility = onVisibility;
        closeWatch = state;

        window.addEventListener('blur', leave);
        window.addEventListener('focus', back);
        document.addEventListener('visibilitychange', onVisibility);
        // 有些浏览器切到外部应用时不一定派发 blur，用低频轮询兜底。
        state.poll = setInterval(() => {
            if (document.visibilityState === 'hidden' || !document.hasFocus()) leave();
        }, 250);
        state.deadline = setTimeout(disarmCloseWatch, CLOSE_WATCH_MS);
    }

    // ======================= 页面提示（Shadow DOM，避免被 B 站样式污染） =======================

    let shadowRoot = null;
    let cardTimer = 0;

    function ensureShadow() {
        if (shadowRoot && shadowRoot.host && shadowRoot.host.isConnected) return shadowRoot;
        shadowRoot = null;
        if (!document.documentElement) return null;

        let host = document.getElementById('ykpili-deeplink-host');
        if (!host) {
            host = document.createElement('div');
            host.id = 'ykpili-deeplink-host';
            host.style.cssText = 'all:initial;position:fixed;top:0;right:0;z-index:2147483647;';
            document.documentElement.appendChild(host);
            shadowRoot = host.attachShadow({ mode: 'open' });
            shadowRoot.innerHTML = `
                <style>
                    :host { all: initial; }
                    .card {
                        box-sizing: border-box;
                        width: 340px;
                        margin: 18px;
                        padding: 14px 16px;
                        border-radius: 16px;
                        background: rgba(24, 25, 30, 0.96);
                        border: 1px solid rgba(255, 255, 255, 0.12);
                        box-shadow: 0 18px 48px rgba(0, 0, 0, 0.42);
                        color: #f4f5f7;
                        font: 13px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI",
                              "PingFang SC", "Microsoft YaHei", sans-serif;
                        backdrop-filter: blur(18px);
                    }
                    .row { display: flex; align-items: center; gap: 10px; }
                    .badge {
                        width: 30px; height: 30px; border-radius: 50%;
                        display: flex; align-items: center; justify-content: center;
                        background: rgba(0, 161, 214, 0.18); color: #4cc3f0;
                        font-size: 16px; flex: 0 0 auto;
                    }
                    .title { font-weight: 600; font-size: 13px; }
                    .detail {
                        margin: 6px 0 10px;
                        color: rgba(255, 255, 255, 0.6);
                        font-size: 11px;
                        word-break: break-all;
                    }
                    .actions { display: flex; gap: 8px; flex-wrap: wrap; }
                    button {
                        appearance: none; border: 1px solid rgba(255, 255, 255, 0.14);
                        background: rgba(255, 255, 255, 0.08); color: #f4f5f7;
                        border-radius: 999px; padding: 5px 12px; font: inherit;
                        font-size: 12px; cursor: pointer;
                    }
                    button.primary { background: #00a1d6; border-color: #00a1d6; color: #fff; }
                    button:hover { filter: brightness(1.08); }
                    .hint { margin-top: 8px; color: rgba(255, 255, 255, 0.38); font-size: 10px; }
                </style>
                <div class="card" hidden>
                    <div class="row">
                        <div class="badge">↗</div>
                        <div class="title"></div>
                    </div>
                    <div class="detail"></div>
                    <div class="actions"></div>
                    <div class="hint"></div>
                </div>
            `;
        } else {
            shadowRoot = host.shadowRoot || host.attachShadow({ mode: 'open' });
        }
        return shadowRoot;
    }

    function hideCard() {
        if (!shadowRoot) return;
        const card = shadowRoot.querySelector('.card');
        if (card) card.hidden = true;
    }

    /**
     * @param {{title: string, detail?: string, primary?: {label: string, onClick: Function},
     *          secondary?: {label: string, onClick: Function}, hint?: string, autoHide?: number}} options
     */
    function showCard(options) {
        const root = ensureShadow();
        if (!root) return;
        const card = root.querySelector('.card');
        if (!card) return;

        card.querySelector('.title').textContent = options.title || APP_NAME;
        card.querySelector('.detail').textContent = options.detail || '';
        card.querySelector('.hint').textContent = options.hint || '';

        const actions = card.querySelector('.actions');
        actions.textContent = '';
        const addButton = (label, primary, onClick) => {
            const button = document.createElement('button');
            button.textContent = label;
            if (primary) button.className = 'primary';
            button.addEventListener('click', (event) => {
                event.preventDefault();
                event.stopPropagation();
                onClick();
            });
            actions.appendChild(button);
        };

        if (options.primary) addButton(options.primary.label, true, options.primary.onClick);
        if (options.secondary) addButton(options.secondary.label, false, options.secondary.onClick);
        if (!options.primary && !options.secondary) addButton('知道了', false, hideCard);

        card.hidden = false;
        clearTimeout(cardTimer);
        cardTimer = setTimeout(hideCard, options.autoHide == null ? 8000 : options.autoHide);
    }

    // ======================= 自动接管 =======================

    function bypassKey(id) {
        return 'ykpili:bypass:' + id;
    }

    const BYPASS_ALL_KEY = 'ykpili:bypass-all-until';

    function isBypassed(id) {
        try {
            if (sessionStorage.getItem(bypassKey(id)) === '1') return true;
            // 短链跳转后 pathname 会变，单条 bypass 跟不上；
            // 「继续加载网页」会写一个 2 分钟的全局免打扰窗口。
            return Number(sessionStorage.getItem(BYPASS_ALL_KEY) || 0) > Date.now();
        } catch (_) {
            return false;
        }
    }

    function bypass(id) {
        try {
            sessionStorage.setItem(bypassKey(id), '1');
        } catch (_) {
            /* ignore */
        }
    }

    function bypassAll() {
        try {
            sessionStorage.setItem(BYPASS_ALL_KEY, String(Date.now() + 120000));
        } catch (_) {
            /* ignore */
        }
    }

    function currentVideoId() {
        const url = safeUrl(location.href);
        if (!url) return null;
        if (isShortHost(url.hostname)) return url.pathname;
        return videoIdOf(url);
    }

    // ======================= 加载前拦截 =======================

    function renderEarlyHandoff(deepLink, id) {
        if (!document.documentElement) return;

        const page = document.createElement('div');
        page.id = 'ykpili-early-handoff';
        page.style.cssText = [
            'position:fixed',
            'inset:0',
            'z-index:2147483647',
            'display:flex',
            'align-items:center',
            'justify-content:center',
            'padding:20px',
            'box-sizing:border-box',
            'background:#0e0f13',
            'color:#f4f5f7',
            'font:14px/1.6 -apple-system,BlinkMacSystemFont,"Segoe UI","PingFang SC","Microsoft YaHei",sans-serif',
        ].join(';');

        page.innerHTML = `
            <div style="width:min(440px,100%);padding:28px;border:1px solid rgba(255,255,255,.12);border-radius:20px;background:#191a20;box-shadow:0 24px 64px rgba(0,0,0,.45);">
                <div style="width:42px;height:42px;margin-bottom:16px;border-radius:50%;display:flex;align-items:center;justify-content:center;background:rgba(0,161,214,.18);color:#4cc3f0;font-size:20px;">↗</div>
                <div style="font-size:18px;font-weight:600;margin-bottom:6px;">正在用 ${APP_NAME} 打开</div>
                <div style="color:rgba(255,255,255,.55);margin-bottom:16px;">已拦截网页加载，正在尝试唤起客户端。</div>
                <div data-role="detail" style="padding:10px 12px;border-radius:10px;background:rgba(255,255,255,.05);color:rgba(255,255,255,.66);font-size:12px;word-break:break-all;margin-bottom:18px;"></div>
                <div style="display:flex;gap:10px;flex-wrap:wrap;">
                    <button data-action="open" style="appearance:none;border:1px solid #00a1d6;background:#00a1d6;color:#fff;border-radius:999px;padding:8px 16px;font:inherit;font-size:13px;cursor:pointer;">打开 ${APP_NAME}</button>
                    <button data-action="continue" style="appearance:none;border:1px solid rgba(255,255,255,.16);background:rgba(255,255,255,.06);color:#f4f5f7;border-radius:999px;padding:8px 16px;font:inherit;font-size:13px;cursor:pointer;">继续加载网页</button>
                </div>
                <div style="margin-top:14px;color:rgba(255,255,255,.38);font-size:11px;">没有反应？确认已安装 ${APP_NAME}，或点「继续加载网页」。</div>
            </div>
        `;
        page.querySelector('[data-role="detail"]').textContent = deepLink;
        page.querySelector('[data-action="open"]').addEventListener('click', (event) => {
            event.preventDefault();
            launchWithGesture(deepLink);
        });
        page.querySelector('[data-action="continue"]').addEventListener('click', (event) => {
            event.preventDefault();
            bypassAll();
            if (id) bypass(id);
            location.reload();
        });
        document.documentElement.appendChild(page);
    }

    /**
     * 在 DOM 还在解析时就把 B 站视频页拦下来。
     *
     * 用户脚本本身无法阻止浏览器发出第一个 HTML 请求，但 document-start
     * 阶段调用 `window.stop()` 可以阻止后续 HTML / JS / 图片继续加载与执行，
     * 页面不会真正跑起来，然后把控制权交给客户端。
     */
    function earlyInterceptBeforeLoad() {
        if (mode !== 'auto') return false;
        if (window.top !== window) return false;
        if (document.readyState !== 'loading') return false;

        const url = safeUrl(location.href);
        if (!url) return false;
        if (!isBilibiliHost(url.hostname) && !isShortHost(url.hostname)) return false;
        if (url.searchParams.get('ykpili') === '0') return false;

        const id = currentVideoId();
        if (!id || isBypassed(id)) return false;

        const deepLink = buildDeepLink(location.href);
        if (!deepLink) return false;

        // 已经发出的首包无法撤回，但剩下的事件流 / 解析 / 脚本可以立刻掐断。
        try {
            window.stop();
        } catch (_) {
            /* 某些沙箱环境不允许，继续走提示页 */
        }
        try {
            document.documentElement.style.background = '#0e0f13';
            document.documentElement.style.colorScheme = 'dark';
        } catch (_) {
            /* ignore */
        }

        renderEarlyHandoff(deepLink, id);
        // 留一帧让提示页先画出来，再尝试唤起；客户端起来后标签会被关掉。
        setTimeout(() => launchSilent(deepLink), 60);
        return true;
    }

    function maybeAutoOpen() {
        if (mode !== 'auto') return;
        if (window.top !== window) return;

        const url = safeUrl(location.href);
        if (!url) return;
        // 脚本在所有网页上运行（为了拦截外站里的 B 站链接），
        // 但只有 B 站 / 短链页面才做「打开页面自动唤起」。
        if (!isBilibiliHost(url.hostname) && !isShortHost(url.hostname)) return;
        // 显式退出参数：?ykpili=0
        if (url.searchParams.get('ykpili') === '0') return;

        const deepLink = buildDeepLink(location.href);
        if (!deepLink) return;

        const id = currentVideoId();
        if (!id || isBypassed(id)) return;

        let key;
        try {
            key = 'ykpili:auto:' + location.pathname + location.search;
            if (sessionStorage.getItem(key) === '1') return;
            sessionStorage.setItem(key, '1');
        } catch (_) {
            /* sessionStorage 不可用时仍然自动打开一次 */
        }

        setTimeout(() => {
            launchSilent(deepLink);
            showCard({
                title: '已尝试用 ' + APP_NAME + ' 打开',
                detail: deepLink,
                primary: {
                    label: '再次打开',
                    onClick: () => launchWithGesture(deepLink),
                },
                secondary: {
                    label: '留在浏览器',
                    onClick: () => {
                        disarmCloseWatch();
                        bypass(id);
                        hideCard();
                    },
                },
                hint: '切到 ' + APP_NAME + ' 后会自动关闭这个标签；没有反应就点「再次打开」。',
                autoHide: 12000,
            });
        }, 450);
    }

    // ======================= 点击拦截 =======================

    function findVideoAnchor(target) {
        let node = target;
        while (node && node !== document) {
            if (node.tagName === 'A' && node.href) {
                if (node.hasAttribute('download')) return null;
                return node;
            }
            node = node.parentNode;
        }
        return null;
    }

    function onClick(event) {
        if (mode === 'off') return;
        if (event.defaultPrevented || event.button !== 0) return;
        if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
        const target = event.target;
        if (target && typeof target.closest === 'function' && target.closest('#ykpili-deeplink-host')) {
            return;
        }

        const anchor = findVideoAnchor(target);
        if (!anchor) return;
        // 新标签页 / 下载链接尊重浏览器默认行为。
        if (anchor.target && anchor.target !== '' && anchor.target !== '_self') return;
        if (!isVideoUrl(anchor.href)) return;

        const deepLink = buildDeepLink(anchor.href);
        if (!deepLink) return;

        event.preventDefault();
        event.stopImmediatePropagation();

        launchWithGesture(deepLink);
        showCard({
            title: '已在 ' + APP_NAME + ' 中打开',
            detail: deepLink,
            primary: {
                label: '再次打开',
                onClick: () => launchWithGesture(deepLink),
            },
            secondary: {
                label: '留在浏览器',
                onClick: () => {
                    disarmCloseWatch();
                    hideCard();
                    window.location.href = anchor.href;
                },
            },
            hint: '切到 ' + APP_NAME + ' 后会自动关闭这个标签；按住 Alt 点击可跳过接管。',
            autoHide: 9000,
        });
    }

    // ======================= 菜单 =======================

    function cycleMode() {
        const index = MODES.indexOf(mode);
        mode = MODES[(index + 1) % MODES.length];
        writeSetting('mode', mode);
        showCard({
            title: '接管模式：' + MODE_LABEL[mode],
            detail: mode === 'auto'
                ? '打开视频页会自动尝试唤起客户端，并拦截站内视频链接点击。'
                : mode === 'confirm'
                    ? '只在点击视频链接时接管，直接打开视频页不自动唤起。'
                    : '不再接管任何链接。',
            primary: { label: '知道了', onClick: hideCard },
            autoHide: 6000,
        });
    }

    function registerMenu() {
        if (typeof GM_registerMenuCommand !== 'function') return;
        GM_registerMenuCommand('切换接管模式（当前：' + MODE_LABEL[mode] + '）', cycleMode);
        GM_registerMenuCommand('切换：打开后关闭标签（当前：' + (closeTab ? '开' : '关') + '）', () => {
            closeTab = !closeTab;
            writeSetting('closeTab', closeTab);
            if (!closeTab) disarmCloseWatch();
            showCard({
                title: '打开后关闭标签：' + (closeTab ? '开' : '关'),
                detail: closeTab
                    ? '切到客户端后自动关闭当前标签。浏览器不允许关闭窗口里的最后一个标签。'
                    : '客户端打开后保留当前标签。',
                primary: { label: '知道了', onClick: hideCard },
                autoHide: 6000,
            });
        });
        GM_registerMenuCommand('打开当前视频到 ' + APP_NAME, () => {
            const deepLink = buildDeepLink(location.href);
            if (deepLink) launchWithGesture(deepLink);
            else showCard({ title: '当前页面不是 B 站视频', primary: { label: '知道了', onClick: hideCard } });
        });
        GM_registerMenuCommand('恢复自动接管', () => {
            mode = 'auto';
            writeSetting('mode', mode);
            showCard({ title: '已恢复自动接管', primary: { label: '知道了', onClick: hideCard } });
        });
    }

    // ======================= 启动 =======================

    // 先尝试「加载前拦截」：命中就不再挂常规点击监听，也不等 DOMContentLoaded。
    const interceptedEarly = earlyInterceptBeforeLoad();

    const boot = () => {
        if (window.top !== window) return;
        // 挂在 window 捕获阶段：比 B 站自己的 document 委托更早拿到事件。
        window.addEventListener('click', onClick, true);
        maybeAutoOpen();
    };

    if (!interceptedEarly) {
        if (document.readyState === 'loading') {
            // document-start 时 body 还不存在；自动接管等 DOM 就绪后再执行，
            // 避免往 <head> 里塞 iframe。
            document.addEventListener('DOMContentLoaded', boot, { once: true });
        } else {
            boot();
        }
    }

    registerMenu();
})();
