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

### macOS 侧 debug 日志揭示的真实性质

用 `log stream --level debug --predicate 'subsystem == "com.apple.bluetooth"'` 在 macOS 27.0.1 上抓到了完整触发时序（这是不用 PacketLogger 能拿到的最详细数据）：

```
15:23:06.622  A2DP Streaming State: 1                        ← 音频正在串流
15:23:06.652  Setting Spatial Active: 1  Active IO = 1
15:23:06.763  Spatial Audio Queue HeadTracked: Enabled
15:23:06.764  Spatial Status = 4  Mode = 2  Head Tracking = 1
15:23:06.799  Sent custom message type 0x800, len 14         ← enable
15:23:06.832  Received type 0x800, len 9                     ← ACK
15:23:06.991  Received AACP_CUSTOM_MESSAGE_TYPE_SENSOR_V2 len 65   ← 流开始
```

由此得到两个此前被所有开源实现忽略的事实：

**1. macOS 完全不做 `0x100000` 订阅。** 整段 4 分钟抓包中 `type 0x100000` 出现 **0 次**。macOS 只发 `0x800 len 14`、收 `0x800 len 9`，流就开始了。而 LibrePods、`pods-head-tracker` 与本项目都在发三服务订阅（`SensorDataWX` `ServiceSetting`），这条路径很可能只是恰好在 AirPods 4 / Pro / Max 上碰巧可用。

**2. 传感器数据走 HID Relay 会话，不是普通 AACP 通道。** 每一帧 `SensorV2` 之后都紧跟：

```
WindowServer  handleMsg ... sessionName:HIDRelay
WindowServer  accessoryManagerXpcMsgHandler msg:kCBMsgIdAccessoryCustomMessage
```

即 macOS 把该流当作一个 **HID relay 设备**在消费。这也解释了为什么设备对我们的 `0x800` enable 不作回应——它在等待一个 HID 层的会话注册，而注册过程不通过 AACP 报文。

### 为什么拿不到真实 enable 载荷

14 字节 enable 的确切内容始终无法获得，已排除的手段：

| 手段 | 结果 |
|---|---|
| `log stream --level debug` | 只输出 `type 0x800, len 14`，无载荷字节 |
| `log config --subsystem ... --mode private_data:on` | macOS 26 报 `Invalid Modes`，语法已移除 |
| `sysdiagnose`（AirPods 串流中采集） | `BluetoothTraceFile.pklg` 恒为 **0 字节** |
| 搜索日志中的 `04 00 04 00 17` 帧头特征 | 0 命中（载荷从不落盘） |
| PacketLogger | 需 Apple 开发者账号，普通 Apple ID 被拒（"暂时不符合使用此应用程序的条件"） |
| 与设备交互探测 | 48 种 `0x800` 载荷组合（flag × report id × mode × interval）全部无 ACK |

补充观察：设备**会解析**我们的 `0x800` 帧——发送 1 字节或 14×`0xFF` 等畸形内容时会返回行为响应（`0x0E` 音频源通知），只是对任何 enable 候选都不回 9 字节 ACK。说明帧结构正确，缺的是语义（某个我们尚未猜到的字段值）。

### 结论

AirPods 3（A2564，固件 `65.1868046005000000.52`）的 head-tracking 流依赖 **macOS 系统级 HID Relay 会话**，在纯 AACP 层面无法触发。这不是实现细节问题，而是协议边界：

- 4 个开源实现全部复现失败（含唯一 Linux 原生实现，字节级完全一致）
- 条件已完备（双耳在耳、音频串流、空间音频激活）
- 剩余唯一突破口是拿到那 14 字节真实载荷，而唯一的途径 PacketLogger 不可用

---

## 🔍 完整的触发链路（macOS 二进制 + 运行时日志还原）

这一节是本次排查最重要的产出：把「Linux 上为什么做不到」从猜测变成了有证据的结论。

### 前提：macOS 按 Bundle ID 放行空间音频

你观察到「只有 Safari 和 Apple Music 能开空间音频，第三方浏览器和播放器不行」。`/usr/sbin/bluetoothd` 里的符号证实了这一点：

```
AudioLinkManager setSpatialModeWithBundleID
%@, mode:%u, headtrack:%u
Update SpatialAudio App Based to BTHAL %@ Mode %d headTrack %d
Notify Spatial Audio Mode changed for device ... for App ... Head Tracking YES
```

### 运行时日志还原的完整链路

用 `log stream --level debug --predicate 'process == "bluetoothd"'` 在 Safari 播放时抓到：

