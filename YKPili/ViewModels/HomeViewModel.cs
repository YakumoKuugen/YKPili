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
    public static int ColumnsPerRow { get; set; } = 5;
    public const double ItemWidth = 192;   // 16:9 横向:封面宽度
    public const double ItemHeight = 108;  // 16:9 横向:封面高度
    public const double CellWidth = 208;   // ItemWidth(192) + 边距(16)
    public const double CellHeight = 166;  // ItemHeight(108) + 标题区 6 + 36 + 内边距 16
    public const int TotalItemCount = 200; // 初始模拟数据条数
    public const int MaxItemCount = 200;   // 允许的最大数据条数(到达后停止追加)

    // 防止并发触发的滚动加载标记
    private bool _isLoadingMore;

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
        // 调色板(20 种 Material 风格颜色)
        string[] colors =
        [
            "#E57373", "#F06292", "#BA68C8", "#9575CD", "#7986CB",
            "#64B5F6", "#4FC3F7", "#4DD0E1", "#4DB6AC", "#81C784",
            "#AED581", "#DCE775", "#FFF176", "#FFD54F", "#FFB74D",
            "#FF8A65", "#A1887F", "#E0E0E0", "#90A4AE", "#BCAAA4"
        ];

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
                PlaceholderColor = colors[i % colors.Length]
            };
        }
    }

    /// <summary>使用真实推荐数据替换全量数据(初次加载)</summary>
    public void LoadRecommendVideos(List<RecommendVideoItem> videos)
    {
        if (videos.Count == 0) return;

        // 调色板(同上,此处复制一份避免在多处共享可变数组)
        string[] colors =
        [
            "#E57373", "#F06292", "#BA68C8", "#9575CD", "#7986CB",
            "#64B5F6", "#4FC3F7", "#4DD0E1", "#4DB6AC", "#81C784",
            "#AED581", "#DCE775", "#FFF176", "#FFD54F", "#FFB74D",
            "#FF8A65", "#A1887F", "#E0E0E0", "#90A4AE", "#BCAAA4"
        ];

        _allItems = new CoverItem[videos.Count];
        for (int i = 0; i < videos.Count; i++)
        {
            var v = videos[i];
            _allItems[i] = new CoverItem
            {
                Index = i,
                // HTML 实体解码(原标题含 &amp; 等)
                Title = System.Net.WebUtility.HtmlDecode(v.Title),
                PlaceholderColor = colors[i % colors.Length],
                ImageUrl = v.Pic,
                Bvid = v.Bvid
            };
            if (i < 3)
                System.Diagnostics.Trace.WriteLine($"[HomeVM] Item {i}: Bvid={v.Bvid} Title={v.Title}");
        }

        // 重新初始化容器池以适配新数据
        int poolCount = (VisibleRows + 2) * ColumnsPerRow;
        int maxNeeded = Math.Min(poolCount, videos.Count);

        // 复用现有容器:把前 maxNeeded 个容器重置为新数据
        for (int i = 0; i < VisibleContainers.Count && i < maxNeeded; i++)
        {
            var existing = VisibleContainers[i];
            existing.Index = i;
            existing.Title = _allItems[i].Title;
            existing.PlaceholderColor = _allItems[i].PlaceholderColor;
            existing.ImageUrl = _allItems[i].ImageUrl;
            existing.Bvid = _allItems[i].Bvid;
        }

        // 多余的容器清空(用透明色占位,避免视觉残留)
        for (int i = maxNeeded; i < VisibleContainers.Count; i++)
        {
            VisibleContainers[i].Title = "";
            VisibleContainers[i].PlaceholderColor = "#00000000";
        }

        // 通知 View 更新画布高度等
        DataChanged?.Invoke();
    }

    /// <summary>追加更多推荐数据(滚动到底部时由 ViewModel 发起)</summary>
    public void AppendRecommendVideos(List<RecommendVideoItem> newVideos)
    {
        _isLoadingMore = false;
        if (newVideos.Count == 0) return;

        // 调色板
        string[] colors =
        [
            "#E57373", "#F06292", "#BA68C8", "#9575CD", "#7986CB",
            "#64B5F6", "#4FC3F7", "#4DD0E1", "#4DB6AC", "#81C784",
            "#AED581", "#DCE775", "#FFF176", "#FFD54F", "#FFB74D",
            "#FF8A65", "#A1887F", "#E0E0E0", "#90A4AE", "#BCAAA4"
        ];

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
                PlaceholderColor = colors[(oldLength + i) % colors.Length],
                ImageUrl = v.Pic,
                Bvid = v.Bvid
            };
        }

        // 通知 View 数据变化(由 View 重新填充容器)
        DataChanged?.Invoke();
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

    /// <summary>初始化容器池(视口首次确定时调用)</summary>
    public void InitializeContainers(double viewportHeight)
    {
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
    public double UpdateOnScroll(double scrollOffsetY)
    {
        int newFirstRow = Math.Max(0, (int)Math.Floor(scrollOffsetY / CellHeight));
        int maxReachableRow = Math.Max(0, TotalRows - VisibleRows);
        newFirstRow = Math.Min(newFirstRow, maxReachableRow);

        // 未变化则不重排
        if (newFirstRow == FirstVisibleRowIndex)
            return CellHeight;

        FirstVisibleRowIndex = newFirstRow;
        FillContainerData();

        // 触发更多数据的条件:未在加载 + 未达上限 + 滚动超过 90% 或触底
        double scrollPercentage = TotalRows > 0 ? (double)FirstVisibleRowIndex / TotalRows : 0;
        bool atBottom = FirstVisibleRowIndex >= maxReachableRow;
        bool shouldLoad = !_isLoadingMore && _allItems.Length < MaxItemCount
                          && (scrollPercentage >= 0.9 || atBottom);

        System.Diagnostics.Trace.WriteLine(
            $"[HomeVM] Scroll: firstRow={FirstVisibleRowIndex} totalRows={TotalRows} " +
            $"visibleRows={VisibleRows} scroll%={scrollPercentage:F2} " +
            $"atBottom={atBottom} isLoading={_isLoadingMore} items={_allItems.Length} " +
            $"shouldLoad={shouldLoad}");

        if (shouldLoad)
        {
            _isLoadingMore = true;
            System.Diagnostics.Trace.WriteLine("[HomeVM] >>> Triggering LoadMoreRequested");
            LoadMoreRequested?.Invoke();
        }

        return CellHeight;
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
                // 越界位置用透明占位
                existing.Index = -1;
                existing.Title = "";
                existing.PlaceholderColor = "#00000000";
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
            PlaceholderColor = _allItems[dataIndex].PlaceholderColor
        };
    }

    // 创建一个空(透明占位)的 CoverItem
    private CoverItem MakeEmpty(int poolIndex)
    {
        return new CoverItem { Index = poolIndex, Title = "", PlaceholderColor = "#00000000" };
    }
}
