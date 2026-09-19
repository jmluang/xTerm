# xTermius 编译记录

## 项目初始化

使用 Tauri v2 + React + TypeScript 创建项目

```bash
# 创建目录结构
mkdir -p xtermius/src xtermius/src-tauri/src xtermius/src-tauri/icons
```

## 依赖问题及解决方案

### 1. PTY 插件版本

- 初始尝试 `@tauri-apps/plugin-pty` - 不存在
- 使用 `tauri-pty` - 不是 Tauri v2 插件
- 最终使用 `tauri-plugin-pty = "0.2.1"`

### 2. 图标格式问题

错误: `icon is not RGBA`

解决:
```bash
# 使用 plasma 渐变创建 RGBA 图标
convert -size 32x32 plasma:#007acc-#005999 -type TrueColorAlpha 32x32.png
convert -size 128x128 plasma:#007acc-#005999 -type TrueColorAlpha 128x128.png
convert -size 256x256 plasma:#007acc-#005999 -type TrueColorAlpha 128x128@2x.png
convert -size 512x512 plasma:#007acc-# TrueColorAlpha icon.png
sips005999 -type -s format icns icon.png --out icon.icns
convert icon.png -resize 256x256 icon.ico
```

### 3. Tailwind CSS 版本

- shadcn/ui 需要 Tailwind v3
- 最初安装 v4 后遇到 `unknown utility class` 错误
- 切换回 v3 后正常工作

```bash
npm uninstall @tailwindcss/vite tailwindcss
npm install -D tailwindcss@3 postcss autoprefixer
```

### 4. shadcn/ui 安装

需要先安装 Tailwind，然后安装 shadcn 依赖:

```bash
npm install clsx tailwind-merge
npm install class-variance-authority
npm install lucide-react
```

### 5. Dialog 插件

用于文件选择对话框:

```bash
npm install @tauri-apps/plugin-dialog
```

## 编译命令

```bash
# 开发模式
npm run dev

# 生产构建
npm run tauri build -- --bundles dmg
```

## DMG 打包

需要安装 create-dmg:

```bash
brew install create-dmg
```

## 当前状态

- 前端构建: ✅ 成功
- Rust 编译: ✅ 完成 (xtermius 二进制 4.8MB)
- DMG 打包: ⏳ 需要手动执行

## 运行方式

### 直接运行二进制
```bash
open xtermius/src-tauri/target/release/xtermius
```

### 打包 DMG
```bash
cd xtermius
npm run tauri build -- --bundles dmg
```
