// 引入基础类型、集合、LINQ
using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.Linq;
// 引入 CommunityToolkit.Mvvm 源生成器
using CommunityToolkit.Mvvm.ComponentModel;
// 引入项目模型与 B 站模型
using YKPili.Models;
using YKPili.Services.Bilibili.Models;

namespace YKPili.ViewModels;

/// <summary>
/// HomeViewModel:首页推荐流 ViewModel
/// 设计要点:
///   1. 使用"虚拟列表 + 容器池"模式,以 Canvas + 少量可视容器实现 200 条数据的高性能滚动;
///   2. _allItems 保存全量数据,VisibleContainers 仅承载当前视口(及上下各一行)所需的可视容器;
///   3. 通过容器复用避免频繁创建/销毁 UI 元素;
///   4. 滚动接近底部时通过事件通知外部加载更多。
/// </summary>
public partial class HomeViewModel : ViewModelBase
{
    // ---- 布局常量(全部为 static const 便于外部 View 直接访问) ----
    /// <summary>
    /// 每行可容纳的列数。
    ///
    /// 该值是**推导结果**而非固定配置:由 InitializeContainers / RecalculateOnResize
    /// 按「视口宽度 / CellWidth」计算,窗口尺寸变化时会重新推导
    /// (例如 1920 宽得 9 列、2560 宽得 12 列、3840 宽得 18 列)。
    /// 任何其他位置都不应赋值给它。
    ///
    /// 初值 1 仅为占位:初始化完成前即便被读到,也不会被误当成有效列数。
    /// </summary>
    public static int ColumnsPerRow { get; set; } = 1;
    public const double ItemWidth = 192;   // 16:9 横向:封面宽度
    public const double ItemHeight = 108;  // 16:9 横向:封面高度
    public const double CellWidth = 208;   // ItemWidth(192) + 边距(16)
    public const double CellHeight = 166;  // ItemHeight(108) + 标题区 6 + 36 + 内边距 16
    public const int TotalItemCount = 200; // 初始模拟数据条数
    public const int MaxItemCount = 200;   // 允许的最大数据条数(到达后停止追加)

    // 防止并发触发的滚动加载标记
    private bool _isLoadingMore;

    /// <summary>
    /// 封面占位色调色板(封面图加载完成前显示的底色)。
    /// 原先这段数组在 GenerateMockDataSource / LoadRecommendVideos / AppendRecommendVideos
    /// 三处各硬编码一份,现统一为单一来源。
    /// </summary>
    private static readonly string[] PlaceholderPalette =
    [
        "#E57373", "#F06292", "#BA68C8", "#9575CD", "#7986CB",
        "#64B5F6", "#4FC3F7", "#4DD0E1", "#4DB6AC", "#81C784",
        "#AED581", "#DCE775", "#FFF176", "#FFD54F", "#FFB74D",
        "#FF8A65", "#A1887F", "#E0E0E0", "#90A4AE", "#BCAAA4"
    ];

    /// <summary>容器池中越界位置的透明占位色(表示该格子没有对应数据)</summary>
    private const string EmptyPlaceholder = "#00000000";

    // 全量数据(逻辑数据源)
    private CoverItem[] _allItems = [];

    /// <summary>
    /// 可视容器池:HomeView 直接绑定此集合并在 Canvas 上布局。
    /// 集合元素会随 FirstVisibleRowIndex 变化而被复用(数据填充)。
    /// </summary>
    [ObservableProperty]
    private ObservableCollection<CoverItem> _visibleContainers = new();

    // 当前可见的行数(由 View 在 OnSizeChanged 时计算并写入)
    public int VisibleRows { get; private set; }

    // 当前滚动位置对应的起始行号
    public int FirstVisibleRowIndex { get; private set; }

    // 总行数(根据 _allItems 数量与每行列数动态计算)
    public int TotalRows => Math.Max(1, (int)Math.Ceiling((double)_allItems.Length / ColumnsPerRow));

    // 容器池中当前的容器数量
    public int ContainerCount => VisibleContainers.Count;

    // 全量数据条数
    public int ItemCount => _allItems.Length;

    // 是否已加载到真实数据
    public bool DataLoaded => _allItems.Length > 0;

    // 默认构造:生成 200 条模拟数据(用于开发期/未登录态)
    public HomeViewModel()
    {
        GenerateMockDataSource();
    }

