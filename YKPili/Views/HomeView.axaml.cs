// 引入基础类型、IO、HTTP、异步、集合
using System;
using System.Collections.Generic;
using System.IO;
using System.Net.Http;
using System.Threading;
using System.Threading.Tasks;
// 引入 Avalonia 布局与媒体类型
using Avalonia;
using Avalonia.Animation;
using Avalonia.Controls;
using Avalonia.Layout;
using Avalonia.Media;
using Avalonia.Media.Imaging;
using Avalonia.Media.Transformation;
// 引入项目内模型、缓存与 ViewModel
using YKPili.Models;
using YKPili.Services.Cache;
using YKPili.ViewModels;

namespace YKPili.Views;

/// <summary>
/// HomeView:首页视图
/// 负责在 Canvas 上以"虚拟列表 + 容器池"模式渲染推荐视频卡片,
/// 并处理滚动、尺寸变化、图片懒加载等交互细节。
/// </summary>
public partial class HomeView : UserControl
{
    // 绑定的 ViewModel 引用
    private HomeViewModel? _vm;
    // 用于跨实例点击转发的静态引用(让代码后台创建的事件能调用到当前 VM 的方法)
    private static HomeViewModel? _homeVm;
    // 单元格尺寸(从 ViewModel 常量中复制,便于访问)
    private double _cellWidth;
    private double _cellHeight;
    // 上一次记录的视口尺寸(用于检测尺寸变化)
    private double _lastViewportWidth;
    private double _lastViewportHeight;
    // 上一次重建时的视口尺寸(用于判断是否需要重建)
    private double _lastRebuildWidth;
    private double _lastRebuildHeight;
    // 防止在更新过程中重入的标志
    private bool _isUpdating;
    // 标记是否已完成首次布局
    private bool _initialized;

    // 共享 HttpClient:用于下载封面图(超时 10s)
    private static readonly HttpClient _httpClient = new() { Timeout = TimeSpan.FromSeconds(10) };
    // 并发下载限流:同时最多下载 4 张图片
    private static readonly SemaphoreSlim _imageLoadSemaphore = new(4, 4);
    // 内存图片缓存:URL -> Bitmap
    private readonly Dictionary<string, Bitmap?> _imageCache = new();
    // 进行中的下载:URL -> 加载任务。
    // 用共享任务而非"下载中标记集合",这样多个控件需要同一张图时都能等到结果,
    // 不会出现"图已下载完、却因为当初被跳过而没有任何控件显示"的漏图。
    private readonly Dictionary<string, Task<Bitmap?>> _imageLoadTasks = new();

    /// <summary>
    /// 封面解码目标宽度(像素)。
    /// 卡片显示宽度为 192 逻辑像素,按 2 倍屏取 384。B 站封面原图通常在 480 以上,
    /// 按原尺寸解码会在容器池上百个时明显增加解码耗时与内存占用。
    /// </summary>
    private const int CoverDecodeWidth = 384;

    /// <summary>
    /// 记录每个封面 Image 控件当前已绑定的图片 URL。
    ///
    /// 容器池会复用控件,因此**不能**用 img.Source 是否为 null 来判断"是否已加载"——
    /// 复用后 img.Source 上残留的是上一个视频的封面,会把新视频误判成"已加载"
    /// 而跳过网络请求,导致新封面永远不出现。
    /// </summary>
    private static readonly AttachedProperty<string?> BoundImageUrlProperty =
        AvaloniaProperty.RegisterAttached<HomeView, Image, string?>("BoundImageUrl");

    public HomeView()
    {
        InitializeComponent();
    }

    /// <summary>
    /// 主题(亮/暗)切换回调:
    /// 卡片控件是在代码后台按当时的 token 值创建的,主题切换后必须重建,
    /// 否则圆角、阴影、文字色会停留在旧主题。
    /// </summary>
    private void OnThemeVariantChanged(object? sender, EventArgs e)
    {
        if (_initialized)
            RebuildAllControls();
    }

    /// <summary>
    /// 从设计 token 读取一个资源值,找不到时回退到调用方给的默认值。
    /// 卡片是在代码后台动态创建的,无法在 XAML 里用 DynamicResource,
    /// 因此这里统一走资源查找,保证几何与排版仍然只有一个来源。
    ///
    /// 注意:必须显式传入 ActualThemeVariant。
    /// 不带主题变体的重载使用 ThemeVariant.Default 解析,
    /// 而 token 定义在 Light/Dark 两个 ThemeDictionary 中,
    /// 会导致所有随主题变化的资源(阴影、文字色等)全部查找失败。
    /// </summary>
    private T Res<T>(string key, T fallback)
    {
        if (this.TryFindResource(key, ActualThemeVariant, out var value) && value is T typed)
            return typed;
        return fallback;
    }

