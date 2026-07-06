# 批量图像处理工具 · 开源调研与技术选型报告

> 调研日期：2026-07-05 ｜ 四路并行调研：同类开源 GUI / 映射改名工具 / 多平台技术栈 / 智能文档增强

## 0. 需求清单（客户）

| # | 需求 | 说明 |
|---|------|------|
| A | 批量裁剪 | 按像素从边缘切，如"左切 100px、下切 57px"（**相对边缘语义**，图片尺寸可以不一致） |
| B | 批量重命名 | 按映射文件改名，每行 `旧名,新名`（如 `1.png,张三.png`） |
| C | 批量调整 | 亮度、对比度、饱和度等 |
| D | 智能增强 | 一键把扫描/拍照的文档图片"弄清楚"（类似扫描全能王魔法滤镜） |

## 1. 核心结论：市场空白成立，自研有差异化价值

调研了 XnConvert、Converseen、BIMP/Batcher、digiKam BQM、nomacs、Caesium、ImBatch、IrfanView、FotoKilof 等所有主流批量图像工具后确认：

- **没有任何现成软件（开源或免费闭源）同时满足 A+B+C**，更不用说 A+B+C+D。
- **A 的"按边相对裁剪"在所有 GUI 中零覆盖**——全都只有固定矩形/画布锚点裁剪，只有 ImageMagick CLI 的 `-chop`/`-shave` 原生支持此语义。
- **B 的"映射文件改名"在所有图像工具中零覆盖**——只有专门的重命名器支持（Bulk Rename Utility 闭源；PowerToys 的对应 feature request 挂了 6 年无人做）。
- C 覆盖最好的是 digiKam BQM（GPL，巨型程序）；D 没有一个批量 GUI 内置。

## 2. 优中取优：借鉴清单

### 2.1 UX 交互借鉴（闭源/GPL，只学交互不碰代码）

| 来源 | 借鉴点 |
|------|--------|
| XnConvert | **动作流水线**：输入 → 动作列表（可排序、每步独立参数）→ 输出 三段式；预设可保存复用；前后对比预览 |
| IrfanView | **显式处理顺序**对话框——让用户控制裁剪/调色执行次序 |
| Bulk Rename Utility | 映射改名四步流：**导入 → 逐行校验 → 预览新旧对照列 → 执行** |
| Advanced Renamer | CSV 导入向导（分隔符/表头/列选择/基准目录），错误行标红 |

### 2.2 代码/库可直接复用（许可干净）

| 项目 | 许可 | 用途 |
|------|------|------|
| OpenCV | Apache-2.0 | 智能增强管线全部算子 |
| Pillow | MIT-CMU | 基础裁剪/调整（现有原型已用） |
| scikit-image | BSD-3 | Sauvola 二值化参考实现 |
| noteshrink | MIT | 笔记类图片背景提纯+调色板量化（约500行，可移植） |
| jdeskew | MIT | 自动纠斜（FFT 径向投影，精度最高，~100行核心） |
| DocShadow-SD7K | MIT | AI 去文档硬阴影（有现成 ONNX 移植） |
| NAFNet | MIT | AI 去模糊（可选深度增强） |
| Batcher (GIMP3) | BSD-3 | "动作+条件筛选"批处理架构（Python） |
| FotoKilof | MIT | Python+Pillow/IM 批处理实现参考 |
| ImageMagick | Apache-2 类 | 可选高性能后端（经 wand 绑定） |
| F2 | MIT | 映射改名的冲突检测/两阶段改名设计（Go，借鉴设计） |

⚠️ 不可复用：Fred's textcleaner（非商用许可，连"重写移植"都禁止——用标准方法自行实现同类效果）；DE-GAN / DocEnTr（非商用）；一切 GPL 项目代码（digiKam/ScanTailor/unpaper 等，只能学思路或外部进程调用）。

### 2.3 映射改名工程要点（借鉴 F2 + 超越它）

1. **默认 dry-run 预览**，显式确认才执行；预览输出与真实执行共用同一套冲突检测。
2. **执行前全量验证 7 类冲突**：非法字符 / 目标已存在 / 重复目标 / 空名 / 超长(255) / Windows 尾部句点+保留名(CON/PRN/AUX/NUL/COM1-9/LPT1-9) / 源不存在。
3. **两阶段改名断环**：先全部改临时名再改目标名，天然支持 a↔b 互换、链式改名（F2 靠重排序解决不了纯环，我们的两阶段方案更优）。
4. **编码**：接受 UTF-8（剥 BOM——F2 不剥 BOM 是著名的坑）+ UTF-16 探测；macOS 上做 NFC/NFD 归一化匹配。
5. **仅大小写改名**在 Win/macOS 上需经临时名两步完成。
6. 部分失败不中止整批，结束时汇总；每次执行落 undo 日志支持撤销。
7. 新名无扩展名时自动补原扩展名（对非技术用户友好，`张三` → `张三.png`）。

