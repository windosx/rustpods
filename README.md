# RustPods 🎧

RustPods 是专为 Linux（Wayland / X11）环境打造的 AirPods 高级控制中心与系统服务。项目基于 Rust 语言开发，采用高性能 GPU 加速 GUI 引擎（GPUI），致力于在现代 Linux 桌面环境下带来媲美甚至超越 macOS 的软硬件交互联动与质感优雅的使用体验。

---

## ✨ 核心特性

- 🎨 **苹果控制中心风格原生 GUI (GPU 加速)**：
  - 基于现代化 **GPUI (`gpui-kit`)** 框架构建，GPU 硬件直通渲染，轻量极速、零撕裂；
  - **苹果设计语言**：圆角卡片、细腻动态毛玻璃质感与语义化配色体系，支持深色（Dark）/ 浅色（Light）模式即时无缝切换；
  - **六大独立模块页面**：
    - **概览首页 (Overview)**：展示设备 3D 渲染图、一键重命名输入框、连接状态指示，以及双耳与充电盒的独立环状电量仪表盘与降噪快捷控制；
    - **空间音频 (Spatial Audio)**：实时动态 3D 头姿罗盘渲染与三轴（俯仰/偏航/横滚）旋转监视；
    - **姿态鼠标 (Head Mouse)**：专属空鼠调节面板，支持模式切换、动力学调优、手势辅助与一键系统授权；
    - **控制与交互 (Controls & Gestures)**：集中管理自适应降噪强度、自动入耳检测、对话感知、耳柄捏按偏好与麦克风通道指定；
    - **智能体接入 (AI Agents)**：AI 提问代答接入管理面板与点头灵敏度测试；
    - **偏好设置 (Preferences)**：常驻系统托盘开关、BLE 开盖就绪弹窗预览、官方硬件代号 (A2564/A2024 等) 与出厂序列号查看。
- 🖱️ **头部姿态控制鼠标 (Head Pose Mouse)**：
  - **基于角速度驱动的 120 Hz 空鼠引擎**：转动头部即以自然角速度驱动光标平滑移动，脖子放松停止转动时光标即刻稳定驻留；
  - **1-Euro 自适应动态防抖滤波**：在静止微动时施加强平滑消除生理性颤抖，快速甩头时自适应提高截止频率保证跟手零滞后；
  - **软死区与非线性加速度曲线**：过滤微小杂波颤动，小角度微转轻松定位微小按钮，快速大转头轻松跨越超宽屏与多显示器；
  - **双操作模式自由切换**：支持相对空鼠模式（Air Mouse，日常最舒适）与绝对指针模式（Laser Pointer，适合演示大屏）；
  - **滚轮滚动模式**：支持一键将头姿上下俯仰切换为鼠标滚轮垂直翻页；
  - **点击手势与耳柄捏按联动**：
    - **耳柄捏按**：单捏鼠标左键，双捏鼠标右键，三捏切换滚轮模式，长按临时暂停/恢复姿态控制；
    - **侧向歪头点击 (Roll Click)**：快速向右歪头触发左键，快速向左歪头触发右键；
    - **静止悬停点击 (Dwell Click)**：光标定位后静止保持 1 秒自动触发左键点击；
  - **极简 Polkit 一键授权**：开启开关时自动检测 `/dev/uinput` 权限，已配置用户静默秒启；未配置用户自动弹出系统级 Polkit (`pkexec`) 授权对话框，免除繁琐的手动配置。
- 🧭 **3D 头部追踪空间音频 (PipeWire + SOFA)**：
  - **真立体声虚拟音箱舞台**：基于 ITU-R BS.775 监听标准，将声源构筑为前方 ±30° 固定立体声音箱对（spatL/spatR），彻底解决单声道声团聚中感；
  - **三轴姿态原子变换**：偏航（水平平移）、俯仰（上下仰视）、横滚（左右歪头产生的反对称升降）经空间坐标旋转实时解算并原子写入 PipeWire filter-chain；
  - **量化与下限节流**：5° 听感不可辨量化 + 50 ms 重建节流，杜绝 100 Hz 高频重建压满 CPU。
- 🤖 **点头代答智能体选择题 (AI Agent Nod Answers)**：
  - 当 ZCode 等桌面智能体弹出 `AskUserQuestion` 时，通过 PreToolUse hook 将问题与选项推送至 RustPods；
  - **点头 N 次自动确认第 N 项**，摇头取消或超时自动回退至原生界面；
  - 采用对称双峰/低谷迟滞状态机与自适应静止基准，即使仰卧或低头阅读姿势依然精准识别；
  - 支持独立的独立浮动问答卡片（Wayland Layer Shell / X11 原生悬浮窗）。
- 🔋 **开盖就绪 iOS 风格动画弹窗**：
  - 结合 BLE Proximity 监听，AirPods 充电盒在电脑旁开启盒盖时，立刻在屏幕右上角弹出优雅的拟态卡片，播放对应耳机代数的立体旋转开盖动画，并显示各部件实时精确电量。