    // DataContext 变化时绑定事件
    protected override void OnDataContextChanged(EventArgs e)
    {
        base.OnDataContextChanged(e);

        if (DataContext is HomeViewModel vm)
        {
            _vm = vm;
            _homeVm = vm;
            _cellWidth = HomeViewModel.CellWidth;
            _cellHeight = HomeViewModel.CellHeight;

            // 订阅滚动与数据变化
            ScrollHost.ScrollChanged += OnScrollChanged;
            // 监听容器布局:首次布局时完成初始化
            ContainerCanvas.LayoutUpdated += OnInitialLayout;
            vm.DataChanged += OnDataChanged;
            // 主题切换后卡片需要重建才能取到新的 token 值(几何/阴影是创建时读取的)
            ActualThemeVariantChanged += OnThemeVariantChanged;
        }
    }

    /// <summary>
    /// 首次布局回调:此时视口尺寸已确定,可以初始化容器。
    /// </summary>
    private void OnInitialLayout(object? sender, EventArgs e)
    {
        if (_vm == null || _initialized) return;

        double viewportWidth = ScrollHost.Viewport.Width;
        double viewportHeight = ScrollHost.Viewport.Height;

        if (viewportWidth <= 0 || viewportHeight <= 0) return;

        _initialized = true;
        ContainerCanvas.LayoutUpdated -= OnInitialLayout;

        // 让 VM 按视口尺寸推导列数/行数并初始化容器池,然后再创建对应的 Avalonia 控件
        _vm.InitializeContainers(viewportWidth, viewportHeight);
        ContainerCanvas.Width = _cellWidth * HomeViewModel.ColumnsPerRow;
        ContainerCanvas.Height = _cellHeight * _vm.TotalRows;
        // 视口确定后立即评估一次:若已有数据不足以填满首屏,需要主动请求更多
        _vm.EvaluateLoadMore();
        BuildContainerControls();
        RepositionAll();

        // 记录尺寸用于后续尺寸变化检测
        _lastViewportWidth = viewportWidth;
        _lastViewportHeight = viewportHeight;
        _lastRebuildWidth = viewportWidth;
        _lastRebuildHeight = viewportHeight;
        // 此后改用 OnLayoutUpdatedForResize 监听尺寸变化
        ScrollHost.LayoutUpdated += OnLayoutUpdatedForResize;
    }

    /// <summary>为 VM 中每个可见容器创建对应的 Avalonia Border 控件</summary>
    private void BuildContainerControls()
    {
        if (_vm == null) return;

        ContainerCanvas.Children.Clear();

        foreach (var item in _vm.VisibleContainers)
            ContainerCanvas.Children.Add(CreateCoverControl(item));

        LoadMissingImages();
    }