    // ================================================================
    // 模拟数据(占位封面与标题)
    // ================================================================
    private void GenerateMockDataSource()
    {
        // 标题词库
        string[] titles =
        [
            "Mountain Dawn", "Ocean Breeze", "City Lights", "Forest Path",
            "Desert Sun", "Arctic Blue", "Coral Reef", "Starry Night",
            "Golden Hour", "Rainy Day", "Spring Bloom", "Autumn Leaves",
            "Winter Frost", "Summer Heat", "River Flow", "Jungle Mist",
            "Canyon Echo", "Island Song", "Storm Front", "Cliff Edge"
        ];

        _allItems = new CoverItem[TotalItemCount];
        for (int i = 0; i < TotalItemCount; i++)
        {
            _allItems[i] = new CoverItem
            {
                Index = i,
                Title = $"{titles[i % titles.Length]} #{i + 1}",
                PlaceholderColor = PlaceholderPalette[i % PlaceholderPalette.Length]
            };
        }
    }

    /// <summary>使用真实推荐数据替换全量数据(初次加载)</summary>
    public void LoadRecommendVideos(List<RecommendVideoItem> videos)
    {
        if (videos.Count == 0) return;

        _allItems = new CoverItem[videos.Count];
        for (int i = 0; i < videos.Count; i++)
        {
            var v = videos[i];
            _allItems[i] = new CoverItem
            {
                Index = i,
                // HTML 实体解码(原标题含 &amp; 等)
                Title = System.Net.WebUtility.HtmlDecode(v.Title),
                PlaceholderColor = PlaceholderPalette[i % PlaceholderPalette.Length],
                ImageUrl = v.Pic,
                Bvid = v.Bvid
            };
            if (i < 3)
                System.Diagnostics.Trace.WriteLine($"[HomeVM] Item {i}: Bvid={v.Bvid} Title={v.Title}");
        }

        // 用统一的数据填充逻辑刷新容器池:它会按当前视口推导池大小、
        // 自动扩容并处理越界空位,避免此处再维护一份与 FillContainerData 重复的映射规则。
        FillContainerData();

        // 通知 View 更新画布高度等
        DataChanged?.Invoke();

        // 首屏数据若不足以填满视口,用户没有滚动空间,必须在这里主动请求更多
        EvaluateLoadMore();
    }

    /// <summary>追加更多推荐数据(滚动到底部时由 ViewModel 发起)</summary>
    public void AppendRecommendVideos(List<RecommendVideoItem> newVideos)
    {
        _isLoadingMore = false;
        if (newVideos.Count == 0) return;

        int oldLength = _allItems.Length;
        // 真正可追加的条数(受 MaxItemCount 上限约束)
        int toAdd = Math.Min(newVideos.Count, MaxItemCount - oldLength);
        if (toAdd <= 0) return;

        // 扩容底层数组
        Array.Resize(ref _allItems, oldLength + toAdd);
        for (int i = 0; i < toAdd; i++)
        {
            var v = newVideos[i];
            _allItems[oldLength + i] = new CoverItem
            {
                Index = oldLength + i,
                Title = System.Net.WebUtility.HtmlDecode(v.Title),
                PlaceholderColor = PlaceholderPalette[(oldLength + i) % PlaceholderPalette.Length],
                ImageUrl = v.Pic,
                Bvid = v.Bvid
            };
        }

        // 数据变多后,池中原本因越界而空置的容器应立即填上新数据。
        // 这一步不能省:窗口放大后重新计算的容器池会比当时的数据量大,
        // 那些空容器只有等到下一次滚动跨行才被 FillContainerData 补上,
        // 表现为"放大后下半部分空白,滚一下才出现"。
        FillContainerData();

        // 通知 View 数据变化(更新画布高度、重排、加载封面)
        DataChanged?.Invoke();

        // 追加后仍不足一屏时继续请求,直到内容可滚动或达到条数上限
        EvaluateLoadMore();
    }

    /// <summary>由 View 在加载更多请求失败/完成后调用,清除加载中标记</summary>
    public void OnLoadMoreCompleted() => _isLoadingMore = false;

    // 数据集合变化事件(供 View 监听以重新布局)
    public event Action? DataChanged;

    // 加载更多事件(供 MainViewModel 监听,触发 B 站 API 请求)
    public event Action? LoadMoreRequested;

    // 单个视频被点击请求播放事件
    public event Action<CoverItem>? VideoPlayRequested;

    /// <summary>由 View 调用,转发给订阅者,触发视频播放流程</summary>
    public void RequestPlayVideo(CoverItem item)
    {
        VideoPlayRequested?.Invoke(item);
    }

