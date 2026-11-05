# macOS 音频设备掉线崩溃（AVSampleBufferAudioRenderer flush 通知）

> 症状：播放中拔掉耳机 / 切换输出设备 / 蓝牙断开 → 应用直接闪退（SIGSEGV）。
> 结论：**mpv 0.41 `ao_avfoundation` 的 bug**，不是应用代码里的空指针。
> 修复方式：给随包 libmpv 打源码补丁（我们的构建脚本本来就自己编 mpv）。

## 1. 崩溃现场

```
-[AVSampleBufferAudioRenderer _wasFlushedAutomaticallyAtTime:]
  → -[NSNotificationCenter postNotificationName:object:userInfo:]
    → __CFNOTIFICATIONCENTER_IS_CALLING_OUT_TO_AN_OBSERVER__
      → libmpv.2.dylib  0x…（imageOffset 1037944）
        → 0x0
```

对照本机 `~/Library/Logs/DiagnosticReports/ykpili-*.ips`：

| 字段 | 值 |
| --- | --- |
| exception | `EXC_BAD_ACCESS (SIGSEGV)`，`KERN_INVALID_ADDRESS at 0x0000000000000000` |
| 崩溃线程 | `com.apple.coremedia.audiorenderer.notification`（dispatch worker） |
| 栈顶帧 | `imageIndex 16 / imageOffset 0`，即 **PC = 0**（跳到空地址） |

关键点：崩溃线程**不是 mpv 的 AO 线程**，而是 CoreMedia 自己派发通知的队列。
也就是说：mpv 的观察者回调在别人的线程上执行，然后跳到了 0。

## 2. 根因

`audio/out/ao_avfoundation.m`（v0.41.0，与当前 master 相同 —— 上游未修）：

```objc
// init()
p->observer = [[AVObserver alloc] initWithAO:ao];
[center addObserver:p->observer selector:@selector(handleRestartNotification:)
               name:AVSampleBufferAudioRendererWasFlushedAutomaticallyNotification
             object:p->renderer];

// 通知回调
- (void)handleRestartNotification:(NSNotification*)notification {
    …
    stop(ao);     // dispatch_sync(p->queue, ^{ … renderer flush / setRate:0 … })
    start(ao);    // requestMediaDataWhenReadyOnQueue(p->queue, ^{ feed(ao); })
}

// uninit()
stop(ao);
[p->renderer release];
dispatch_release(p->queue);                                  // ← 先释放
CFRelease(p->format_description);
[[NSNotificationCenter defaultCenter] removeObserver:p->observer];  // ← 后摘观察者
```

两个问题叠加：

1. **跨线程调用**：`stop()` / `start()` 只能在 AO 线程（播放线程）上跑，
   这里却在 CoreMedia 的通知队列上跑，直接并发访问 `p->renderer` / `p->synchronizer` / `p->queue`；
2. **释放顺序反了**：`uninit` 先 `dispatch_release(p->queue)`、`release` renderer，
   最后才 `removeObserver`。切换视频 / 关闭播放器时 `mpv_terminate_destroy`
   正在跑 uninit，而 flush 通知恰好到达 → `dispatch_sync` 打到一个已经释放的
   queue 对象上，就是栈里的「观察者回调 → 0x0」。

设备掉线正是最容易触发 `_wasFlushedAutomaticallyAtTime:` 的路径，所以表现为
「拔耳机必崩」。

## 3. 为什么不能简单地换成 coreaudio

本机实测：

```bash
$ mpv av://lavfi:sine=frequency=440:duration=1 --ao=coreaudio --no-video
[ao/coreaudio] unable to set the input channel layout on the audio unit (…/-50)
[ao] Failed to initialize audio driver 'coreaudio'
```

`features.toml` 里留 `coreaudio` 只是为了 `YK_AO` 兜底，默认仍然是
`avfoundation`。所以只能在 avfoundation 上修，不能靠「换个 AO」绕过。

## 4. 修复

补丁：[`packaging/libmpv/patches/0001-ao-avfoundation-notification-race.patch`](../packaging/libmpv/patches/0001-ao-avfoundation-notification-race.patch)

1. 回调只调用 `ao_request_reload()` —— 它只置一个原子事件并唤醒播放线程，
   是 mpv 为「设备变化」提供的线程安全入口；
2. 播放线程在 `fill_audio_out_buffers()` 里消费事件，走
   `reload_audio_output()` 重建 AO：新 renderer / synchronizer，`end_time_av`
   归位，不再跨线程碰 `priv`；
3. `uninit` 调整为 **先 `detach` + `removeObserver`，再 `stop` / 释放**；
4. `detach` 与通知回调共用 `@synchronized (self)`：摘除之后回调再也拿不到
   `ao` 指针，uninit 可以放心释放。

## 5. 构建集成

- 补丁清单写在 [`features.toml`](../packaging/libmpv/features.toml) 的
  `platform.<p>.patches`，`build-libmpv.sh` 在克隆完 mpv 后按顺序 `git apply`；
- 已应用的补丁用 `git apply --reverse --check` 识别，重复构建幂等；
- 补丁**内容参与产物指纹**：改补丁会让 `verify` 判定「需要重建」，
  不会继续复用旧 dylib；没有补丁的平台（Windows）指纹保持不变，不会白重编；
- 补丁打不上（mpv 升级改了上下文）时构建立刻退出 6，不会产出「只打了一半」的库。

## 6. 验证

```bash
# 1) 补丁能干净地打到 v0.41.0 上（反向检查证明幂等判定可用）
git -C <干净的 mpv v0.41.0> apply --check <patch>
git -C <同上> apply --check --reverse <patch>

# 2) 重编（会顺带验证「清单 → 补丁 → 编译 → 指纹」整条链路）
./scripts/build-libmpv.sh --platform macos

# 3) 产物里应该是新逻辑，旧 WARN 字符串已经不在
strings packaging/macos/vendor/libmpv.2.dylib | grep -c "restarting due to system notification"   # 0
strings packaging/macos/vendor/libmpv.2.dylib | grep -c "reloading audio output"                  # ≥1

# 4) 指纹 / 体积 / 导出
python3 scripts/libmpv_features.py verify --platform macos \
    --file packaging/macos/vendor/libmpv.2.dylib
```

人工复现路径（需要真机音频设备）：播放任意视频 → 拔掉耳机 / 断开蓝牙音箱
→ 应用应继续播（AO 重建），不再闪退。

## 7. 备注

- **Homebrew 的 libmpv 没打这个补丁**：`cargo run` 时如果加载的是
  `/opt/homebrew/lib/libmpv.2.dylib`，崩溃仍会复现；用
  `YK_LIBMPV=packaging/macos/vendor/libmpv.2.dylib` 指向自己编的产物即可验证修复。
- 截至 2026-11-05，mpv master 的 `ao_avfoundation.m` 与 v0.41.0 完全一致，
  属上游未修问题，可以按第 2 节的复现条件反馈上游。