    /// <summary>
    /// 创建单个封面卡片控件:
    ///   - 上方:封面图(带占位色背景,无图时显示纯色)
    ///   - 下方:标题(2 行省略)
    /// 整个外层 Border 响应点击,触发播放。
    /// </summary>
    private Control CreateCoverControl(CoverItem item)
    {
        // ---- 从设计 token 读取几何与排版(避免魔法数字) ----
        CornerRadius mediaRadius = Res("MediaRadius", new CornerRadius(12));
        CornerRadius cardRadius = Res("CardRadius", new CornerRadius(12));
        double titleFontSize = Res("FontSizeControl", 13.0);
        double titleLineHeight = Res("LineHeightControl", 18.0);
        double titleGap = Res("Space1", 4.0);
        BoxShadows shadowHover = Res("ShadowCardHover", default(BoxShadows));
        TimeSpan hoverDuration = Res("DurationFast", TimeSpan.FromMilliseconds(150));

        // 封面容器:圆角矩形,带占位色作为加载前的背景
        var imageContainer = new Border
        {
            Height = HomeViewModel.ItemHeight,
            Width = HomeViewModel.ItemWidth,
            CornerRadius = mediaRadius,
            // 让封面图按圆角裁剪——原先未开启,导致圆角只作用于占位底色而图片仍是方角
            ClipToBounds = true,
            HorizontalAlignment = HorizontalAlignment.Center
        };
        // 把字符串形式的占位色绑定到 Border 背景(由 HexColorConverter 转为 SolidColorBrush)
        imageContainer.Bind(Border.BackgroundProperty,
            new Avalonia.Data.Binding("PlaceholderColor")
            {
                Converter = new HexColorConverter()
            });

        // 封面图:图片会异步加载并设置 Source
        var coverImage = new Image
        {
            Width = HomeViewModel.ItemWidth,
            Height = HomeViewModel.ItemHeight,
            Stretch = Stretch.UniformToFill
        };
        coverImage.IsHitTestVisible = false; // 不参与命中测试,由外层 Border 处理点击
        imageContainer.Child = coverImage;

        // 标题文本(字体族与前景色由全局 TextBlock 样式统一提供,此处只设卡片专用尺寸)
        var titleBlock = new TextBlock
        {
            HorizontalAlignment = HorizontalAlignment.Center,
            Margin = new Thickness(0, titleGap + 2, 0, 0),
            FontSize = titleFontSize,
            LineHeight = titleLineHeight,
            TextWrapping = TextWrapping.Wrap,
            MaxWidth = HomeViewModel.ItemWidth,
            // 用 MaxLines 而不是 MaxHeight 来限制行数:
            // MaxHeight 与 TextTrimming 组合会触发 Avalonia 文本布局异常
            // (ShapedTextRun.Split 抛 "requested length N consumes entire run"),
            // 标题含 emoji 等代理对字符时尤其容易复现。
            MaxLines = 2,
            TextTrimming = TextTrimming.CharacterEllipsis
        };
        titleBlock.Bind(TextBlock.TextProperty,
            new Avalonia.Data.Binding("Title"));

        // 上下堆叠:封面 + 标题
        var stack = new StackPanel
        {
            HorizontalAlignment = HorizontalAlignment.Center,
            Children = { imageContainer, titleBlock }
        };

        // 外层 Border:接收点击,显示手型光标,并负责 hover 浮起
        var outerBorder = new Border
        {
            Margin = new Thickness(8),
            Padding = new Thickness(0),
            CornerRadius = cardRadius,
            Background = Brushes.Transparent,
            Child = stack,
            DataContext = item,
            Cursor = new Avalonia.Input.Cursor(Avalonia.Input.StandardCursorType.Hand),
            // 静止态无阴影;悬停时抬升并加深阴影(BewlyCat 卡片同样以阴影表达抬升)
            RenderTransform = TransformOperations.Parse("scale(1)"),
            Transitions = new Transitions
            {
                new BoxShadowsTransition { Property = Border.BoxShadowProperty, Duration = hoverDuration },
                new TransformOperationsTransition { Property = Visual.RenderTransformProperty, Duration = hoverDuration }
            }
        };

        // hover 浮起:阴影加深 + 轻微放大。缩放不参与布局,因此不影响 Canvas 定位与虚拟列表计算。
        outerBorder.PointerEntered += (_, _) =>
        {
            outerBorder.BoxShadow = shadowHover;
            outerBorder.RenderTransform = TransformOperations.Parse("scale(1.03)");
        };
        outerBorder.PointerExited += (_, _) =>
        {
            outerBorder.BoxShadow = default;
            outerBorder.RenderTransform = TransformOperations.Parse("scale(1)");
        };
        outerBorder.Tapped += (_, _) =>
        {
            // 点击时从 DataContext 拿到对应的 CoverItem 并请求播放
            var dc = outerBorder.DataContext as CoverItem;
            System.Diagnostics.Trace.WriteLine($"[HomeView] Tapped - DataContext Bvid={dc?.Bvid}, Title={dc?.Title}");
            if (dc != null && dc.Bvid is { Length: > 0 })
                _homeVm?.RequestPlayVideo(dc);
        };

        return outerBorder;
    }

    /// <summary>
    /// 滚动事件。
    ///
    /// 只有跨越整行时才重排容器并检查图片:同一行内滚动时,所有容器的数据与相对位置
    /// 都没有变化,跳过可避免每帧遍历整个容器池。
    /// 全屏下容器池可达上百个,这是滚动流畅度的关键。
    /// </summary>
    private void OnScrollChanged(object? sender, ScrollChangedEventArgs e)
    {
        if (_vm == null || _isUpdating) return;

        _isUpdating = true;
        bool rowChanged = _vm.UpdateOnScroll(ScrollHost.Offset.Y);
        if (rowChanged)
        {
            RepositionAll();
            LoadMissingImages();
        }
        _isUpdating = false;
    }

