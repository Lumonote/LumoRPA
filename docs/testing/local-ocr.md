# 本地 OCR 使用与自检

在桌面“模型源”中选择 Local OCR，选择 ModelScope OCR 预设并下载，保存并激活该模型源。模型文件保存在应用数据目录的 `models/ocr` 下；下载先写入临时目录，通过配置、处理器、分词器、权重及全部分片检查后才发布。

每个模型分别显示三种状态：

- **模型文件**：未下载、文件不完整、文件完整。文件完整仅代表所需文件存在且结构检查通过。
- **运行环境**：未检查、缺少依赖、环境就绪。点击“检查运行环境”检查所选 Python、PyTorch、Transformers、Pillow、Accelerate 和模型需要的 Transformers 类；GLM-OCR、PaddleOCR-VL 还检查 Torchvision 是否可导入，不加载权重。
- **本地识别**：未验证、验证通过、验证失败。文件完整后点击“测试本地识别”，加载模型并识别内置空白图片；失败原因会显示在卡片中。此检查验证加载与生成链路，不能衡量真实文档准确率。

刷新列表不会加载模型。Python 默认为 `python3`，可在启动应用前用 `LUMO_OCR_PYTHON` 指定安装了所需依赖的解释器。各预设仍可能要求额外的模型专用依赖；以自检的具体错误为准。

GLM-OCR、PaddleOCR-VL 的多模态处理器还需要与 PyTorch 版本兼容的 `torchvision`。仅安装 `torch`、`transformers`、`Pillow` 和 `accelerate` 不足以使用这两个引擎；`torchvision` 缺失或导入失败时，运行环境会显示“缺少依赖”及具体错误。DeepSeek-OCR 当前通过 `AutoTokenizer`、`AutoModel` 和模型自身的 `infer()` 接口调用，通用探针不额外要求 `torchvision`；模型自定义代码的其他依赖由本地识别自检确认。

以下流程仅需图片读取权限。先保存并激活本地模型源，再将图片放到实际授权路径，修改示例中的 `/absolute/path/`：

```yaml
apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: local-document-ocr
  ai:
    enabled: true
    diagnose_on_failure: false
    budget:
      max_calls_per_run: 4
spec:
  capabilities:
    fs.read: ["/absolute/path/**"]
  steps:
    - id: recognize
      action: doc.ocr
      with:
        image: "/absolute/path/scan.png"
        ocr:
          engine: local
          model: "modelscope/ZhipuAI/GLM-OCR"
          allow_cloud: false
          timeout_ms: 120000
```

结果包含 `text`、`lines`、`source: ocr_local` 和 `left_machine: false`。对扫描 PDF 使用 `doc.parse` 的同名 `ocr` 配置，会识别可提取的整页扫描图片并保留本地来源标记。

本地路径使用离线模式和 `local_files_only`。文件缺失、依赖缺失、超时或推理失败都会报告错误，**不会自动转用云端**。远程 OCR 仍需同时开启 `ocr.allow_cloud` 和对应的 `network` 或 `llm` 能力授权。

2026-11-10 已通过项目 CLI 加载官方 GLM-OCR 真实权重，完成中英文、数字及退化扫描图片的 5 份本地样本验收；忽略空白后的 CER 均为 0，独立进程耗时约 3.73–4.74 秒，采样进程树 RSS 约 2.72–2.73 GiB。完整样本、版本、权重校验值、原始日志及限制见[设备验收记录](reports/2026-11-10-glm-ocr/README.md)。这些合成样本建立了真实模型基线；其他预设及实际业务扫描件仍需分别验收。

## 真实文档批量验收

`scripts/ocr-acceptance.py` 使用已有模型目录，通过实际构建的 Lumo CLI 执行 `doc.ocr`，逐份文档输出 CER（字符错误率）、耗时、采样内存和本地来源。脚本不会下载模型；模型、Python 依赖缺失或识别失败都会令验收失败，不会跳过样本。