- 🟢 **系统托盘动态环形电量 (`ksni`)**：
  - 符合 Freedesktop StatusNotifierItem 标准，无缝嵌入 Wayland（Niri、Hyprland、Sway、Waybar）及 GNOME/KDE 任务栏；
  - 托盘图标以多分辨率高清环形进度条动态显示当前耳塞电量，低电量警戒变红，充电状态呈鲜绿色并附带闪电徽标（`⚡`）；
  - 右键菜单提供耳机实时电量状态、降噪模式快速切换与主界面呼出。
- ⚡ **纯 Rust 原生 L2CAP AACP 协议栈**：
  - 无需额外守护中间层，基于 `bluer` 直接建立经典蓝牙 L2CAP PSM `0x1001` 加密信道；
  - 自动宣告全部传感器（`request_all_descriptors`），打通 RemoteHID 描述符 `0x800` 约 100 Hz 传感器高频流；
  - 完美实现自动入耳检测（摘下暂停/戴上恢复）与开口说话自动压低音量的对话感知（Conversational Awareness）。

---

## 🛠️ 模块架构

```
rustpods/
├── Cargo.toml
├── LICENSE                        # GNU AGPL-3.0 开源许可协议
├── README.md
├── src/
│   ├── main.rs                    # 应用入口、命令行解析、Tokio 异步运行时与单实例 D-Bus 锁
│   ├── core/                      # 核心蓝牙协议与传感器数学层
│   │   ├── aacp/                  # Apple Accessory Communication Protocol
│   │   │   ├── commands.rs        # 控制命令编码（降噪模式、麦克风偏好、自适应强度等）
│   │   │   ├── events.rs          # 状态通知解码（电量、入耳状态、按柄事件）
│   │   │   ├── packets.rs         # 基础 AACP 报文打包/解包、Opcode 路由分发
│   │   │   ├── remotehid.rs       # RemoteHID 描述符 0x800 协议解析与传感器注册
│   │   │   ├── rtbuddy.rs         # RTBuddy 传感器子协议（传感器公告与采样率协商）
│   │   │   └── mod.rs             # AacpManager 连接维护、生命周期与全局数据流广播
│   │   ├── head_tracking.rs       # 姿态解算（陀螺仪角速度提取、重力参考轴、零偏自适应估计）
│   │   ├── nod_counter.rs         # 点头计数状态机（智能体选择题代答核心）
│   │   ├── spatial.rs             # 空间音频会话声明与状态跟踪
│   │   ├── models.rs              # AirPods 全系硬件型号能力矩阵库
│   │   ├── device.rs              # AirPodsDeviceState 响应式设备状态模型
│   │   └── crypto.rs              # AES-128 解密与蓝牙 RPA 哈希校验
│   ├── system/                    # 桌面生态与系统外设驱动层
│   │   ├── head_mouse/            # 头部姿态鼠标系统
│   │   │   ├── one_euro.rs        # 1-Euro 自适应动态低通滤波器
│   │   │   ├── engine.rs          # 软死区计算、非线性加速度曲线、手势点击判定
│   │   │   ├── uinput.rs          # Linux /dev/uinput 驱动封装与 Polkit 权限管理
│   │   │   ├── controller.rs      # 120 Hz 独立解耦渲染循环与指令分发
│   │   │   └── mod.rs
│   │   ├── audio_follow.rs        # PipeWire HRTF 空间音频 3D 舞台方位角/俯仰角实时调度
│   │   ├── bluetooth.rs           # BlueZ 适配器管理与自动连接服务
│   │   ├── le_monitor.rs          # BLE Proximity 充电盒开盖与广播监听
│   │   ├── agent_hooks.rs         # 智能体 Hook 接入注册表
│   │   ├── zcode_ask.rs           # ZCode 选择题提问代答 Unix Domain Socket 服务
│   │   ├── mpris.rs               # D-Bus MPRIS 播放器控制联动
│   │   ├── tray.rs                # StatusNotifierItem 动态高清系统托盘
│   │   └── config.rs              # 用户偏好设置持久化（原子临时文件保存与容灾备份）
│   └── ui/                        # GPUI 原生图形界面
│       ├── components/
│       │   ├── hero.rs            # 概览页：设备形象大图、重命名操作、状态指示
│       │   ├── battery.rs         # 左右耳与充电盒独立圆形环状进度条与充电闪电
│       │   ├── noise.rs           # 降噪/通透/自适应分段控制器与能力标签卡片
│       │   ├── head_mouse.rs      # 姿态鼠标专属设置页面：模式、动力学滑块、手势开关
│       │   ├── headtracking.rs    # 空间音频页：实时 3D 头部姿态旋转罗盘
│       │   ├── agents.rs          # 智能体接入管理面板
│       │   ├── preferences.rs     # 偏好设置面板（托盘驻留、开盖动画开关）
│       │   └── settings.rs        # 耳机高级控制（入耳检测、麦克风指定、提示音）
│       ├── ask_popup.rs           # 智能体提问悬浮窗 (Wayland Layer Shell / X11)
│       ├── popup.rs               # 充电盒开盖 3D 模型旋转弹窗
│       ├── theme.rs               # 苹果控制中心风格调色板（深浅色无缝切换）
│       ├── state.rs               # GPUI 响应式状态管理总线
│       ├── window.rs              # 主窗口侧边栏导航、滑动条及各模块装配
│       └── mod.rs                 # GPUI Application 生命周期与事件循环
```