## 3. 智能增强（需求 D）方案

**默认走经典 CV 管线（零模型、CPU 单张 50–300ms），AI 作可选"深度增强"开关。**

推荐管线（OpenCV，约 300–500 行）：

```
文档判别（直方图亮峰 + 低饱和度占比 + Otsu 前景比，<10ms，保守阈值）
 → (可选) 页面四边形检测 + 透视矫正
 → 阴影/光照均衡：大核形态学估计背景 → 除法归一化（division normalization）
 → 输出模式分支：
     · 增艳彩色：逐通道归一化 + 饱和度×1.2~1.5 + 百分位拉伸 + 白点钳制
     · 黑白文档：Sauvola 二值化（自实现 ~40 行，绕开 cv2.ximgproc 绑定 bug）
     · 笔记量化：noteshrink 思路（MIT 可直接抄）
 → 去噪（median/形态学）→ USM 锐化
 → 自动纠斜（jdeskew 算法，MIT）
```

- 批量 1000 张：经典管线约 2 分钟；AI 路线无 GPU 要 0.5–10 小时 → AI 只做可选项。
- AI 可选组件（许可全 MIT）：DocShadow（去硬阴影，ONNX 现成）+ NAFNet（去手抖模糊）。
- 误判成本不对称：把风景照误当文档破坏性大 → 判别阈值保守 + 提供"强制/跳过"开关。
- Rust 未来移植：imageproc 已有全部硬骨头算子（自适应阈值/形态学/Hough/透视），缺口（Sauvola/CLAHE/除法归一化）合计约 300 行可自写，无根本障碍。

## 4. 多平台技术栈：推荐 Czkawka 式「一核多壳」

六方案对比结论（详见调研原文）：

| 排名 | 方案 | 一句话 |
|------|------|--------|
| 🥇 | **Tauri 2 + Rust core** | 安装包 3–10MB；core crate 可编译到桌面三平台+iOS/Android/wasm；CLI/MCP/GUI 三形态=三个薄壳包同一个核 |
| 🥈 | Electron + sharp | 开发最快但 150–300MB 体积税，移动端无路 |
| 🥉 | Python 原型续命 | 迭代最快，但每平台单独打包+macOS 公证之痛，是过渡方案 |
| 4 | Qt/C++ | 单人成本过高，GUI 代码无法复用到其他形态 |
| 5 | 纯 PWA+wasm | showDirectoryPicker 仅 Chromium（~27% 覆盖），文件夹批处理不成立 |
| 6 | Flutter desktop | 图像生态最弱，最后还得 FFI 接 Rust，不如直接 Tauri |

架构标杆是 **Czkawka**（MIT 的 Rust core + CLI/GTK/Slint/Android 四壳）：

```
pic-core  (Rust lib, MIT)  ← 裁剪/映射改名/调整/智能增强；image + imageproc + rayon
├── pic-cli   (clap)       ← 命令行
├── pic-mcp   (rmcp)       ← MCP server
└── pic-app   (Tauri 2)    ← GUI，前端直接复用现有 Web UI（HTML/JS 原样迁移）
```

注：智能增强的 AI 可选组件在 Rust 侧经 onnxruntime/ort crate 挂载；经典管线纯 Rust 实现。

## 5. 建议的落地路线（两步走）

1. **第一步（立即）**：完成现有 Python + Pillow + OpenCV + 本地 Web UI 版本——A/B/C/D 四需求全覆盖，先交付给客户用起来、验证需求细节（裁剪语义、映射格式、增强效果预期）。CLI + MCP + Web GUI 三形态齐全。
2. **第二步（正式版）**：按上面的 Rust 一核多壳架构重写核心，Web UI 资产原样迁入 Tauri 2；Python 版退役为需求验证原型。核心沉淀在 MIT 的 pic-core，未来移动端/wasm 都不用重写。

两步走的关键收益：客户这周就能用上，而多平台正式版的 UI 和算法设计已经过真实使用验证。
