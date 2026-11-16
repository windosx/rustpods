# Arch Linux (AUR) 维护指南

本项目在 `contrib/arch/` 下提供了两个 PKGBUILD 模板：
- **`PKGBUILD-bin`**（推荐，对应 AUR 包名 `rustpods-bin`）：直接从 GitHub Releases 拉取预编译二进制并打包，安装速度快，无需用户在本地编译 GPUI 与 PipeWire 绑定。
- **`PKGBUILD`**（对应 AUR 包名 `rustpods`）：从 GitHub 源码 Release 编译。

---

## 提交至 AUR 步骤

### 1. 首次推送到 AUR（以 `rustpods-bin` 为例）

确保你已在 [AUR 官网](https://aur.archlinux.org/) 注册账号并上传了 SSH 公钥。

```bash
# 1. 克隆 AUR 空仓库
git clone ssh://aur@aur.archlinux.org/rustpods-bin.git
cd rustpods-bin

# 2. 复制模板并重命名为 PKGBUILD
cp /path/to/rustpods/contrib/arch/PKGBUILD-bin ./PKGBUILD

# 3. 校验并生成 .SRCINFO（Arch 必须包含该文件）
# 确保已更新 pkgver 与正确的 sha256sums
updpkgsums   # 自动计算并填入 release tarball 的 sha256
makepkg --printsrcinfo > .SRCINFO

# 4. 本地测试打包与安装
makepkg -si

# 5. 提交并推送到 AUR
git add PKGBUILD .SRCINFO
git commit -m "Initial release v0.1.0"
git push origin master
```

---

## 新版本更新流程

当发布新版本（例如 `v0.2.0`）时：

1. 修改 `PKGBUILD` 中的 `pkgver=0.2.0` 与 `pkgrel=1`。
2. 运行 `updpkgsums` 自动更新下载校验码。
3. 重新生成 `.SRCINFO`：
   ```bash
   makepkg --printsrcinfo > .SRCINFO
   ```
4. 提交并推送：
   ```bash
   git commit -am "Update to v0.2.0"
   git push origin master
   ```