    /// <summary>
    /// 初始化容器池(视口首次确定时调用)。
    ///
    /// 列数与行数都由这里根据视口尺寸推导——View 不应直接写 ColumnsPerRow。
    /// 该状态若被外部抢先赋值,RecalculateOnResize 里的"列数是否变化"判断会失效,
    /// 进而导致窗口尺寸变化时容器池不按新列数重建(详见 Views/HomeView.axaml.cs 对应注释)。
    /// </summary>
    public void InitializeContainers(double viewportWidth, double viewportHeight)
    {
        ColumnsPerRow = Math.Max(1, (int)(viewportWidth / CellWidth));
        VisibleRows = Math.Max(1, (int)Math.Ceiling(viewportHeight / CellHeight));
        int poolCount = (VisibleRows + 2) * ColumnsPerRow;

        VisibleContainers.Clear();
        FirstVisibleRowIndex = 0;

        FillContainerData();
    }

    /// <summary>
    /// 滚动回调:由 View 在 ScrollChanged 时调用。
    /// 根据新滚动偏移计算 FirstVisibleRowIndex,并视情况触发加载更多。
    /// 返回 CellHeight 以便调用方复用。
    /// </summary>
    /// <summary>
    /// 滚动回调。
    ///
    /// 返回值表示"可见起始行是否发生变化":容器内容与屏幕位置只在跨行时才需要更新,
    /// 未跨行时应由 View 跳过重排。全屏下容器池可达上百个,
    /// 若每帧无条件重排会直接表现为滚动卡顿。
    /// </summary>
    public bool UpdateOnScroll(double scrollOffsetY)
    {
        int newFirstRow = Math.Max(0, (int)Math.Floor(scrollOffsetY / CellHeight));
        int maxReachableRow = Math.Max(0, TotalRows - VisibleRows);
        newFirstRow = Math.Min(newFirstRow, maxReachableRow);

        bool rowChanged = newFirstRow != FirstVisibleRowIndex;
        if (rowChanged)
        {
            FirstVisibleRowIndex = newFirstRow;
            FillContainerData();
        }

        // 行号未变化时同样必须评估加载更多。
        // 首屏数据不足以填满视口时 maxReachableRow 恒为 0,行号永远停在 0,
        // 若在此提前 return,加载更多将永远不会触发——用户也因此没有任何可滚动空间。
        EvaluateLoadMore();

        return rowChanged;
    }

    /// <summary>
    /// 评估是否需要追加更多数据。触发情形有两类:
    ///   1. 已滚动到底部(常规滚动加载);
    ///   2. 内容不足以填满视口——此时用户根本没有可滚动空间,ScrollChanged 不会发生,
    ///      必须在数据就绪后主动请求,否则首屏之后永远不会有新内容。
    /// 由 View 在数据变化与首次布局完成后调用,滚动时也会经过 UpdateOnScroll 调用。
    /// </summary>
    public void EvaluateLoadMore()
    {
        // 视口尚未确定时无法判断"内容是否够一屏",等布局完成后再评估
        if (VisibleRows <= 0) return;
        // 正在加载或已达条数上限时不重复触发
        if (_isLoadingMore || _allItems.Length >= MaxItemCount) return;

        int maxReachableRow = Math.Max(0, TotalRows - VisibleRows);
        bool atBottom = FirstVisibleRowIndex >= maxReachableRow;
        bool contentTooShort = TotalRows <= VisibleRows;
        double scrollPercentage = TotalRows > 0 ? (double)FirstVisibleRowIndex / TotalRows : 0;

        if (!atBottom && !contentTooShort && scrollPercentage < 0.9) return;

        _isLoadingMore = true;
        System.Diagnostics.Trace.WriteLine(
            $"[HomeVM] >>> 触发加载更多: items={_allItems.Length} totalRows={TotalRows} " +
            $"visibleRows={VisibleRows} firstRow={FirstVisibleRowIndex} " +
            $"atBottom={atBottom} contentTooShort={contentTooShort}");
        LoadMoreRequested?.Invoke();
    }

