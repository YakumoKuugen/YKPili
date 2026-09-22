# YKPili 皮肤系统

本目录是应用的**设计 Token 层与控件样式层**，材质移植自 [BewlyCat](https://github.com/keleus/BewlyCat)
（`src/styles/variables.scss`）。所有视觉常量只在这里定义一次，视图一律引用 token。

## 目录结构

```
Styles/
  Theme.axaml                    合并入口（App.axaml 只引用这一个文件）
  Tokens/
    Palette.axaml                品牌色、主题色、状态色（不随主题变化）
    Typography.axaml             字号 / 行高 / 字重 / 字体族
    Dimensions.axaml             圆角 / 间距 / 控件几何
    Motion.axaml                 时长与缓动曲线
    Surfaces.axaml               语义表面（随亮暗变化）
    Shadows.axaml                阴影与玻璃边缘光（随亮暗变化）
  Controls/
    FluentOverrides.axaml        覆盖 Fluent 主题资源键（按钮 / 滚动条外观）
    GlassSurface.axaml           玻璃材质样式类
    TextStyles.axaml             排版样式类
    ButtonStyles.axaml           按钮补充样式
    ScrollBarStyles.axaml        滚动条补充样式
```

## 用法

```xml
<!-- 普通引用：主题相关资源必须用 DynamicResource，否则切换亮暗不会重新求值 -->
<Border Background="{DynamicResource SurfaceContent}"
        CornerRadius="{DynamicResource CardRadius}"
        BoxShadow="{DynamicResource Shadow1}" />

<!-- 玻璃面板 -->
<Border Classes="glass elevated">...</Border>        <!-- 悬浮层级 -->
<Border Classes="glass elevated on-content">...</Border>  <!-- 叠在图片/视频网格之上 -->
<Border Classes="glass modal">...</Border>           <!-- 对话框层级 -->

<!-- 排版 -->
<TextBlock Classes="heading" />   <!-- 20px Bold -->
<TextBlock Classes="title" />     <!-- 15px SemiBold -->
<TextBlock Classes="caption" />   <!-- 12px 三级文字色 -->
<TextBlock Classes="muted" />

<!-- 按钮 -->
<Button Classes="accent large" /> <!-- 主题色主操作 -->
<Button Classes="icon" />         <!-- 圆形图标按钮，尺寸由使用处指定 -->
```

## 换主题色

当前固定为 B 站蓝 `#00A1D6`。改动只需替换 `Tokens/Palette.axaml` 中的
`ThemePrimaryColor` 与 `ThemePrimaryBrush`，以及 `ThemePrimary10Brush` … `ThemePrimary90Brush`
九档透明度梯度（数字表示保留的不透明度），无需触碰任何视图。

## 两个必须注意的坑

**1. XML 注释里不能出现两个连续连字符**

`<!-- 这是 --bew-xxx -->` 会让 XAML 解析直接失败（`AVLN1001`），因为 XML 规范禁止注释内含 `--`。
引用 BewlyCat 的 CSS 变量名时，请写成 `bew-xxx` 而非 `--bew-xxx`。

**2. 代码后台取 token 必须传主题变体**

```csharp
// 正确
this.TryFindResource(key, ActualThemeVariant, out var value)

// 错误：使用 ThemeVariant.Default 解析，全部落空
this.TryFindResource(key, out var value)
```

token 定义在 `Light` / `Dark` 两个 `ThemeDictionary` 中，不传变体的重载会一律查找失败，
导致阴影、文字色等主题相关资源静默回退到默认值。

## 已知限制：没有 backdrop-filter

Avalonia 12 的 `Visual.Effect` 只支持 `DropShadowDirectionEffect`，**没有 `BlurEffect`**，
也没有自定义 Effect 扩展点；模糊能力仅剩 `BoxShadow.Blur`（只模糊阴影本身）与
`WindowTransparencyLevel`（只模糊窗口后方的桌面）。

因此 BewlyCat 的 `blur(20px) saturate(180%)` **无法**移植。本套材质的替代方案是四层叠加：

1. 半透明填充（`SurfaceElevated` / `SurfaceElevatedStrong`）
2. 1px 半透明亮边（`BorderPopover`）
3. **inset 白色高光**（`ShadowGlass*` 的 inset 段）—— 玻璃质感的主要来源
4. 外阴影定层次（`ShadowGlass*` 的 outset 段）

其中第 3 条与 BewlyCat 同源（它也是用 inset 阴影"模仿"磨砂边缘），因此可精确复刻；
真正缺失的只是背景内容的实时模糊。

**推论**：面板叠在高频内容（视频网格、图片）之上时，必须使用 `on-content` 变体把填充
提高到 0.92 不透明度，否则背后的画面会直接干扰文字阅读——没有模糊可以依赖。

## 验证方式

```bash
# 构建（dotnet 不在 PATH；不关遥测会因沙箱拒绝写 ~/Library/Application Support 而失败）
AVALONIA_TELEMETRY_OPTOUT=1 /usr/local/share/dotnet/dotnet build YKPili.Desktop/YKPili.Desktop.csproj

# 运行
AVALONIA_TELEMETRY_OPTOUT=1 /usr/local/share/dotnet/dotnet run --project YKPili.Desktop/YKPili.Desktop.csproj
```

各页面在亮暗双主题下的渲染效果见 `docs/skin-preview/`。
