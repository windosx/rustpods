<p align="center">
  <img src="assets/banner.png" alt="RustPods 横幅" width="520">
</p>

<h1 align="center">RustPods</h1>

<p align="center">
  <strong>面向 Linux 的 AirPods 控制中心与系统服务。</strong><br>
  原生 Rust、GPU 加速界面、头部追踪空间音频，以及用头部操控的光标。
</p>

<p align="center">
  <a href="README.md">English</a> · <strong>简体中文</strong>
</p>

<p align="center">
  <a href="LICENSE"><img alt="许可证：AGPL-3.0" src="https://img.shields.io/badge/license-AGPL--3.0-blue.svg"></a>
  <img alt="平台：Linux（Wayland / X11）" src="https://img.shields.io/badge/platform-Linux%20%7C%20Wayland%20%7C%20X11-1d1d1f?logo=linux&logoColor=white">
  <img alt="Rust：1.85+" src="https://img.shields.io/badge/rust-1.85%2B-orange?logo=rust">
  <a href="https://github.com/windosx/rustpods/issues"><img alt="GitHub issues" src="https://img.shields.io/github/issues/windosx/rustpods"></a>
</p>

---

RustPods 把 macOS 上习以为常的 AirPods 体验带到 Linux 桌面：电量与入耳状态、降噪控制、真正像控制中心的界面；开启头部追踪后，还能获得固定在空间中的声场，以及用转头驱动光标的空鼠。

它直接通过 L2CAP 与 Apple AACP 协议通信，不需要桥接守护进程、不需要给 BlueZ 打补丁，也不需要一台 Android 手机中转。

