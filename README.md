# RustPods 🎧

RustPods 是使用 Rust 语言重构并全面升级的 Linux 端 AirPods 高级控制中心与系统服务。项目致力于在 Linux（Wayland / X11）环境下，实现媲美 macOS 的 AirPods 软硬件联动功能与质感优雅的原生图形界面。

---

## ✨ 核心特性

- 🎨 **macOS 质感原生 GUI (侧边栏分类布局)**：
  - 基于现代化的 **Libadwaita / GTK4** 设计，采用媲美 macOS 系统设置与控制中心的**左侧分类侧边栏 + 右侧独立页面**；
  - **概览首页 (Overview)**：展示设备形象图、一键重命名弹窗、连接状态胶囊以及左右耳/充电盒实时**圆形环状电量仪表盘**与快捷控制；
  - **耳机控制 (Controls & Gestures)**：集中管理噪声控制、入耳检测策略、对话感知、麦克风指定与充电盒提示音；
  - **偏好设置 (Preferences & Hardware)**：配置**关闭窗口最小化到任务栏 (常驻后台)** 开关、查看官方硬件代号 (A2564)、固件版本 (6A321) 与三耳出厂序列号。
- 🟢 **任务栏圆形动态电量图标 (`icon_pixmap`)**：
  - 符合 Freedesktop StatusNotifierItem 标准，托盘图标以 **多分辨率高清环形进度条** 动态显示当前最低耳机电量；
  - 电量低于 20% 警示红，20%~40% 黄色，高电量与充电状态呈苹果活力绿，并在充电时在中央实时绘制闪电徽标（`⚡`）；
  - 窗口关闭时自动隐藏至任务栏常驻，点击托盘即时唤醒主界面。
- ⚡ **原生 L2CAP AACP 协议栈**：
  - 基于 `bluer` 直接建立针对经典蓝牙 L2CAP PSM `0x1001` 的数据信道；
  - 实现 Apple 官方私有握手魔数、特性位激活（Feature Flags）与全量状态事件订阅；
  - 完整支持降噪模式切换（降噪 / 通透 / 自适应 / 关闭）、自适应降噪强度平滑滑动调节、按柄捏合手势自定义、麦克风模式偏好（自动/左/右）与充电盒提示音开关。
- 🎵 **智能媒体联动 (MPRIS & PipeWire)**：
  - **自动入耳检测 (In-Ear Detection)**：摘下一只耳塞或双耳时自动暂停本地播放器（如 YesPlayMusic、Spotify、VLC 或浏览器媒体），重新戴回时平滑恢复播放；
  - **对话感知 (Conversational Awareness)**：检测到用户开口说话时自动压低媒体音量，结束说话后平滑恢复原音量。
- 🧭 **头部追踪与点头/摇头手势（实验性）**：
  - 解析 RTBuddy `SensorV2` 传感器帧（固定 65 字节，字段偏移为帧内绝对值：orientation1 @43、orientation2 @45、orientation3 @47、水平加速度 @51、垂直加速度 @53）；
  - 前 10 个样本自动校准中立姿态，输出 pitch / yaw / roll 三轴角度并做 5 点滑动平均；
  - 移植 LibrePods 的置信度模型识别点头（是）与摇头（否），要求 3 个交替极值且加权置信度 ≥ 0.7；
  - 「空间音频」页以 Cairo 绘制实时罗盘并显示采样率与手势提示。
  - **状态：协议前半段已验证，数据流尚未拉通**，详见下方「已知限制」。
- 🔔 **系统托盘驻留 (`ksni`)**：
  - 符合 Freedesktop StatusNotifierItem 标准，无缝嵌入 Wayland（如 `niri`、`Waybar`）及 GNOME/KDE 状态栏；
  - 托盘右键菜单实时显示左右耳及充电盒电量，并提供快速降噪模式切换与主界面呼出入口。

---

## 🛠️ 项目架构

