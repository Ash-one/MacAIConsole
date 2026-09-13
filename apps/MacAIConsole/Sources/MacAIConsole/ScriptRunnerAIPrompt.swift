import Foundation

/// 组装给外部 AI 的单文件 Script Runner 生成 prompt。
///
/// 元数据字段与 hook 契约镜像 daemon 的 script runner v1（`script.rs` 的
/// inspect/create 才是最终裁决，AI 产出有偏差会被检查环节以精确错误拦截）；
/// 官方模板原文由 daemon 提供，随注入保持同步。
enum ScriptRunnerAIPrompt {
    static func makeAIPrompt(kind: String, template: String) -> String {
        """
        你在为 MacAI（一个 macOS 本地 AI Runtime）编写一个「单文件 Script Runner」。
        生成前先执行下方的确认环节；确认通过后，只输出一个完整的 Python 代码块，不要任何解释文字。

        ## 第 0 步：确认需求（未满足时不要生成文件）
        检查本次对话和末尾「我的需求」：
        - 如果没有说明要适配的具体模型（没有模型名称或模型主页/仓库网址），只回复一句询问，
          向用户索要模型的网址（如 HuggingFace 仓库链接或模型下载页），并说明拿到网址后才会生成文件。
        - 如果给了模型网址，但你无法确认该模型目录的真实文件构成（权重文件名、config、tokenizer 等），
          同样先询问或如实说明，禁止凭空猜测 required_files 的文件名。
        - 需求完整时才继续生成，并使用用户提供的模型事实填写 dependencies 与 local_detectors。

        ## 文件格式（不符合会被 MacAI 拒绝）
        1. 文件必须恰好包含一个 PEP 723 元数据块：块首行是顶格的 `# /// script`，块内每行以 `# ` 开头，
           块以单独一行顶格的 `# ///` 结束。块内是 TOML。
        2. `[tool.macai]` 只接受以下字段，多写或拼错字段名都会被拒绝：
           - schema = "macai.script-runner.v1"（固定值）
           - id：全局唯一且稳定的 Runner ID，形如 "org.<作者>.<名称>"
           - version：合法 SemVer，如 "0.1.0"
           - capability："chat.v1" / "stt.v1" / "tts.v1" 三选一，一个文件只声明一种能力
           - adapter：非空适配器名，如 "my-tts"
           - model_format = "directory"（固定值）
           - network_during_runtime：布尔。模型文件由 MacAI 模型管理提供，推理过程一般不联网，填 false；
             只有推理确实需要联网才填 true（安装依赖联网与此无关）
           - 可选 [tool.macai.timeouts]：boot_seconds(默认 30) / load_seconds(默认 300) /
             inference_seconds(默认 300) / shutdown_seconds(默认 10)
           - 可选 [[tool.macai.local_detectors]]（推荐提供）：id、reason、required_files（文件名数组）。
             目录中存在全部 required_files 列出的文件时，MacAI 才把该模型目录路由到此 Runner
        3. requires-python 固定为 ">=3.12,<3.13"；dependencies 用 PEP 508 写法声明代码实际 import 的
           直接推理库。每个依赖都必须有 macOS Apple Silicon (arm64) 预编译 wheel；
           除非必要不要引入 PyTorch。

        ## Python 契约（MacAI host 按名字调用这些函数，函数名与签名必须完全一致）
        - 必须实现 `def load(model_path, profile):`：加载模型并返回任意对象（host 会持有该对象）。
          model_path 是 MacAI 识别出的模型目录路径；profile 是含 model_id 的字典。
        - 按 capability 实现其一：
          - chat.v1：`def chat(model, messages, options):` messages 是 OpenAI 格式消息列表；
            返回完整字符串，或逐段 yield 字符串实现流式。
          - stt.v1：`def transcribe(model, wav_path, options):` wav_path 是已归一化的 PCM WAV 路径；
            返回转写文本。
          - tts.v1：`def synthesize(model, text, output_path, options):` 把 WAV 写入 output_path
            （daemon 指定的授权路径），返回该路径或 None；options 是完整请求字典，
            含 text / voice / speed / format / language 键。
        - 可选：`def describe(model):`（报告设备等信息）、`def unload(model):`（清理资源）。
        - 出错直接抛异常（如 ValueError），host 会映射为结构化错误；不要静默 fallback
          （如未知 voice 应抛错而不是悄悄换默认音色）。文件必须自包含，
          不能 import 本地其他模块，也不要读取模型目录之外的本地文件。

        ## 目标能力
        本次要生成的是：\(capabilityTitle(kind))。文件中的 capability 与 hook 必须与之匹配。

        ## 完整官方模板（\(capabilityTitle(kind)) 的字段与 hook 用法以此为准）
        ```python
        \(template)
        ```

        ## 输出前自查
        - 恰好一个 PEP 723 块且正确闭合；字段名与上文完全一致、无多余字段
        - 只声明一种 capability；hook 函数名与签名与目标能力一致
        - required_files 与用户模型目录的真实文件名一致
        - dependencies 均为代码直接 import 的库且写明版本上界

        ## 我的需求
        <请替换本行：要接入的推理库或模型、模型目录里的文件构成、需要的 capability、其他要求>
        """
    }

    static func capabilityTitle(_ kind: String) -> String {
        switch kind {
        case "chat": return "对话（chat.v1）"
        case "stt": return "语音识别（stt.v1）"
        case "tts": return "语音合成（tts.v1）"
        default: return kind
        }
    }
}