先通过桌面应用下载模型并配置 Python，再准备代表性图片和人工核对的参考文本。建议包含中文、英文、数字、表格及实际扫描质量的文档，避免仅用空白图片作为准确率样本。在自建的验收目录保存 `manifest.json`，图片和参考文件路径相对此文件解析：

```json
{
  "version": 1,
  "model": "modelscope/ZhipuAI/GLM-OCR",
  "cases": [
    {
      "id": "invoice-cn",
      "image": "fixtures/invoice.png",
      "expectedFile": "references/invoice.txt",
      "maxCer": 0.05,
      "maxElapsedMs": 120000,
      "maxPeakRssMb": 8192,
      "timeoutMs": 180000
    },
    {
      "id": "contract-amount",
      "image": "fixtures/amount.png",
      "expectedText": "合同金额100000",
      "maxCer": 0.0
    }
  ]
}
```

每个样本必须填写唯一 `id`、非空本地图片，以及 `expectedText` 或 UTF-8 `expectedFile` 中的一项。`maxCer` 默认 0.10，`timeoutMs` 默认 180000；耗时和内存阈值可省略。示例阈值仅展示配置格式，应按设备和业务要求调整。

```sh
cargo build -p lumo-cli
python3 scripts/ocr-acceptance.py /absolute/path/acceptance/manifest.json \
  --home /absolute/path/existing-lumo-home \
  --lumo target/debug/lumo \
  --python /absolute/path/ocr-venv/bin/python \
  --output /tmp/lumo-ocr-acceptance
```

`--home` 必须指向实际包含 `models/ocr` 的现有 Lumo 数据目录；`--python` 可省略以沿用 `LUMO_OCR_PYTHON` 或 `python3`。`--model` 可以覆盖 manifest 的模型。脚本在 macOS/Linux 上运行，要求系统提供可执行的 `ps`，以采样进程树内存。

每个样本都会启动独立 CLI 和模型进程，耗时包含加载模型的冷启动时间。内存是约每 50 ms 采样 CLI 及其子进程 RSS 的最大合计值，以字节记录；`maxPeakRssMb` 的单位为 MiB。该值不包含 GPU 显存，亦不能替代操作系统提供的精确峰值；多个进程共享的页面可能被重复计入。执行失败或超时时，已采集的耗时和内存仍保留在报告中。

CER 为 Unicode 字符编辑距离除以参考字符数，包含插入、删除、替换；插入很多内容时可大于 1。比较前统一 NFC 和换行，默认保留空格；只有业务允许忽略版式空白时才添加 `--ignore-whitespace`。单份标准化参考及识别结果最多 25000 字符。报告中的 `microCerOfScoredCases` 按参考字符数汇总有识别结果的样本，仍需同时检查整次验收 `status` 和失败列表，不能用该汇总掩盖未完成的样本。

输出目录包含 `report.json` 和每份文档的 CLI stdout/stderr 日志。退出码 0 表示所有样本都通过；配置错误、推理错误、任何阈值超限或来源不符合 `source: ocr_local` / `left_machine: false` 均退出 1。脚本生成仅授权当前图片读取的临时流程、关闭云端回退和失败诊断、使用临时 Local 模型源并启用离线环境变量；不改动已有模型源配置，且以 `--no-store` 避免写入执行历史。日志及报告包含识别文本，应保存在合适的本地目录。

脚本自身的计算、输入校验、子进程调用、本地来源拒绝及超时回收测试可单独执行：

```sh
python3 -m unittest discover -s scripts -p test_ocr_acceptance.py -v
```

这些测试使用替代 CLI 验证验收工具契约，不代表真实 OCR 模型准确率。真实 GLM-OCR 基线及可复用图片已保存在[设备验收记录](reports/2026-11-10-glm-ocr/README.md)；实际设备和业务文档可沿用同一脚本及 manifest 格式验证。
