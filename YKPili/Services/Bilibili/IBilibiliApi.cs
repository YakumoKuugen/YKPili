// 引入异步任务类型
using System.Threading.Tasks;
// 引入 B 站 API 响应模型
using YKPili.Services.Bilibili.Models;
// 别名以避免与本项目 Models.UserSession 冲突
using UserSession = global::YKPili.Models.UserSession;

namespace YKPili.Services.Bilibili;

/// <summary>
/// IBilibiliApi:B 站开放接口客户端抽象
/// 涵盖登录、用户信息、推荐流、视频详情、播放地址等核心场景。
/// </summary>
public interface IBilibiliApi
{
    /// <summary>申请登录二维码(返回二维码图片 URL 与唯一 key)</summary>
    Task<LoginQrCodeResponse> GenerateQrCodeAsync();

    /// <summary>
    /// 轮询登录状态,参数为二维码 key。
    /// 返回值包含原始轮询响应以及(登录成功时)从 Set-Cookie 头解析出的会话信息。
    /// </summary>
    Task<(LoginPollResponse PollResponse, UserSession? Session)> PollLoginAsync(string qrcodeKey);

    /// <summary>通过 SESSDATA 获取用户信息(含 WBI 签名所需 img_key/sub_key)</summary>
    Task<BilibiliUserInfoResponse> GetUserInfoAsync(string sessdata);

    /// <summary>获取首页推荐视频列表(需要登录态与 WBI 签名)</summary>
    Task<BilibiliRecommendResponse> GetRecommendVideosAsync(UserSession session);

    /// <summary>获取指定 bvid 的视频基础信息(用于取得 cid 等)</summary>
    Task<VideoInfoResponse> GetVideoInfoAsync(string bvid, UserSession session);

    /// <summary>获取指定 bvid + cid 的播放地址(DASH 分离流或 durl 单文件)</summary>
    Task<VideoPlayUrlResponse> GetPlayUrlAsync(string bvid, long cid, UserSession session);
}