> [!NOTE]
> RustPods 仍处于早期阶段（v0.1.0）。头部追踪已在 AirPods 3 上通过 Apple RemoteHID 完成端到端验证；较新的型号使用 `src/core/head_tracking.rs` 中记录的载荷布局。遇到问题请[提交 issue](https://github.com/windosx/rustpods/issues)。

## 功能特性

### 苹果控制中心风格界面

- 基于 [GPUI](https://crates.io/crates/gpui-kit) 的 GPU 渲染：圆角卡片、半透明材质，深浅色主题即时切换。
- 六个页面：**概览**、**空间音频**、**姿态鼠标**、**控制与交互**、**智能体接入**、**偏好设置**。
- 概览页展示设备渲染图、就地重命名、连接状态，以及左右耳和充电盒各自的环状电量仪表。
- 按型号识别能力：当前耳机不支持的控件会隐藏，而不是静默失败。

### 头部姿态鼠标

- 120 Hz 运动引擎：以角速度驱动光标，转头时光标平滑移动，停下时立刻驻留。
- 1-Euro 自适应滤波：静止时强平滑，快速转头时降低延迟。
- 软死区加非线性加速度曲线：小角度微调可精准定位，大角度甩头可跨越超宽屏和多显示器。
- 支持相对**空鼠模式**、绝对**激光指针模式**，以及把俯仰映射为滚轮的滚动模式。
- 手势：耳柄单击/双击/三击、长按暂停、歪头点击，以及可选的静止悬停点击。
- `/dev/uinput` 尚未授权时，通过 Polkit 一键申请权限。

### 头部追踪空间音频（PipeWire + SOFA）

- 按 ITU-R BS.775 构建前方 ±30° 的真实立体声音箱舞台，由两个独立的 SOFA 空间化器（`spatL` / `spatR`）渲染。
- 偏航、俯仰、横滚投影到头部坐标后，原子写入 PipeWire filter-chain。
- 5° 听感量化加 50 ms 重建节流，避免 100 Hz 传感器流占满 CPU。
- HRTF 播放流是被动节点，跟随当前默认输出；应用按流路由到虚拟节点，而不是把虚拟节点设为系统默认。

### 点头代答智能体选择题

- 当 ZCode 等桌面智能体触发 `AskUserQuestion` 时，`PreToolUse` hook 会把问题转发给 RustPods。
- **点头 N 次即选择第 N 项**；摇头取消；超时自动回退到原生界面。
- 带迟滞的状态机与自适应静止基准，即使仰卧或低头阅读也能稳定识别。
- 问题显示在独立的浮动卡片中（Wayland Layer Shell / X11 悬浮窗）。

### 开盖弹窗与系统托盘

- 通过 BLE Proximity 监听充电盒开盖，弹出 iOS 风格卡片、对应型号的 360° 动画和实时电量。
- 基于 Freedesktop StatusNotifierItem 的托盘图标（`ksni`），兼容 Waybar、Niri、Hyprland、Sway、GNOME 和 KDE。
- 托盘环形图标显示当前耳塞电量：低电量变红，充电时呈绿色并带闪电徽标。
- 右键菜单提供实时电量、降噪模式切换和主界面呼出。

### 纯 Rust AACP 协议栈

- 通过 `bluer` 在经典蓝牙 PSM `0x1001` 上建立纯 Rust L2CAP 信道。
- 自动宣告全部传感器描述符（`request_all_descriptors`），并以约 100 Hz 启动 Apple RemoteHID（`0x800`）。
- 从解码后的 AACP 通知驱动入耳检测（摘下暂停、戴上恢复）和对话感知。
- AES-128 载荷解密与 RPA 哈希校验位于 `src/core/crypto.rs`。

## 设备兼容性

RustPods 可识别 AirPods 1–4、AirPods 4（ANC）、AirPods Pro 1/2（Lightning 与 USB-C）以及 AirPods Max（Lightning 与 USB-C），并且只展示对应型号真正实现的控件。

| 能力 | 支持型号 |
| --- | --- |
| 电量、入耳检测、型号识别 | 全系列 |
| 降噪、通透、自适应音频 | 支持该能力的型号（如 AirPods Pro 2、AirPods 4 ANC、AirPods Max） |
| 对话感知 | AirPods Pro 2、AirPods 4 ANC |
| 头部追踪 | AirPods 3 经 RemoteHID（`0x800`），已端到端验证；AirPods 4 载荷布局已实现 |

## 系统要求

- Linux，运行中的 **BlueZ**（>= 5.60），以及支持经典蓝牙的适配器。
- **Rust 1.85+**（项目使用 edition 2024）。
- 头部追踪空间音频需要 **PipeWire** 和 SOFA filter-chain 模块：
  - Fedora：`sudo dnf install pipewire-module-filter-chain-sofa`
  - Debian/Ubuntu：`sudo apt install libpipewire-0.3-modules`
- 头部姿态鼠标需要 **`/dev/uinput`** 访问权限，RustPods 可以通过 Polkit 申请。
- 构建依赖：`clang` / `libclang`、`pkg-config` 和 PipeWire 头文件。
  - Fedora：`sudo dnf install clang pipewire-devel pkg-config`
  - Debian/Ubuntu：`sudo apt install clang libclang-dev libpipewire-0.3-dev pkg-config`

`audio-follow` 是默认 Cargo 特性，因为 `pipewire-sys` 需要上述头文件。如果只想构建其余功能：

```bash
cargo build --release --no-default-features
```

界面、托盘、AACP 协议栈和姿态鼠标仍然可用，仅头部追踪音频输出不可用。

## 编译

```bash
git clone https://github.com/windosx/rustpods.git
cd rustpods
cargo build --release
```

产物位于 `target/release/rustpods`。

## 安装

把二进制和桌面文件安装到用户目录：

```bash
install -Dm755 target/release/rustpods ~/.local/bin/rustpods

install -Dm644 contrib/io.github.rustpods.RustPods.desktop \
  ~/.local/share/applications/io.github.rustpods.RustPods.desktop

install -Dm644 assets/icons/hicolor/scalable/apps/rustpods.svg \
  ~/.local/share/icons/hicolor/scalable/apps/io.github.rustpods.RustPods.svg

# 可选：为不加载 SVG 的面板安装 PNG 尺寸。
for size in 16 24 32 48 64 128 256 512 1024; do
  install -Dm644 "assets/icons/hicolor/${size}x${size}/apps/rustpods.png" \
    "$HOME/.local/share/icons/hicolor/${size}x${size}/apps/io.github.rustpods.RustPods.png"
done

update-desktop-database ~/.local/share/applications 2>/dev/null || true
gtk-update-icon-cache -f -t ~/.local/share/icons/hicolor 2>/dev/null || true
```

确保 `~/.local/bin` 在 `PATH` 中。

## 使用

```bash
rustpods                  # 打开控制中心
rustpods --no-gui         # 仅托盘守护模式（适合开机自启）
rustpods --debug          # 输出详细日志
rustpods --log-file PATH  # 同时把日志写入文件
```

再次启动 GUI 会激活已有窗口，而不是启动第二个引擎。所有 AACP 模式共享同一个会话级锁，因为两个引擎会同时占用耳机，导致双方都收不到响应。

### 姿态鼠标快速上手

1. 打开侧边栏的**姿态鼠标**页面。
2. 打开开关。如果 `/dev/uinput` 不可写，会弹出 Polkit 授权窗口；验证一次后驱动立即生效。
3. 常用手势：
   - 日常使用建议保持**空鼠模式**：头部轻微转动即可精准移动光标。
   - 长按任意耳柄可临时暂停或恢复姿态控制，适合说话或转头时使用。
   - 三击耳柄切换滚动模式，随后抬头或低头即可平滑翻阅长页面。

### 头部追踪空间音频

1. 下载 SOFA 数据集（例如 SADIE II Neumann KU100 或 MIT KEMAR），放到固定目录：

   ```bash
   mkdir -p ~/.local/share/rustpods/sofa
   # 将例如 D1_48K_24bit_256tap_FIR_SOFA.sofa 复制到这里
   ```

2. 安装 filter-chain 配置：

   ```bash
   mkdir -p ~/.config/pipewire/pipewire.conf.d
   cp contrib/pipewire/99-rustpods-hrtf.conf ~/.config/pipewire/pipewire.conf.d/
   ```

3. 编辑复制后的文件，把**两处** `filename` 都改成 SOFA 文件的绝对路径。PipeWire 不会展开 `~`，相对路径会静默失败。

4. 重启 PipeWire，并打开**空间音频**页面：

   ```bash
   systemctl --user restart pipewire
   ```

5. 把需要空间化的播放流路由到 **RustPods Head-Tracked Stereo**（例如在 `pavucontrol` 的“播放”标签页或用 `pw-link`）。**不要**把它设为系统默认输出：HRTF 播放流是被动节点，会跟随真实的默认输出，这样音量键和其他桌面行为仍作用于物理设备。

### 智能体点头代答

**智能体接入**页面可以安装 ZCode 的 `PreToolUse` hook 并查看状态。安装后，ZCode 的问题会显示在浮动卡片中；按提示点头若干次、摇头取消，或等待超时回到终端原生界面。Hook 通过 Unix socket 与 RustPods 通信，服务未运行时始终回退到原生提示。

## 模块架构

```text
rustpods/
├── Cargo.toml
├── LICENSE
├── NOTICE                         # 第三方署名与许可
├── README.md                      # English
├── README_zh.md                   # 本文件
├── contrib/
│   ├── io.github.rustpods.RustPods.desktop
│   └── pipewire/99-rustpods-hrtf.conf
├── assets/                        # 横幅、图标、360° 设备动画
└── src/
    ├── main.rs                    # 入口、命令行解析、Tokio 运行时、单实例锁
    ├── core/                      # 蓝牙协议与传感器数学
    │   ├── aacp/
    │   │   ├── commands.rs        # 降噪模式、麦克风偏好、自适应强度
    │   │   ├── events.rs          # 电量、入耳状态、耳柄按键
    │   │   ├── packets.rs         # AACP 报文打包/解包与 Opcode 分发
    │   │   ├── remotehid.rs       # 描述符 0x800 上的 Apple RemoteHID
    │   │   ├── rtbuddy.rs         # RTBuddy 传感器公告与采样率协商
    │   │   └── mod.rs             # AacpManager 连接生命周期与事件广播
    │   ├── head_tracking.rs       # 姿态积分、重力参考系、陀螺仪零偏
    │   ├── nod_counter.rs         # 点头手势状态机
    │   ├── spatial.rs             # 空间音频会话状态
    │   ├── models.rs              # AirPods 型号与能力矩阵
    │   ├── device.rs              # 响应式设备状态
    │   └── crypto.rs              # AES-128 解密与 RPA 哈希校验
    ├── system/                    # 桌面集成与设备驱动
    │   ├── head_mouse/
    │   │   ├── one_euro.rs        # 1-Euro 自适应滤波
    │   │   ├── engine.rs          # 死区、加速度曲线、手势点击
    │   │   ├── uinput.rs          # /dev/uinput 驱动与 Polkit 授权
    │   │   ├── controller.rs      # 120 Hz 控制循环
    │   │   └── mod.rs
    │   ├── audio_follow.rs        # PipeWire 方位角/俯仰角实时更新
    │   ├── bluetooth.rs           # BlueZ 适配器管理与自动连接
    │   ├── le_monitor.rs          # BLE Proximity 开盖监听
    │   ├── agent_hooks.rs         # 智能体 Hook 注册表
    │   ├── zcode_ask.rs           # ZCode AskUserQuestion Unix Socket 服务
    │   ├── mpris.rs               # MPRIS 媒体控制
    │   ├── tray.rs                # StatusNotifierItem 托盘
    │   └── config.rs              # 原子化配置持久化
    └── ui/                        # GPUI 界面
        ├── components/            # 概览、电量、降噪、姿态鼠标等页面
        ├── ask_popup.rs           # 智能体提问浮动卡片
        ├── popup.rs               # 开盖 360° 动画弹窗
        ├── theme.rs               # 苹果控制中心配色
        ├── state.rs               # 响应式状态总线
        ├── window.rs              # 侧边栏导航与页面装配
        └── mod.rs                 # GPUI 应用生命周期
```

## 测试

```bash
cargo test
```

测试覆盖 AACP 打包与分发、AES-128 解密、姿态积分与陀螺仪零偏估计、1-Euro 滤波、死区与手势数学、配置迁移以及型号能力映射。

提交 PR 前请同时运行：

```bash
cargo clippy --all-targets
```

## 参与贡献

欢迎提交 issue 和 pull request。涉及协议或头部追踪的改动，请附上测试所用 AirPods 型号与固件版本，并尽量提供日志或抓包。不同型号和固件的追踪行为差异很大，在一副耳机上有效的改动不一定适用于整个系列。

## 致谢

RustPods 建立在早期逆向工程与开源项目的成果之上：

- **[LibrePods](https://github.com/kavishdevar/librepods)**（AGPL-3.0）—— AACP 命令与事件编码、L2CAP 传输模型和手势检测。RustPods 的手势检测器移植自 LibrePods 的 `GestureDetector`。
- **[MousePods](https://github.com/starman-tech/mousepods)**，作者 Matteo Gaudioso（[@starman-tech](https://github.com/starman-tech)），Apache-2.0 —— 首个公开的 Apple RemoteHID（HID over AACP）文档，以及 RustPods 姿态解算与空鼠动力学所遵循的 `MotionEngine` 数学。**如果你也基于这些工作开发，请告知作者：开一个 issue 或提及 @starman-tech。** 学术用途请通过 MousePods 的 [CITATION.cff](https://github.com/starman-tech/mousepods/blob/main/CITATION.cff) 引用。
- **[anilsrml/airpods-posture](https://github.com/anilsrml/airpods-posture)** —— 较新 AirPods 固件的传感器载荷布局，包括 AirPods 4 使用的标记相对偏移。
- **[SpriteOvO/AirPodsDesktop](https://github.com/SpriteOvO/AirPodsDesktop)**（GPL-3.0）—— `assets/animations` 下的 360° AirPods 动画由其色键动画素材转换而来。

完整的第三方署名与许可信息见 [NOTICE](NOTICE)。

## 许可证

RustPods 以 **[GNU Affero General Public License v3.0](LICENSE)** 开源。

第三方作品保留其原始许可：

- MousePods —— Apache-2.0，Copyright Matteo Gaudioso。
- AirPodsDesktop 动画素材 —— GPL-3.0。

---

AirPods、macOS 和 Safari 是 Apple Inc. 在美国及其他国家/地区的注册商标。RustPods 是独立的开源社区项目，与 Apple Inc. 无附属、背书或赞助关系。