    /// <summary>
    /// 视口尺寸变化时调用,返回是否发生真实变化。
    /// 区别"列数变化"(需要全量重建)与"仅行数变化"(只需调整池尾)。
    /// </summary>
    public bool RecalculateOnResize(double viewportWidth, double viewportHeight, double cellWidth, double cellHeight)
    {
        int newCols = Math.Max(1, (int)(viewportWidth / cellWidth));
        int newRows = Math.Max(1, (int)Math.Ceiling(viewportHeight / cellHeight));

        if (newCols == ColumnsPerRow && newRows == VisibleRows)
            return false;

        bool colsChanged = newCols != ColumnsPerRow;
        ColumnsPerRow = newCols;
        VisibleRows = newRows;

        if (colsChanged)
        {
            // 列数变化:数据索引映射方式改变,需要重建
            FirstVisibleRowIndex = 0;
            int poolCount = (VisibleRows + 2) * ColumnsPerRow;
            VisibleContainers.Clear();
            for (int i = 0; i < poolCount; i++)
            {
                int logicalRow = i / ColumnsPerRow;
                int logicalCol = i % ColumnsPerRow;
                int dataIndex = logicalRow * ColumnsPerRow + logicalCol;
                if (dataIndex < _allItems.Length)
                    VisibleContainers.Add(CloneTo(i, dataIndex));
                else
                    VisibleContainers.Add(MakeEmpty(i));
            }
        }
        else
        {
            // 仅行数变化:调整池尾(添加/移除)
            int poolCount = (VisibleRows + 2) * ColumnsPerRow;
            while (VisibleContainers.Count < poolCount)
            {
                int idx = VisibleContainers.Count;
                int logicalRow = FirstVisibleRowIndex + idx / ColumnsPerRow;
                int logicalCol = idx % ColumnsPerRow;
                int dataIndex = logicalRow * ColumnsPerRow + logicalCol;
                if (dataIndex >= 0 && dataIndex < _allItems.Length)
                    VisibleContainers.Add(CloneTo(idx, dataIndex));
                else
                    VisibleContainers.Add(MakeEmpty(idx));
            }
            while (VisibleContainers.Count > poolCount)
                VisibleContainers.RemoveAt(VisibleContainers.Count - 1);
        }

        return true;
    }

    /// <summary>
    /// 用 _allItems 当前可见行的数据填充/复用容器池。
    /// 保持池大小 = (VisibleRows + 2) * ColumnsPerRow(上下各多 1 行做缓冲)。
    /// </summary>
    private void FillContainerData()
    {
        int poolCount = (VisibleRows + 2) * ColumnsPerRow;

        // 1. 确保池子有足够容器
        while (VisibleContainers.Count < poolCount)
        {
            int idx = VisibleContainers.Count;
            int logicalRow = FirstVisibleRowIndex + idx / ColumnsPerRow;
            int logicalCol = idx % ColumnsPerRow;
            int dataIndex = logicalRow * ColumnsPerRow + logicalCol;
            if (dataIndex < _allItems.Length)
                VisibleContainers.Add(CloneTo(idx, dataIndex));
            else
                VisibleContainers.Add(MakeEmpty(idx));
        }

        // 2. 对池中已有容器做数据重映射
        for (int i = 0; i < poolCount; i++)
        {
            int logicalRow = FirstVisibleRowIndex + i / ColumnsPerRow;
            int logicalCol = i % ColumnsPerRow;
            int dataIndex = logicalRow * ColumnsPerRow + logicalCol;

            CoverItem existing = VisibleContainers[i];

            if (dataIndex >= 0 && dataIndex < _allItems.Length)
            {
                existing.Index = i; // 池中索引
                existing.Title = _allItems[dataIndex].Title;
                existing.PlaceholderColor = _allItems[dataIndex].PlaceholderColor;
                existing.ImageUrl = _allItems[dataIndex].ImageUrl;
                existing.Bvid = _allItems[dataIndex].Bvid;
                if (dataIndex < 3)
                    System.Diagnostics.Trace.WriteLine($"[FillData] Pool[{i}] Bvid={existing.Bvid} Title={existing.Title}");
            }
            else
            {
                // 越界位置用透明占位。
                // 同时清空图址与 BVID,避免容器复用时残留上一轮的数据。
                existing.Index = -1;
                existing.Title = "";
                existing.PlaceholderColor = EmptyPlaceholder;
                existing.ImageUrl = string.Empty;
                existing.Bvid = string.Empty;
            }
        }
    }

    // 用数据项内容克隆一个新的 CoverItem 给池中容器使用
    private CoverItem CloneTo(int poolIndex, int dataIndex)
    {
        return new CoverItem
        {
            Index = poolIndex,
            Title = _allItems[dataIndex].Title,
            PlaceholderColor = _allItems[dataIndex].PlaceholderColor,
            // 必须同时带上图址与 BVID:列数变化重建容器池时会走这里,
            // 若漏掉这两个字段,新建的容器永远没有封面可加载(整屏空占位)。
            ImageUrl = _allItems[dataIndex].ImageUrl,
            Bvid = _allItems[dataIndex].Bvid
        };
    }

    // 创建一个空(透明占位)的 CoverItem
    private CoverItem MakeEmpty(int poolIndex)
    {
        return new CoverItem { Index = poolIndex, Title = "", PlaceholderColor = EmptyPlaceholder };
    }
}
