// 引入基础类型、IO、HTTP、异步、集合
using System;
using System.Collections.Generic;
using System.IO;
using System.Net.Http;
using System.Threading;
using System.Threading.Tasks;
// 引入 Avalonia 布局与媒体类型
using Avalonia;
using Avalonia.Controls;
using Avalonia.Layout;
using Avalonia.Media;
using Avalonia.Media.Imaging;
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
    // 正在下载中的 URL 集合(避免重复请求)
    private readonly HashSet<string> _pendingImageUrls = new();

    public HomeView()
    {
        InitializeComponent();
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

        // 根据视口宽度计算每行可容纳的列数
        HomeViewModel.ColumnsPerRow = Math.Max(1, (int)(viewportWidth / _cellWidth));
        ContainerCanvas.Width = _cellWidth * HomeViewModel.ColumnsPerRow;
        ContainerCanvas.Height = _cellHeight * _vm.TotalRows;

        // 让 VM 初始化容器池,然后我们再创建对应的 Avalonia 控件
        _vm.InitializeContainers(viewportHeight);
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
    private static Control CreateCoverControl(CoverItem item)
    {
        // 封面容器:圆角矩形,带占位色作为加载前的背景
        var imageContainer = new Border
        {
            Height = HomeViewModel.ItemHeight,
            Width = HomeViewModel.ItemWidth,
            CornerRadius = new CornerRadius(6),
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

        // 标题文本
        var titleBlock = new TextBlock
        {
            HorizontalAlignment = HorizontalAlignment.Center,
            Margin = new Thickness(0, 6, 0, 0),
            // 中文字体优先
            FontFamily = "PingFang SC, Microsoft YaHei, Noto Sans CJK SC, sans-serif",
            FontSize = 13,
            TextWrapping = TextWrapping.Wrap,
            MaxWidth = HomeViewModel.ItemWidth,
            MaxHeight = 36,     // 限制 2 行(行高约 18px)
            TextTrimming = TextTrimming.CharacterEllipsis,
            LineHeight = 18
        };
        titleBlock.Bind(TextBlock.TextProperty,
            new Avalonia.Data.Binding("Title"));

        // 上下堆叠:封面 + 标题
        var stack = new StackPanel
        {
            HorizontalAlignment = HorizontalAlignment.Center,
            Children = { imageContainer, titleBlock }
        };

        // 外层 Border:接收点击,显示手型光标
        var outerBorder = new Border
        {
            Margin = new Thickness(8),
            Padding = new Thickness(0),
            CornerRadius = new CornerRadius(6),
            Background = Brushes.Transparent,
            Child = stack,
            DataContext = item,
            Cursor = new Avalonia.Input.Cursor(Avalonia.Input.StandardCursorType.Hand)
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

    /// <summary>滚动事件:通知 VM 更新起始行,然后重排 + 懒加载</summary>
    private void OnScrollChanged(object? sender, ScrollChangedEventArgs e)
    {
        if (_vm == null || _isUpdating) return;

        _isUpdating = true;
        _vm.UpdateOnScroll(ScrollHost.Offset.Y);
        RepositionAll();
        LoadMissingImages();
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

    /// <summary>遍历 Canvas 子元素,异步加载尚未缓存的图片</summary>
    private void LoadMissingImages()
    {
        foreach (var child in ContainerCanvas.Children)
            _ = LoadImageForControlAsync(child);
    }

    /// <summary>
    /// 单个控件的图片异步加载流程:
    ///   1) 内存命中则直接用
    ///   2) 已经在下载中则跳过
    ///   3) 尝试磁盘缓存
    ///   4) 磁盘未命中则从网络下载
    ///   5) 成功后将图片存到内存缓存(并保存到磁盘)
    /// </summary>
    private async Task LoadImageForControlAsync(Control child)
    {
        // 控件结构守卫
        if (child is not Border outerBorder) return;
        if (outerBorder.Child is not StackPanel stack || stack.Children.Count == 0) return;
        if (stack.Children[0] is not Border imageBorder || imageBorder.Child is not Image img) return;
        if (outerBorder.DataContext is not CoverItem item) return;
        if (string.IsNullOrEmpty(item.ImageUrl)) return;

        // 1) 内存缓存命中
        if (_imageCache.TryGetValue(item.ImageUrl, out var cached) && cached != null)
        {
            img.Source = cached;
            return;
        }

        // 2) 已在下载中
        if (_pendingImageUrls.Contains(item.ImageUrl)) return;

        _pendingImageUrls.Add(item.ImageUrl);
        await _imageLoadSemaphore.WaitAsync();
        try
        {
            // 3) 尝试磁盘缓存(按 BVID 分子目录)
            if (!string.IsNullOrEmpty(item.Bvid))
                TryLoadFromDiskCache(item, img);

            if (img.Source != null)
            {
                return; // 已从磁盘加载
            }

            // 4) 网络下载
            var response = await _httpClient.GetAsync(item.ImageUrl);
            response.EnsureSuccessStatusCode();
            var stream = await response.Content.ReadAsStreamAsync();
            var bitmap = new Bitmap(stream);
            _imageCache[item.ImageUrl] = bitmap;

            // 仅当当前控件仍引用同一 URL 时才更新,避免滚动时旧数据写入
            if (outerBorder.DataContext is CoverItem currentItem && currentItem.ImageUrl == item.ImageUrl)
                img.Source = bitmap;

            // 5) 保存到磁盘
            if (!string.IsNullOrEmpty(item.Bvid))
                _ = SaveCoverToDiskAsync(item.Bvid, item.ImageUrl);
        }
        catch
        {
            // 失败则保持占位色(用户仍能看到色块)
        }
        finally
        {
            _imageLoadSemaphore.Release();
            _pendingImageUrls.Remove(item.ImageUrl);
        }
    }

    /// <summary>尝试从磁盘加载封面图(路径: CacheRoot/{bvid}/cover.jpg)</summary>
    private void TryLoadFromDiskCache(CoverItem item, Image img)
    {
        try
        {
            var coverPath = Path.Combine(CacheManager.Instance.CacheRoot, item.Bvid, "cover.jpg");
            if (!File.Exists(coverPath)) return;

            var bitmap = new Bitmap(coverPath);
            _imageCache[item.ImageUrl] = bitmap;
            img.Source = bitmap;
            System.Diagnostics.Trace.WriteLine($"[HomeView] Loaded cover from disk cache: {item.Bvid}");
        }
        catch (Exception ex)
        {
            System.Diagnostics.Trace.WriteLine($"[HomeView] Disk cache load failed: {ex.Message}");
        }
    }

    /// <summary>异步保存封面到磁盘(用于下次快速启动)</summary>
    private static async Task SaveCoverToDiskAsync(string bvid, string imageUrl)
    {
        try
        {
            var cacheDir = Path.Combine(CacheManager.Instance.CacheRoot, bvid);
            Directory.CreateDirectory(cacheDir);
            var coverPath = Path.Combine(cacheDir, "cover.jpg");

            var response = await _httpClient.GetAsync(imageUrl);
            response.EnsureSuccessStatusCode();
            await using var stream = await response.Content.ReadAsStreamAsync();
            await using var fileStream = new FileStream(coverPath, FileMode.Create, FileAccess.Write,
                FileShare.None, 64 * 1024, useAsync: true);
            await stream.CopyToAsync(fileStream);
            System.Diagnostics.Trace.WriteLine($"[HomeView] Saved cover to disk cache: {bvid}");
        }
        catch (Exception ex)
        {
            System.Diagnostics.Trace.WriteLine($"[HomeView] Disk cache save failed: {ex.Message}");
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

        // 实际没有跨越列/行边界,只做位置调整
        if (!needWidthRebuild && !needHeightRebuild)
        {
            RepositionAll();
            return;
        }

        _lastRebuildWidth = viewportWidth;
        _lastRebuildHeight = viewportHeight;

        // 重新计算列数与行数
        int newCols = Math.Max(1, (int)(viewportWidth / _cellWidth));
        int newRows = Math.Max(1, (int)Math.Ceiling(viewportHeight / _cellHeight));

        if (newCols != currentCols || newRows != currentRows)
        {
            if (newCols != currentCols)
            {
                HomeViewModel.ColumnsPerRow = newCols;
                ContainerCanvas.Width = _cellWidth * newCols;
            }
            // 让 VM 决定如何重建容器池
            _vm.RecalculateOnResize(viewportWidth, viewportHeight, _cellWidth, _cellHeight);
        }

        ContainerCanvas.Height = _cellHeight * _vm.TotalRows;

        // 同步 UI 控件
        ContainerCanvas.Children.Clear();
        foreach (var item in _vm.VisibleContainers)
            ContainerCanvas.Children.Add(CreateCoverControl(item));

        RepositionAll();
        LoadMissingImages();
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