    /// <summary>根据当前 FirstVisibleRowIndex 重新定位 Canvas 中所有子元素</summary>
    private void RepositionAll()
    {
        if (_vm == null) return;

        int firstRow = _vm.FirstVisibleRowIndex;
        int cols = HomeViewModel.ColumnsPerRow;

        for (int i = 0; i < ContainerCanvas.Children.Count; i++)
        {
            var child = ContainerCanvas.Children[i];
            int poolRow = i / cols;
            int poolCol = i % cols;

            // 用 Canvas 绝对定位(非 Grid 的网格布局)
            Canvas.SetLeft(child, poolCol * _cellWidth);
            Canvas.SetTop(child, (firstRow + poolRow) * _cellHeight);
        }
    }

    /// <summary>VM 数据变化时:更新画布高度、重排、懒加载</summary>
    private void OnDataChanged()
    {
        if (_vm == null || !_initialized) return;

        ContainerCanvas.Height = _cellHeight * _vm.TotalRows;
        RepositionAll();
        LoadMissingImages();
    }

    /// <summary>
    /// 为"确实需要加载封面"的容器启动异步加载。
    ///
    /// 结构解析与 URL 判断都放在同步路径上:本方法由滚动与数据变化触发,
    /// 而绝大多数容器其实无需处理。若一律调用 async 方法,每次都会产生
    /// 状态机与 Task 分配,容器池上百个时会形成明显的 GC 压力。
    /// </summary>
    private void LoadMissingImages()
    {
        foreach (var child in ContainerCanvas.Children)
        {
            if (!TryGetCoverTarget(child, out var outer, out var item, out var img)) continue;
            if (img.GetValue(BoundImageUrlProperty) == item.ImageUrl) continue;
            _ = LoadImageForControlAsync(outer, item, img);
        }
    }

    /// <summary>
    /// 同步解析容器控件结构,取出外层 Border、封面数据与 Image。
    /// 任一环节不满足(结构不符 / 无数据 / 无图址)都返回 false。
    /// </summary>
    private static bool TryGetCoverTarget(Control child, out Border outer, out CoverItem item, out Image img)
    {
        outer = null!;
        item = null!;
        img = null!;

        if (child is not Border outerBorder) return false;
        if (outerBorder.Child is not StackPanel stack || stack.Children.Count == 0) return false;
        if (stack.Children[0] is not Border imageBorder || imageBorder.Child is not Image image) return false;
        if (outerBorder.DataContext is not CoverItem cover) return false;
        if (string.IsNullOrEmpty(cover.ImageUrl)) return false;

        outer = outerBorder;
        item = cover;
        img = image;
        return true;
    }

    /// <summary>
    /// 为单个卡片控件确保封面图就位。
    ///
    /// 结构判断已由调用方(TryGetCoverTarget)完成,这里只负责标记与回填。
    /// 判断依据是控件当前绑定的 URL,而不是 img.Source —— 容器池会复用控件,
    /// 复用后 img.Source 上残留的是上一个视频的图,会把新封面误判为"已加载"而跳过下载。
    /// </summary>
    private async Task LoadImageForControlAsync(Border outerBorder, CoverItem item, Image img)
    {
        // 立即占位标记,避免同一控件被并发重复处理
        img.SetValue(BoundImageUrlProperty, item.ImageUrl);

        var bitmap = await GetOrStartImageLoadAsync(item);

        // 控件在等待期间可能已被复用(滚动),只有仍指向同一 URL 时才回填
        if (outerBorder.DataContext is CoverItem current && current.ImageUrl == item.ImageUrl)
        {
            if (bitmap != null)
                img.Source = bitmap;
            else
                img.SetValue(BoundImageUrlProperty, null); // 失败则清除标记,允许后续滚动时重试
        }
    }

    /// <summary>
    /// 取得某个 URL 的封面图,必要时启动加载。
    /// 用共享 Task 去重:多个控件需要同一张图时 await 同一个任务,
    /// 不会出现"已经有人在下载就跳过、结果谁也没显示"的漏图。
    /// </summary>
    private Task<Bitmap?> GetOrStartImageLoadAsync(CoverItem item)
    {
        if (_imageCache.TryGetValue(item.ImageUrl, out var cached))
            return Task.FromResult(cached);

        if (_imageLoadTasks.TryGetValue(item.ImageUrl, out var running))
            return running;

        var task = LoadImageCoreAsync(item);
        _imageLoadTasks[item.ImageUrl] = task;
        return task;
    }

