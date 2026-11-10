# 视觉 Agent

`ai.screen_agent` 接收任务目标 `objective` 和可见结果 `expected`，通过
`desktop.observe` 截图，让支持图片的已配置模型选择下一步，再调用现有
`desktop.click`、`desktop.type`、`desktop.key`、`desktop.scroll` 或等待动作。
每次动作后重新截图；模型返回 `done` 时，再用一张新截图独立验证结果，
验证失败不会报告任务成功。

```yaml
apiVersion: lumorpa.io/v1
kind: Flow
metadata:
  id: visual-report
spec:
  capabilities:
    desktop: [screen, mouse, keyboard]
    llm: ["vision/report-model"]
  steps:
    - id: open_report
      action: ai.screen_agent
      with:
        objective: "在当前应用中打开月报"
        expected: "月报标题和内容显示在屏幕上"
        model: "vision/report-model"
        max_actions: 10
        timeout_ms: 120000
        max_tokens: 16000
        max_cost_usd_micro: 1000000
        confirm_actions: true
```

将 `vision/report-model` 换成模型源中的实际配置。需要宿主屏幕录制及输入
权限；默认每次输入操作都要确认。缺少确认通道会在截图前失败，界面拒绝
后不会继续操作。取消在等待模型和确认时也会生效，超时会向原生任务发送
中断信号。连续三次操作没有画面变化会停止，要求人工检查。

成功结果包含 `verified`、执行过的动作名称、Tokens 和估算费用，不包含
截图。AI 调用写入既有用量账本；失败、超时或取消时未返回的用量标为未知，
交给任务对账处理。费用按仓库价格表估算；单次请求的输入 Tokens 和账单
在响应后才能确定，最后一次请求可能超过所设预算，之后不会继续执行动作。
这不是模型供应商的账户额度控制。

图片只在内存中传递给配置的模型；使用远程模型时图片会发送到该模型服务。
`desktop.observe` 单独作为流程步骤使用时，其输出仍遵循普通步骤的变量和
存储规则。坐标支持负屏幕原点及截图缩放映射。允许的模型操作使用固定
枚举及大小、坐标范围检查，不能选择任意工具或执行 shell。

```sh
CARGO_INCREMENTAL=0 cargo test -p lumo-ai --locked --test screen_agent
```

回归验证重新截图、独立后置条件验证、错误工具及越界坐标拒绝、取消、
内部超时、中断信号、用量未知与操作上限。测试使用模型和屏幕替身，
证明运行契约；真实视觉模型的任务成功率仍需在目标桌面上验收。