```
rustpods/
├── Cargo.toml
├── src/
│   ├── main.rs                    # 命令行入口、多线程 Tokio 异步运行时与应用生命周期
│   ├── core/                      # 核心协议与状态层
│   │   ├── aacp/                  # Apple Accessory Communication Protocol (L2CAP PSM 0x1001)
│   │   │   ├── commands.rs        # 控制命令编码（降噪模式、麦克风、按柄、自适应强度等）
│   │   │   ├── events.rs          # 状态通知解码（电量、入耳状态、按柄事件）
│   │   │   ├── packets.rs         # 握手包编解码、Opcode 路由分发
│   │   │   └── mod.rs             # AacpManager 连接、数据收发线程与事件广播
│   │   ├── models.rs              # AirPods 各代硬件型号库与能力矩阵映射
│   │   ├── device.rs              # AirPodsDeviceState 统一响应式数据模型
│   │   └── crypto.rs              # AES-128 与蓝牙 RPA 哈希验证 (ah)
│   ├── system/                    # 桌面生态与系统服务集成
│   │   ├── bluetooth.rs           # BlueZ 适配器管理、设备扫描与连接监听
│   │   ├── le_monitor.rs          # BLE Proximity 充电盒广播监听
│   │   ├── mpris.rs               # D-Bus MPRIS 播放器智能播放/暂停联动
│   │   ├── tray.rs                # StatusNotifierItem 动态圆形电量系统托盘
│   │   └── config.rs              # 用户偏好设置持久化 (~/.config/rustpods/config.json)
│   └── ui/                        # Libadwaita / GTK4 用户友好图形界面
│       ├── components/
│       │   ├── hero.rs            # 耳机形象展示、重命名弹窗、连接指示
│       │   ├── battery.rs         # 苹果圆形环状独立电量与充电动画仪表
│       │   ├── noise.rs           # 降噪/通透/自适应分段控制器与能力标签卡片
│       │   ├── preferences.rs     # 偏好设置与硬件信息面板
│       │   └── settings.rs        # 入耳检测、麦克风偏好、提示音设置组
│       ├── window.rs              # 主窗口 macOS 侧边栏布局与刷新逻辑
│       └── mod.rs                 # AdwApplication 与后台线程安全通信总线
│       ├── window.rs              # 主窗口 (AdwPreferencesWindow) 布局与刷新逻辑
│       └── mod.rs                 # AdwApplication 与后台线程安全通信总线
```

---

## 🚀 编译与运行

### 依赖项
- Rust 1.80+ (推荐最新稳定版)
- `gtk4` 和 `libadwaita` 开发库（Fedora 上运行 `sudo dnf install gtk4-devel libadwaita-devel`）
- `bluez` (>= 5.60) 与 `pipewire`

### 编译
```bash
cd /home/windosx/Dev/rustpods
cargo build --release
```

### 运行方式

1. **图形界面模式 (默认)**：
   ```bash
   ./target/release/rustpods
   ```

2. **后台守护进程模式 (无窗口，常驻托盘与 MPRIS 自动联动)**：
   ```bash
   ./target/release/rustpods --no-gui
   ```

3. **开启详细调试日志**：
   ```bash
   ./target/release/rustpods --debug
   ```

---

## 🧪 自动化与真机测试

运行项目测试套件：
```bash
cargo test
```
测试覆盖了 AACP 报文打包编解码、电量解析、入耳检测状态机、头部姿态解算以及各代耳机型号能力矩阵。
并在已配对的 AirPods 3 真机（`CC:68:E0:38:9C:E9`）上通过了 L2CAP PSM 0x1001 握手与硬件信息读取的端到端验证。

### 头部追踪诊断子命令

```bash
# 完整握手后启动头部追踪，逐秒打印是否在流、实测采样率
./target/debug/rustpods --debug --head-tracking-test CC:68:E0:38:9C:E9

# 复现 LibrePods 的最小序列（不跑 AACP 1.3 握手）
./target/debug/rustpods --no-handshake --head-tracking-test CC:68:E0:38:9C:E9

# 扫描候选 (service, interval) 组合，找出真正能拉起数据流的一组
./target/debug/rustpods --sweep --head-tracking-test CC:68:E0:38:9C:E9
```

---

## ⚠️ 已知限制：头部追踪数据流尚未拉通

以下结论均来自真机实测（AirPods 3 / A2564 / 固件 `65.1868046005000000.52`）。与网络上流传的部分说法不一致，记录在此以免重复踩坑。

### 已验证的事实

**1. `SensorV2` 样本格式**：固定 65 字节，字段偏移为帧内绝对值（`orientation1` @43、`orientation2` @45、`orientation3` @47、水平加速度 @51、垂直加速度 @53）。macOS 抓包中去除多线程重复记录后实测 **42–50 Hz，中位间隔 20.1 ms**。