    /// <summary>
    /// 封面图实际加载流程:磁盘缓存 → 网络下载,成功后写入内存与磁盘缓存。
    /// 返回 null 表示加载失败(调用方保留占位色)。
    /// </summary>
    private async Task<Bitmap?> LoadImageCoreAsync(CoverItem item)
    {
        await _imageLoadSemaphore.WaitAsync();
        try
        {
            // 1) 磁盘缓存(按 BVID 分子目录)
            var fromDisk = TryLoadFromDiskCache(item);
            if (fromDisk != null)
            {
                _imageCache[item.ImageUrl] = fromDisk;
                return fromDisk;
            }

            // 2) 网络下载。
            //    只请求一次:同一份字节既用于解码显示,也用于写入磁盘缓存。
            //    (原实现为写缓存又单独请求了一次,等于每张封面下载两遍)
            var response = await _httpClient.GetAsync(item.ImageUrl);
            response.EnsureSuccessStatusCode();
            var bytes = await response.Content.ReadAsByteArrayAsync();

            Bitmap bitmap;
            using (var ms = new MemoryStream(bytes))
                bitmap = Bitmap.DecodeToWidth(ms, CoverDecodeWidth);

            _imageCache[item.ImageUrl] = bitmap;
            System.Diagnostics.Trace.WriteLine($"[HomeView] 网络下载封面成功 bvid={item.Bvid}");

            // 3) 落盘复用刚下载的字节,供下次启动直接命中
            if (!string.IsNullOrEmpty(item.Bvid))
                _ = SaveCoverBytesAsync(item.Bvid, bytes);

            return bitmap;
        }
        catch (Exception ex)
        {
            // 失败时保留占位色(用户仍能看到色块)
            System.Diagnostics.Trace.WriteLine($"[HomeView] 封面加载失败 bvid={item.Bvid}: {ex.Message}");
            return null;
        }
        finally
        {
            _imageLoadSemaphore.Release();
            _imageLoadTasks.Remove(item.ImageUrl);
        }
    }

    /// <summary>
    /// 尝试从磁盘读取封面图(路径: CacheRoot/{bvid}/cover.jpg)。
    /// 不存在或读取失败均返回 null,由调用方继续走网络下载。
    /// </summary>
    private static Bitmap? TryLoadFromDiskCache(CoverItem item)
    {
        try
        {
            if (string.IsNullOrEmpty(item.Bvid)) return null;

            var coverPath = Path.Combine(CacheManager.Instance.CacheRoot, item.Bvid, "cover.jpg");
            if (!File.Exists(coverPath)) return null;

            System.Diagnostics.Trace.WriteLine($"[HomeView] 命中磁盘缓存 bvid={item.Bvid}");
            using var fs = File.OpenRead(coverPath);
            return Bitmap.DecodeToWidth(fs, CoverDecodeWidth);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Trace.WriteLine($"[HomeView] 磁盘缓存读取失败 bvid={item.Bvid}: {ex.Message}");
            return null;
        }
    }

    /// <summary>
    /// 把封面字节写入磁盘缓存(路径: CacheRoot/{bvid}/cover.jpg)。
    /// 复用调用方已经下载好的字节,不再重复发起网络请求。
    /// </summary>
    private static async Task SaveCoverBytesAsync(string bvid, byte[] bytes)
    {
        try
        {
            var cacheDir = Path.Combine(CacheManager.Instance.CacheRoot, bvid);
            Directory.CreateDirectory(cacheDir);
            var coverPath = Path.Combine(cacheDir, "cover.jpg");

            await File.WriteAllBytesAsync(coverPath, bytes);
            System.Diagnostics.Trace.WriteLine($"[HomeView] 封面已写入磁盘缓存 bvid={bvid}");
        }
        catch (Exception ex)
        {
            System.Diagnostics.Trace.WriteLine($"[HomeView] 封面落盘失败 bvid={bvid}: {ex.Message}");
        }
    }

    /// <summary>监听布局更新以检测视口尺寸变化(并触发重建)</summary>
    private void OnLayoutUpdatedForResize(object? sender, EventArgs e)
    {
        if (_vm == null || !_initialized) return;

        double viewportWidth = ScrollHost.Viewport.Width;
        double viewportHeight = ScrollHost.Viewport.Height;
        if (viewportWidth <= 0 || viewportHeight <= 0) return;

        bool widthChanged = Math.Abs(viewportWidth - _lastViewportWidth) >= 1.0;
        bool heightChanged = Math.Abs(viewportHeight - _lastViewportHeight) >= 1.0;

        if (!widthChanged && !heightChanged) return;

        _lastViewportWidth = viewportWidth;
        _lastViewportHeight = viewportHeight;

        RebuildAllControls();
    }

