# UX 规范数据（按需加载）

主文档 `../ux-design-preview.html` 只保留**概览与外壳**（约 20 KB），
每章正文存放在这里，点击目录或章节卡片时才加载。

## 文件

| 文件 | 内容 |
| --- | --- |
| `manifest.js` | 章节清单 + 跨章锚点索引 |
| `<id>.js` | 单章数据（22 份） |
| `previews.js` | 首页三张实景预览的 HTML |

单章数据结构：

```js
window.UX_CHAPTERS["pages"] = {
  "id": "pages",
  "num": "12",
  "title": "页面与场景全景",
  "summary": "第 03 节画的是首页一个场景……",
  "group": "页面与组件",
  "kb": 136,
  "sections": 56,
  "h3": ["12.1 搜索", "12.1.1 顶栏下拉建议面板", "…"],
  "html": "<section id=\"pages\">…整章 HTML…</section>"
};
```

- `group` 决定首页卡片与目录分组（设计基础 / 页面与组件 / 落地与附录）
- `h3` 只用于搜索与篇幅统计；正文标题由 `html` 决定
- `html` 里包含表格与 `.stage` 实景预览，样式来自 `../ux-assets/doc.css`
- `manifest.js` 末尾还有 `window.UX_ANCHORS`：小节 id → 所属章节，
  这样 `#e8-flip` 这类跨章小节锚点也能先加载章节再定位

## 为什么是 `.js` 而不是 `.json`

`file://` 下浏览器会拦截 `fetch('x.json')`（每个本地文件被当作唯一源，CORS 不放行）。
所以数据文件用**两行赋值包住一个纯 JSON 对象**，靠 `<script src>` 注入实现按需加载：

```js
window.UX_CHAPTERS = window.UX_CHAPTERS || {};
window.UX_CHAPTERS["pages"] = { …纯 JSON… };
```

去掉前两行的赋值，第二行后半段就是可用 `JSON.parse` 解析的合法 JSON。
如果以后确定只通过本地服务器打开（`python3 -m http.server`），
可以把这些文件直接改名为 `.json`，并把 `doc.js` 的 `loadChapter` 换成 `fetch`。

## 修改

- **改文案 / 表格**：编辑对应 `<id>.js` 里的 `html` 字符串（注意 JSON 转义）；
  也可以重新从归档的单文件版生成。
- **新增一章**：加 `<id>.js`，在 `manifest.js` 的 `UX_MANIFEST` 里补一条；
  目录、首页卡片、上一章/下一章会自动生成。
- **调整顺序 / 分组**：改 manifest 数组顺序与 `group` 字段。

## 目录结构

```
docs/
  ux-design-preview.html          外壳 + 概览（19 KB）
  ux-assets/
    doc.css                       原文档全部设计令牌与预览样式（119 KB）
    shell.css                     阅读器外壳样式（11 KB）
    doc.js                        路由 / 懒加载 / 目录 / 搜索（18 KB）
  ux-data/                        本目录（22 章 + manifest + previews，约 800 KB）
  archive/
    ux-design-preview.full-2026-10-25.html   拆分前的单文件完整版（只读参考）
```