**2. service_id 与 cookie 来自设备的 XML 广告帧头，且每次连接都会重新分配。** 这是本项目最关键的发现：

```
04 00 04 00 17 00 | 00 08 00 00 | 6D 02 | 00 1E 00 00 | 83 34 01 00 | <?xml ...
 AACP header        descriptor    len     service_id    cookie        plist
                    0x800
```

实测三次连接的结果（id 递增，证明不可硬编码）：

| 服务 | 连接的 service_id | cookie | MaxReportSize |
|------|------------------|--------|---------------|
| `iedrelay` | `0x1300` → `0x1B00` | `0x00013C88` | 40 |
| `neorelay`（6-DoF 头部姿态） | `0x1600` → `0x1E00` → `0x2600` | `0x00013483` | 28 |
| `mando` | `0x1700` → `0x1F00` | `0x00011F02` | 27 |

**3. AACP `0x02` 版本协商**：设备在握手后立即发送此帧，此前被当作未知 opcode 丢弃。设备端能力列表完整内容为 45 字节。现已解析并原样回显。

### 尚未解决的关键缺口

设备对 **14 字节 HID enable 完全无响应**（既无 9 字节 ACK，也无数据流）。已尝试两种候选布局，均失败：

- `[service_id:4][cookie:4][0xD2][0x01][interval_us:4]`
- `[0x01][service_id:u16][0x00][cookie:4][0xD2][0x01][interval_us:4]`

已排除的假设：

- ❌ service 编号问题：`6/7/8/9/14/16` × `10/20/40 ms` 共 18 种组合全部无响应
- ❌ 握手顺序问题：不跑 AACP 1.3、直接复现 LibrePods `INIT`+`START`，同样无效
- ❌ 被其他设备占用：`0x2E` 连接设备列表只含本机
- ❌ 音频未播放：抓包窗口内 A2DP 在 Idle/A2DP 间反复切换，SensorV2 始终流动
- ❌ 字节级抓包：`~/Downloads/bluetooth.log` 中 AACP 相关行原始字节数为 **0**，3158 处字段被 `<private>` 脱敏；此路不通

### macOS 还做了、我们尚未实现的步骤

对照抓包，macOS 在 enable 前还有这些动作，均未实现：

| macOS 动作 | 状态 |
|---|---|
| `feature capabilities mask 0x2f2ff` | 我们发 `0xD7`，不同 |
| 逐服务连接确认 `0x10→0x80000→0x1→0x8→0x100000` | 不等确认 |
| `sendTimestamp`（UNIX Epoch 秒+微秒） | 未实现 |
| `TimeSyncEnable` + Remote Timesync 注册 | 未实现 |
| Jitter Buffer（控制命令 `0x0B`） | 枚举中缺失 |

其中 `Audio Timesync: Sensor data support: Yes` 明确把时间同步与传感器数据绑定，是最可疑的缺口。

**结论：在补齐时间同步之前，「空间音频」页会一直显示「等待数据流」。** 其余功能（电量、入耳检测、降噪、按柄、对话感知）均已在真机验证正常。

---

## 🔬 与开源参考实现的对照（AirPods 3 实测结论）

为了确认上述缺口，本项目对照了所有能找到的真机实现，并把它们的做法逐一在 AirPods 3（A2564，固件 `65.1868046005000000.52`）上复现：

