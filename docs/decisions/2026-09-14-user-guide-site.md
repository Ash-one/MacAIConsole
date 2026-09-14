# 网页版用户使用指南（docs/guide/）

Status: implemented

## 问题

MacAI 的"怎么用"此前散落在根 README、`apps/MacAIConsole/README.md` 与决策记录里：普通用户
要从混合了开发者内容的 README 中自行拼出安装、装引擎、下模型、接入 API 的完整路径；遇到
Gatekeeper 拦截、引擎编译依赖、注册被拒等问题时没有按症状组织的排障入口。`docs/index.html`
项目主页是概览式介绍，不承载操作步骤。缺一份按用户任务组织、可从网页直接访问的使用说明。

## 决策

在 `docs/guide/` 新增单页静态文档站点 `index.html`，与项目主页同一套设计语言（深色、
系统字体、复制按钮），内容按任务组织：

- 入门：定位与边界、系统要求、DMG / 源码安装、五分钟跑通（MiniCPM5 主线）；
- 日常使用：控制台五页导览、七个 Runner 引擎（含 whisper.cpp 编译特例与 Script Runner
  信任边界）、模型获取三途径与注册语义（ID 约束、inspect 路由、硬选择、生命周期）；
- 三大能力：Chat（含 reasoning_content、session_id 缓存与 temp=0 分岔怪癖）、STT（格式
  归一化、multipart 字节语义、引擎选型）、TTS（音色滚轮与试听、情感指令、speed 现状）；
- 参考：macai 全命令表、HTTP API endpoints 表、数据目录与日志、环境变量；
- 帮助：按症状折叠式排障（10 条）与安全边界、当前限制。

站点为零外部依赖的静态 HTML（无框架、无 CDN、无构建步骤），本地双击或任意静态服务器可开，
未来开启 GitHub Pages（docs/ 为根）即可直接在线访问。与主页互链（主页导航与页脚 →
`guide/`，指南顶栏 → `../index.html`），根 README 与本目录索引均链接指南。

指南是摘要层：不复制易漂移的契约细节，行为断言以 README、代码与决策记录为准，页脚注明
最后更新日期。

## 备选

- **mdBook / mkdocs 等文档工具链**：需要新构建依赖与工具流程，违背仓库低依赖约定，且
  Markdown 工具生成的默认主题与项目视觉不一致。放弃。
- **GitHub Wiki**：仓库外，不走 PR 审阅，无法与代码在同一 bounded change 中同步收敛。放弃。
- **扩充 README**：README 已承担当前实现状态 owner，再叠加按任务组织的操作手册会进一步
  挤压其作为事实索引的可读性；开发者内容与普通用户内容混排的问题依旧。放弃。

## 后果与边界

- 指南与 README 存在内容重叠（安装、命令、endpoints），两者需人工同步：README 仍是当前
  行为的 owner，指南在页脚声明"行为以 README、代码与决策记录为准"。指南不带构建期校验，
  漂移靠复查发现——这是放弃工具链换取零依赖的已知代价。
- 指南不引入搜索、版本化或多语言；单页 + 侧边栏锚点对当前体量（15 节）够用，文档量显著
  增长时再考虑拆分。

## 验证

- `grep -E '(src|href)="https?://' docs/guide/index.html` 仅命中指向 GitHub 的外链，资产
  （logo、截图）全部相对引用，离线可开。
- 截图目验：桌面（1440px）与窄屏（≤960px 侧边栏折叠为目录）渲染正常，复制按钮、锚点、
  scrollspy 工作。
- `docs/index.html` 导航与页脚的 `guide/` 链接可达 `docs/guide/index.html`；根 README 与
  本索引已加入指南链接。
