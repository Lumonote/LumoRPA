# 本地 OCR 使用与自检

在桌面“模型源”中选择 Local OCR，选择 ModelScope OCR 预设并下载，保存并激活该模型源。模型文件保存在应用数据目录的 `models/ocr` 下；下载先写入临时目录，通过配置、处理器、分词器、权重及全部分片检查后才发布。

每个模型分别显示三种状态：

- **模型文件**：未下载、文件不完整、文件完整。文件完整仅代表所需文件存在且结构检查通过。
- **运行环境**：未检查、缺少依赖、环境就绪。点击“检查运行环境”检查所选 Python、PyTorch、Transformers、Pillow、Accelerate 和模型需要的 Transformers 类，不加载权重。
- **本地识别**：未验证、验证通过、验证失败。文件完整后点击“测试本地识别”，加载模型并识别内置空白图片；失败原因会显示在卡片中。此检查验证加载与生成链路，不能衡量真实文档准确率。

刷新列表不会加载模型。Python 默认为 `python3`，可在启动应用前用 `LUMO_OCR_PYTHON` 指定安装了所需依赖的解释器。各预设仍可能要求额外的模型专用依赖；以自检的具体错误为准。

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

本次验证使用临时模型文件、替代模型模块及真实 Python 进程测试下载、目录选择和调用契约，未下载 GB 级真实权重。真实模型在具体设备上的加载、内存需求和识别准确率尚需通过上述自检及代表性文档验收。