| 项目 | 平台 | 关键做法 | 在本机结果 |
|---|---|---|---|
| [anilsrml/airpods-posture](https://github.com/anilsrml/airpods-posture) | **Linux** | HANDSHAKE + 单发 START（service 16, seq 115） | ❌ 无流 |
| [StarNumber12046/airpods-head-track](https://github.com/StarNumber12046/airpods-head-track) | Windows | HANDSHAKE + 特性位 + 通知 + START（service 14） | ❌ 无流 |
| [batubozkan/pods-head-tracker](https://github.com/batubozkan/pods-head-tracker) | Linux (Pi) | 上述 + `BT_SECURITY=MEDIUM` + **每 4 秒交替重发 START** | ❌ 无流（90 秒 / 20 次重发） |
| [tomppi/airpods_rtbuddy_v37_probe](https://github.com/tomppi/airpods_rtbuddy_v37_probe) | Windows | 心率服务（service 19），非头部追踪 | — |

三个真 bug / 真机制是在这一轮里被找出来并修掉的：

1. **`BT_SECURITY = MEDIUM`（SOL_BLUETOOTH=274 / opt 4）**：BlueZ 要求 PSM `0x1001` 链路加密，普通 raw L2CAP socket 只申请 low security。缺少这一步时有些环境下 `connect()` 直接返回 `EACCES`。
2. **START 必须周期性重发**：耳机入盒时发出的 START 不会生效，必须持续重发以捕捉"戴上"的时刻；`pods-head-tracker` 用 4 秒间隔并交替两种变体。
3. **流帧判据是 `len >= 70` 且 `p[10] ∈ {0x44, 0x45}`**，不是"固定 65 字节"。

### 为什么 AirPods 3 仍然不出流

抓包显示 SensorV2 出现在 `Spatial Audio Mode = 2 (Head Track)` 被激活后 **31 ms**：

```
L9392  UpdateSpatialAudioCCStates Streaming = 1 ... Mode = 2 Head Track
L9409  Sent custom message type 0x800 len 14      ← 14 字节 enable
L9428  Custom msg received [SensorV2], len: 65    ← 流开始
```

而 iOS/macOS 发送的 AACP 控制命令清单里（`01,06,0A,0B,12,13,17,18,1A,1F,20,24,32,36,3C,3E`）**没有任何一条是空间音频模式**。也就是说该状态由 coreaudiod / WindowServer 在系统侧置位，再配合 A2DP 实际播放音频才生效，**不通过 AACP 报文**。

`pods-head-tracker` 的作者只在 **AirPods 4 (ANC)** 上验证过，README 明确写着其他型号未测试。AirPods 3 的固件很可能把该流锁在 macOS 的空间音频激活流程之后。

### 帧长度语义（易错点）

macOS 日志里的 `len` 是 **body 长度**，不是线路上的帧长度：

| 日志 | 含义 | 线路长度 |
|---|---|---|
| `Sent custom message type 0x800, len 14` | HID enable body 14B | 12 + 14 = 26B |
| `Custom msg received [SensorV2], len: 65` | 样本 body 65B | 12 + 65 = 77B |
| `Received ... type 0x800, len 9` | enable 的 ACK | 12 + 9 = 21B |

而参考实现 `pods-head-tracker` 的判据是 `len >= 70 && p[:10] == 04 00 04 00 17 00 00 00 10 00 && p[10] ∈ {0x44,0x45}`，其中 `p[10..12]` 是 body 长度字段（`0x44`=68 / `0x45`=69），因此**真实样本帧是 80/81 字节**，且内部带标记 `3a 3e 08 10 1a 3a`。这两种格式本项目都已支持解析。

### 最后一轮实测（条件完备，仍失败）

在**双耳在耳**（`left: InEar, right: InEar`）、**音乐正在播放**、设备正常响应（电量 64%/67%）的条件下，使用从本次连接广告中读出的真实 service id，发送与 macOS 录制载荷逐字节一致的 14 字节 enable，并每 4 秒重发，覆盖：

- descriptor `0x800` / `0x100000`
- interval `20000` / `40000` µs
- 两种 START 变体
- 8 字节探测帧（模拟 macOS 首个 `0x800 len 8`）
- `OwnsConnection=1`、`BT_SECURITY=MEDIUM`

**设备对所有 enable 变体均无任何回应**（连 macOS 会回的 9 字节 ACK 都没有）。会话 id 每连接递增 8（19→27→…→166），确认解析正确。


### 若目标是"Linux 上的空间音频"，可行路线不依赖 AirPods 传感器

经检索，**目前没有任何开源项目实现「AirPods 传感器 → PipeWire 空间音频」**。已实现的 PipeWire 空间音频方案都用其他姿态来源：

| 项目 | 做法 |
|---|---|
| [qeqqe/spatial-track](https://github.com/qeqqe/spatial-track) | PipeWire `filter-chain` + SOFA HRTF，姿态来自 **opentrack 神经网络人脸追踪** |
| Collaplex-Spatial-Audio / kz-dsp / timbre | 纯 PipeWire DSP（HRTF/EQ/混响），无头部追踪 |
| HanreichC/MyPods 的 "spatial audio" | 实为 AACP 音效（EQ），非头部追踪 |

因此若要在 Linux 上获得头追空间音频，推荐路线是 **PipeWire HRTF + opentrack（人脸追踪）**，而非等待 AirPods 传感器可用。