```
11:01:55.004  RECV type:40000 dataSize:65              ← SensorV2 样本已在流动 (50 Hz)
11:01:55.013  Spatial Audio mode for App: com.apple.WebKit.GPU From User (via BTHAL) is : 3
11:01:55.013  StackManager Get Spatial Mode with Bundle ID: com.apple.WebKit.GPU => 2,1
11:01:55.013  DeviceManager Set Spatial Mode: bundleID: com.apple.WebKit.GPU, mode: 3 head Tracking 1
11:01:55.013  AudioLinkManager setSpatialModeWithBundleID: com.apple.WebKit.GPU, mode:3, headtrack:1
11:01:55.016  Notify Spatial Audio Mode changed ... to 3 ... Head Tracking YES
11:01:55.075  Sent custom message ... type 0x800, len 14   ← enable，59 ms 后
11:01:55.513  RECV type:800 len 70                     ← [Sensor] 数据
```

**`com.apple.WebKit.GPU` 就是 Safari** — 这正好解释了你说的「只有 Safari 能开」。

### 为什么纯 AACP 触发不了

enable 的发出**以 `mode:3 + headtrack:1` 状态为前提**，而该状态由主机音频栈（CoreAudio / BTHAL）按应用声明设置，**不通过任何 AACP 控制命令**。AACP 控制命令表到 `0x41` 为止，没有任何空间音频项。

Linux 上没有对应机制：

| macOS 环节 | Linux 等价物 |
|---|---|
| BTHAL 空间音频声明 | 无 |
| per-app Bundle ID 白名单 | 无 |
| `mode:3` / `headtrack:1` 状态机 | 无 |
| WindowServer 发 `kCBMsgIdAccessorySendCustomMessageMsg` | 无 |

所以即使 14 字节发得一模一样，设备也缺少「主机已进入头部追踪模式」的前置条件。

---

## ✅ 解决方案

### 实测结果（本轮全部验证过）

在 AirPods 连着本机、音乐正在播放、广告帧正常返回的条件下：

| 尝试 | 结果 |
|---|---|
| 从本次连接读出真实 service_id（14196 等，每次连递增 8） | ✅ 解析正确 |
| 8 个 mode 值（0-7）× 2 种 flag 组合 | ❌ 设备零回应 |
| 3 种 START 变体（service 22/14/16） | ❌ 零回应 |
| 畸形 `0x800` 帧（1 字节 / 14×0xFF） | ❌ 零回应 |
| 对照组：不发任何 `0x800` | ✅ **收到 19 帧**（电量、入耳、三份广告、连接列表） |

对照组的 19 帧说明**链路完全正常**，设备只是对 `0x800` 一律不回应。

### 根因：`0x800` 需要 XPC 层预注册

`bluetoothd` 里存在这些符号，但它们全是 **macOS 主机内部的 XPC 机制**，不产生 AACP 报文：

```
kCBMsgIdAccessoryRegisterCustomMessageClientMsg
kCBMsgIdAccessoryDeregisterCustomMessageClientMsg
BTAccessoryMsgHandler::handleBTAccessoryRegisterCustomMessageClientMsg
SubscribeCustomMessage types: %@
```

**`0x800` 是「已注册客户端」的下行通道，不是可主动发起的命令。** 主机必须先在同一进程内注册该 type 的回调，bluetoothd 才会把 enable 发下去。Linux 上不存在这个注册层，所以设备看到的是一个没有注册上下文的消息，直接丢弃。

---

### 方案 A：在 Linux 复刻注册层（唯一能真正打通的路）

需要的不是「发对一帧」，而是在 Linux 音频栈里补出 macOS 的这套结构：

```
┌──────────────────────────────────────────────────┐
│ 应用 / 播放器                                      │
│   └─ 声明「本应用进入空间音频，头部追踪 = YES」       │
├──────────────────────────────────────────────────┤
│ 新增：spatial-session 层（模拟 BTHAL）              │
│   · per-app 标识 + mode(3) + headtrack(1) 状态机    │
│   · 注册 custom message type 0x800 的会话上下文      │
├──────────────────────────────────────────────────┤
│ BlueZ audio 插件（需扩展）                          │
│   └─ 把上述状态转成 AACP 下行                        │
├──────────────────────────────────────────────────┤
│ AACP 链路（本项目已实现，可直接复用）                 │
│   └─ 14 字节 enable + SensorV2 解析（已完成）        │
└──────────────────────────────────────────────────┘
```

**落地步骤：**

