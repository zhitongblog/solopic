# SoloPic · 免费批量图像处理

免费、开源、离线的批量图像处理工具（CLI 命令为 `pic`）。一个核心，三种用法：**图形界面（GUI）**、**命令行（CLI）**、**MCP Server**（给 Claude 等 AI 助手直接调用）。

**多语言**：图形界面支持中文 / English / 日本語 / 한국어 / Français / Deutsch / Español / Português（右上角切换，默认跟随系统语言）；CLI 与处理报告支持中/英（跟随系统，或用环境变量 `PIC_LANG=zh|en` 指定）。

## 四大功能

| 功能 | 说明 |
|------|------|
| ✂ **批量裁剪** | 按像素从边缘切，如"左边切 100px、下边切 57px"——每张图按自身尺寸裁，尺寸不一致也没问题 |
| 📝 **批量改名** | 按映射文件改名：每行 `旧名,新名`（如 `1.png,张三.png`），一键全部改好，可撤销 |
| 🎚 **批量调整** | 亮度 / 对比度 / 饱和度 / 锐度 / 灰度，系数 1.0 = 不变 |
| ✨ **智能增强** | 扫描/手机拍照的文档一键变清楚：自动去阴影、光照均衡、背景变白、文字加深、自动纠斜（±12°） |

支持格式：png / jpg / webp / bmp / gif / tif。自动应用手机照片的 EXIF 方向。默认输出到 `pic-output` 子文件夹，不动原图（也可选择覆盖）。

## 构建

需要 [Rust](https://rustup.rs/)（Windows 上另需 WebView2，Win10/11 自带）：

```
cargo build --release
```

产物在 `target/release/`：`pic.exe`（CLI）、`pic-app.exe`（GUI）、`pic-mcp.exe`（MCP Server）。
打安装包：`cargo install tauri-cli --version "^2" && cargo tauri build`（在 `crates/pic-app` 下）。

## 图形界面

```
pic-app.exe            # 打开后选择文件夹
pic-app.exe D:\照片    # 直接打开某个文件夹（也可以把文件夹拖到 exe 上）
```

左侧点选要处理的图片（默认全选），右侧选功能页签，实时预览"原图 → 处理后"对比，点"开始处理"。

## 命令行

```powershell
# 批量裁剪：左切 100px、下切 57px，输出到 D:\照片\pic-output
pic crop --left 100 --bottom 57 D:\照片

# 批量改名：默认只预览，加 -x 才真正执行；执行后自动生成 undo 日志
pic rename D:\照片 --map 名单.txt          # 预览
pic rename D:\照片 --map 名单.txt -x       # 执行
pic rename D:\照片 --undo pic-rename-undo-xxxx.json -x   # 撤销

# 批量调整：亮度 +20%、对比度 +10%
pic adjust --brightness 1.2 --contrast 1.1 D:\照片

# 智能增强：彩色增强（默认）/ gray 灰度 / bw 黑白扫描件
pic enhance D:\照片
pic enhance --mode bw D:\扫描件
pic enhance --max-deskew 30 D:\扫描件   # 纠斜搜索范围默认 ±20°，可调 1~45

# 其它
pic ls D:\照片          # 列出图片
pic <命令> --json       # JSON 输出，便于脚本
pic <命令> --overwrite  # 覆盖原图（默认输出到 pic-output 子目录）
```

### 映射文件格式

```
# 井号开头是注释；分隔符支持英文逗号、中文逗号、Tab
1.png,张三.png
a.png，李四          ← 新名不写扩展名时自动沿用原扩展名（李四.png）
b.png	王五.png
```

- 编码：UTF-8（带不带 BOM 都行）/ UTF-16（Excel 另存为的 CSV 也能读）
- 执行前自动校验：文件不存在、目标重名、目标已被占用、Windows 非法字符/保留名等，有问题的行单独报错，不影响其他行
- `a.png → b.png` 与 `b.png → a.png` 互换、链式改名都安全（两阶段改名）

## MCP Server（给 AI 助手用）

在 Claude Code / Claude Desktop 的 MCP 配置中加入（参考仓库根目录 `mcp.json`）：

```json
{
  "mcpServers": {
    "pic": {
      "command": "D:\\code\\pic\\target\\release\\pic-mcp.exe"
    }
  }
}
```

提供工具：`batch_crop` / `batch_rename` / `batch_adjust` / `batch_enhance` / `list_images`。
之后可以直接对 AI 说："把 D:\照片 里所有图左边切 100 像素" 或 "按 D:\名单.txt 批量改名"。

## 架构

```
crates/
├── pic-core   核心库（裁剪/改名/调整/增强，纯 Rust，MIT）
├── pic-cli    命令行壳（clap）
├── pic-mcp    MCP Server 壳（rmcp）
└── pic-app    图形界面壳（Tauri 2 + 原生 HTML/JS，无 Node 依赖）
```

一核多壳：核心逻辑只有一份，三个入口都是薄封装。未来可基于 pic-core 编译移动端 / wasm。
界面文案在 `crates/pic-app/ui/i18n.js`（8 语言字典），引擎消息在核心库内 `tr(中文, English)` 双语。
技术选型与同类开源软件调研见 [docs/RESEARCH.md](docs/RESEARCH.md)。

## 说明与限制

- 智能增强为经典 CV 管线（除法归一化去阴影 + Sauvola 自适应二值化 + 投影法纠斜），无 AI 模型依赖，CPU 毫秒级，批量千张分钟级
- 多帧动图（GIF 动画）会跳过不处理；WebP 输出为无损编码
- 处理后的图片不保留 EXIF 元数据（方向已应用到像素，文档场景无影响）
- JPEG 输出质量 95

## License

MIT
