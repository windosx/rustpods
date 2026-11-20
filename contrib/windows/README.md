# RustPods for Windows

## 1. 安装与使用 (Installation)

RustPods 提供两种 Windows 分发形式：
- **安装包（推荐）**：下载 `RustPods-Setup-v*.exe`，双击安装向导即可完成安装。支持自动创建桌面与开始菜单快捷方式、开机后台自启。
- **便携绿色版 (Portable)**：下载 `rustpods-v*-windows-x86_64.zip`，解压即用。

---

## 2. 双模运行机制 (Dual-Mode Architecture)

### 模式一：免驱原生模式 (Native Driverless Mode) —— 推荐普通用户
**开箱即用，无需任何特殊配置，100% 绿色安全**：
- 基于 Windows WinRT 蓝牙硬件级广告过滤，实时检测充电盒开盖与电量。
- 360° 开盖动态弹窗，左右耳与充电盒独立环形电量显示。
- 任务栏系统托盘实时动态 32 位环形电量图标。
- 基于 Windows 全局系统媒体传输控件（GSMTC）实现入耳自动暂停/戴上恢复播放。
- 完整的 GPU 加速控制中心。

### 模式二：驱动增强模式 (Driver Enhanced Mode) —— 进阶用户
当系统中安装有 MagicAAP 蓝牙 L2CAP 内核驱动或运行命名管道桥接时，RustPods 会**自动识别并激活增强模式**：
- 解锁降噪、通透、自适应音频等 ANC 模式切换。
- 解锁 100 Hz Apple RemoteHID 姿态传感器遥测。
- 解锁头部姿态空鼠、激光指针与滚轮滚动（Win32 `SendInput`，免管理员权限）。
- 解锁智能体工具权限点头/摇头手势审批。
- 解锁 OpenTrack UDP (`127.0.0.1:4242`) 6DOF 头部姿态广播，直连微软模拟飞行（MSFS）、DCS World 等仿真游戏。

---

## 3. 如何在 Windows 上启用 MagicAAP 驱动？

由于微软 Windows 蓝牙协议栈在用户态限制了经典蓝牙 L2CAP PSM `0x1001` 的直接连接，进阶功能依赖第三方内核驱动 `MagicAAP.sys`。

> [!WARNING]
> 由于 `MagicAAP.sys` 是第三方未签名驱动，加载该驱动需要 Windows 进入**测试模式**（Test Mode）。若您电脑上安装了对内核驱动签名有极高要求的反作弊游戏（如 Valorant/Vanguard），请保持使用免驱原生模式即可。

### 安装步骤：
1. 以管理员身份打开 PowerShell 或终端：
   ```cmd
   bcdedit /set testsigning ON
   ```
2. 重启电脑进入 Windows 测试模式（屏幕右下角会显示“测试模式”水印）。
3. 安装 MagicAAP 驱动（可通过设备管理器或右键 `.inf` 文件点击“安装”）。
4. 启动 RustPods，程序将自动检测到 `\\.\MagicAAP` 并解锁全部增强功能。