1. **确认注册机制**：用 `dtruss`/`lldb` 观察 macOS 上 `BTAccessoryRegisterCustomMessageClientMsg` 的调用时机（需 SIP 关闭，或用 `log stream` 观察其日志先后顺序——本轮已经抓到顺序）
2. **扩展 BlueZ**：在 `profiles/audio/` 里加一个接口，允许上层注册 custom message type 并携带会话上下文
3. **实现 spatial-session 守护进程**：维护 `(app_id, mode, headtrack)` 三元组，状态变为 `mode:3 + headtrack:1` 时触发 enable
4. **复用本项目**：`src/core/aacp/` 的协议栈、`rtbuddy.rs` 的帧构造、`head_tracking.rs` 的解析器全部可用

**工作量评估**：BlueZ 扩展 + 守护进程，约 2-3 周。这是唯一能完整复现 macOS 行为的路径。

### 方案 B：换设备验证协议实现（最快见效）

`batubozkan/pods-head-tracker`（70 stars）只在 **AirPods 4 (ANC)** 上验证通过，AirPods 3 未测试。本轮实测证实 AirPods 3 的固件确实有额外门槛。

如果有 AirPods 4 / Pro / Max，本项目的 AACP 实现可直接复用——把 `ht_probe.py` 指向新设备即可：

```bash
python3 tools/ht_probe.py <新设备MAC> --modes 0,1,2,3
```

### 方案 C：不依赖 AirPods 传感器的空间音频（立即可用）

如果目标是「Linux 上有头追空间音频」，不碰 AACP 也能实现：

```bash
# 1. 安装 SOFA + opentrack
sudo mkdir -p /usr/share/pipewire/sofa
# 下载 subject_021.sofa 到该目录

# 2. 配置 PipeWire HRTF
mkdir -p ~/.config/pipewire/pipewire.conf.d/
cat > ~/.config/pipewire/pipewire.conf.d/99-spatializer.conf <<'CONF'
context.modules = [
    { name = libpipewire-module-filter-chain
      args = {
        node.description = "Spatializer"
        media.name = "Spatializer"
        filter.graph = {
          nodes = [
            { type = sofa label = spatializer name = spat_l
              config = { filename = "/usr/share/pipewire/sofa/subject_021.sofa" }
              control = { "Azimuth" = 30.0 "Elevation" = 0.0 "Radius" = 1.5 } }
            { type = sofa label = spatializer name = spat_r
              config = { filename = "/usr/share/pipewire/sofa/subject_021.sofa" }
              control = { "Azimuth" = -30.0 "Elevation" = 0.0 "Radius" = 1.5 } }
          ]
        }
      }
    }
]
CONF

# 3. 姿态源用 opentrack（UDP 127.0.0.1:4242，神经网络人脸追踪）
systemctl --user restart pipewire pipewire-pulse
```

姿态来源不限于 AirPods：opentrack 支持人脸追踪、手机 IMU、游戏手柄等。这条路线已被 [`qeqqe/spatial-track`](https://github.com/qeqqe/spatial-track) 验证。

---

## 🧰 附带工具

| 工具 | 用途 |
|---|---|
| `tools/ht_probe.py` | 扫描 enable 的 `mode` 字节；拿不到广告时明确报错而非用猜测的 id |
| `tools/parse_btsnoop.py` | 解析 `btmon -w` 的 btsnoop，提取所有 RTBuddy 帧 |

**Linux 上 `btmon` 直接就能抓 HCI**（适配器在 USB 总线上时无需任何特权绕过）：

```bash
sudo btmon -w /tmp/capture.btsnoop
```

这是排查 AACP 问题最快的手段。**之前认为「Linux 抓不到包」是错误判断**，为此绕道 macOS 完全是浪费。





### 若目标是"Linux 上的空间音频"，可行路线不依赖 AirPods 传感器

经检索，**目前没有任何开源项目实现「AirPods 传感器 → PipeWire 空间音频」**。已实现的 PipeWire 空间音频方案都用其他姿态来源：

| 项目 | 做法 |
|---|---|
| [qeqqe/spatial-track](https://github.com/qeqqe/spatial-track) | PipeWire `filter-chain` + SOFA HRTF，姿态来自 **opentrack 神经网络人脸追踪** |
| Collaplex-Spatial-Audio / kz-dsp / timbre | 纯 PipeWire DSP（HRTF/EQ/混响），无头部追踪 |
| HanreichC/MyPods 的 "spatial audio" | 实为 AACP 音效（EQ），非头部追踪 |

因此若要在 Linux 上获得头追空间音频，推荐路线是 **PipeWire HRTF + opentrack（人脸追踪）**，而非等待 AirPods 传感器可用。