---

## 🚀 编译与安装

### 系统前置依赖
- **Rust 工具链**：Rust 1.85+（推荐使用 `rustup` 安装最新稳定版）
- **蓝牙服务**：`bluez` (>= 5.60)
- **输入子系统**：Linux 内核自带 `/dev/uinput` 支持
- **音频系统**：`pipewire`，若需开启 3D 空间音频需安装 SOFA 滤镜模块：
  - Fedora: `sudo dnf install pipewire-module-filter-chain-sofa`
  - Ubuntu/Debian: `sudo apt install libpipewire-0.3-modules`
- **构建依赖**：`clang` / `libclang` 开发库（PipeWire bindgen 构建需要）

### 编译构建

```bash
# 克隆仓库
git clone https://github.com/your-username/rustpods.git
cd rustpods

# 编译 Release 优化版本（默认包含全部核心功能）
cargo build --release
```

编译产物位于 `target/release/rustpods`。

---

## 💻 运行与使用

### 1. 启动图形控制中心
直接运行二进制文件即可呼出原生 GPUI 主窗口：
```bash
./target/release/rustpods
```

### 2. 后台守护进程模式
适合放入系统自启动配置（开机仅驻留托盘，无初始弹窗，双击托盘随时唤出）：
```bash
./target/release/rustpods --no-gui
```

### 3. 头部姿态鼠标快速上手
1. 打开左侧边栏的 **「姿态鼠标」** 页面；
2. 拨动 **「启用姿态鼠标」** 开关：
   - 若系统尚未为普通用户开放 `/dev/uinput` 权限，系统会自动弹出 **Polkit** 授权窗口；
   - 验证通过后驱动立即生效，开关自动就绪；
3. **日常使用小贴士**：
   - **舒适模式**：推荐保持在「相对空鼠模式」，就像手握激光空鼠一样，头部轻微晃动即可精准移动光标；
   - **临时喝水/转头**：长按任意耳柄即可临时暂停/恢复姿态控制，避免光标因自然交谈而偏移；
   - **浏览长网页**：三击耳柄切换至「滚轮模式」，上下抬头低头即可平滑翻阅长文档。

---

## 🎧 3D 头部追踪空间音频设置指南

RustPods 采用与真实声学音响一致的前方 ±30° 监听音箱构型，声音具有宽广自然的声场宽度与纵深感。

1. **下载 SOFA HRTF 数据集**：
   ```bash
   mkdir -p ~/.local/share/rustpods/sofa
   # 可下载公开的 MIT KEMAR 或 SADIE II KU100 数据集放入该目录
   ```
2. **配置 PipeWire filter-chain**：
   将仓库内的 [`contrib/pipewire/99-rustpods-hrtf.conf`](contrib/pipewire/99-rustpods-hrtf.conf) 复制到 `~/.config/pipewire/pipewire.conf.d/`：
   ```bash
   mkdir -p ~/.config/pipewire/pipewire.conf.d/
   cp contrib/pipewire/99-rustpods-hrtf.conf ~/.config/pipewire/pipewire.conf.d/
   systemctl --user restart pipewire
   ```
3. 在系统音频输出中选择 **"RustPods Head-Tracked Stereo"** 虚拟汇聚节点，在「空间音频」页面中即可看到立体声舞台跟随头部实时旋转。

---

## 🧪 自动化测试

项目拥有严格完备的自动化测试体系，覆盖协议解析、加解密、角速度提取、1-Euro 动态滤波算法、软死区与配置迁移：
```bash
cargo test
```
全量 **189 项单元测试** 在本地与 CI 环境均全绿通过。

---

## 📜 开源协议与致谢 (License & Acknowledgements)

RustPods 采用 **[GNU Affero General Public License v3.0](LICENSE)** 协议开源。

### 特别致谢与参考项目
RustPods 的诞生离不开开源社区先行探索者的卓越贡献，我们向以下项目及作者致以诚挚的敬意与感谢：

1. **[LibrePods](https://github.com/kavun-dev/librepods)** (AGPL-3.0)
   - 为 AirPods AACP 私有蓝牙交互协议的逆向工程、L2CAP 通信架构及手势检测奠定了坚实的基础。
2. **[MousePods](https://github.com/starman-tech/mousepods)** (Apache-2.0, Copyright Matteo Gaudioso)
   - 本项目的姿态解算解耦模型（基于重力垂直投影的 `MotionEngine`）与空鼠动力学算法深度借鉴了 MousePods 的研究成果。
3. **[anilsrml/airpods-posture](https://github.com/anilsrml/airpods-posture)** 等社区抓包与姿态协议探索项目。

---

*AirPods、macOS、Safari 均为 Apple Inc. 在美国及其他国家/地区的注册商标。本项目为独立开源社区作品，与 Apple Inc. 无任何附属或官方合作关系。*