    /// <summary>
    /// 视口尺寸发生有意义变化时触发重建:
    ///   - 宽度跨越列边界:列数变化 -> 完整重建
    ///   - 高度跨越行边界:行数变化 -> 调整池尾
    /// </summary>
    private void RebuildAllControls()
    {
        if (_vm == null) return;

        double viewportWidth = ScrollHost.Viewport.Width;
        double viewportHeight = ScrollHost.Viewport.Height;
        if (viewportWidth <= 0 || viewportHeight <= 0) return;

        int currentCols = HomeViewModel.ColumnsPerRow;
        int currentRows = _vm.VisibleRows;

        // 宽度变化检测
        bool widthGrown = viewportWidth >= _lastRebuildWidth + _cellWidth;
        bool widthShrunk = viewportWidth < currentCols * _cellWidth;
        bool needWidthRebuild = widthGrown || widthShrunk;

        // 高度变化检测
        bool heightGrown = viewportHeight >= _lastRebuildHeight + _cellHeight;
        bool heightShrunk = viewportHeight < currentRows * _cellHeight;
        bool needHeightRebuild = heightGrown || heightShrunk;

        // 未跨越列/行边界:列数与行数都没变,容器的排列也不会变,直接返回。
        // (此处原先还会调用一次 RepositionAll,但位置既然没变,那就是一次纯开销的全量遍历)
        if (!needWidthRebuild && !needHeightRebuild)
            return;

        _lastRebuildWidth = viewportWidth;
        _lastRebuildHeight = viewportHeight;

        // 重新计算列数与行数
        int newCols = Math.Max(1, (int)(viewportWidth / _cellWidth));
        int newRows = Math.Max(1, (int)Math.Ceiling(viewportHeight / _cellHeight));

        if (newCols != currentCols || newRows != currentRows)
        {
            // 注意:不要在这里预先修改 HomeViewModel.ColumnsPerRow。
            // 一旦外部先改了它,VM 内部的"列数是否变化"判断就恒为 false,
            // 会错误地走"仅行数变化"分支——导致 FirstVisibleRowIndex 不重置、
            // 容器池不按新列数重建索引映射,表现为最大化后布局错乱、顶部留白。
            int colsBefore = HomeViewModel.ColumnsPerRow;
            _vm.RecalculateOnResize(viewportWidth, viewportHeight, _cellWidth, _cellHeight);

            if (HomeViewModel.ColumnsPerRow != colsBefore)
            {
                // 列数变化会让整块内容重新排布,旧的滚动偏移已失去意义,
                // 必须回到顶部,才能与 VM 内部重置的 FirstVisibleRowIndex = 0 保持一致。
                ContainerCanvas.Width = _cellWidth * HomeViewModel.ColumnsPerRow;
                ScrollHost.Offset = new Vector(ScrollHost.Offset.X, 0);
            }
        }

        ContainerCanvas.Height = _cellHeight * _vm.TotalRows;

        // 同步 UI 控件
        ContainerCanvas.Children.Clear();
        foreach (var item in _vm.VisibleContainers)
            ContainerCanvas.Children.Add(CreateCoverControl(item));

        RepositionAll();
        LoadMissingImages();

        // 视口变大后,原有数据可能不足以填满新的一屏(此时用户没有滚动空间,
        // ScrollChanged 不会发生),需要主动评估一次是否需要追加数据。
        _vm.EvaluateLoadMore();
    }
}

/// <summary>
/// HexColorConverter:十六进制颜色字符串 -> SolidColorBrush 的值转换器
/// 用于在 XAML 绑定中直接把 "#RRGGBB" 形式的字符串当作颜色源使用。
/// </summary>
public class HexColorConverter : Avalonia.Data.Converters.IValueConverter
{
    public object? Convert(object? value, Type targetType, object? parameter,
        System.Globalization.CultureInfo culture)
    {
        if (value is string hex && !string.IsNullOrEmpty(hex))
        {
            try
            {
                var color = Color.Parse(hex);
                return new SolidColorBrush(color);
            }
            catch
            {
                return Brushes.Gray;
            }
        }
        return Brushes.Transparent;
    }

    // 反向转换不需要,直接抛 NotSupportedException
    public object? ConvertBack(object? value, Type targetType, object? parameter,
        System.Globalization.CultureInfo culture)
        => throw new NotSupportedException();
}
