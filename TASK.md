* [ ] 明确目标与范围

  * [ ] 需求：从命令行接受**空格/逗号混合**的 glob 或目录；目录递归；输出 `===path===` / `===end of 'path'===`；默认文本读取，可选用 `textutil`（macOS）或自动模式。
  * [ ] 非目标：文档/Office 格式的跨平台解析（除 macOS 的 `textutil` 以外）。
  * [ ] 验收标准：给定示例输入，输出顺序稳定（路径字典序）、路径相对当前目录，找不到文件时退出码=2。

* [ ] 开发环境与工具链

  * [ ] 安装 Rust stable（≥1.70，或按你团队 MSRV 要求）。
  * [x] 初始化项目结构（`cargo new printfiles`）或使用现有画布中的代码。
  * [ ] 本地平台检查：macOS 上确认 `textutil` 可用；Linux/Windows 上确认无 `textutil` 时可优雅回退。

* [ ] 代码组织与基础设施

  * [x] 引入依赖：`clap`（CLI）、`globwalk`（glob/递归）、`which`（探测 `textutil`）。
  * [x] 设定 `Cargo.toml` 元数据（作者、license、repository、categories、keywords）。
  * [ ] 启用代码规范：`rustfmt`、`clippy`（CI 里 `-D warnings`）。
  * [ ] 明确最小支持 Rust 版本（MSRV），并在 CI matrix 中约束。

* [ ] CLI 设计与实现

  * [x] 位置参数 `items: Vec<String>`：允许空格/逗号混合；如 `"src/**/*.rs,docs/*.md" "tests/*.rs"`。
  * [x] 选项 `--reader {text,textutil,auto}`，默认 `text`。
  * [x] 选项 `--ext rs,md`：仅在“参数是目录”时生效，用于扩展名过滤。
  * [x] 友好错误消息与用法示例；找不到匹配时打印提示并返回码 2。
  * [x] 退出码规划：0=成功；1=参数/其他错误；2=无匹配。

* [ ] 匹配与收集逻辑

  * [x] 参数拆分：将所有参数拼接后按逗号再切分，去空白。
  * [x] 对**目录参数**：用 `globwalk` 的 `**/*` 递归遍历，应用 `--ext` 过滤，仅收集文件。
  * [x] 对**普通/带 `**` 的 pattern**：使用 `globwalk` 进行跨平台 glob 展开并收集文件。
  * [x] 去重与排序：使用 `BTreeSet<PathBuf>` 确保有序且去重。
  * [x] 路径显示：以当前工作目录为基准生成相对路径，并去掉开头的 `./`。

* [ ] 读取与输出

  * [x] 标准输出前后分别打印：`===<relpath>===` 与 `===end of '<relpath>'===`。
  * [x] `text` 读取：`fs::read` → `String::from_utf8_lossy`（兼容非 UTF-8）。
  * [x] `textutil` 读取：`which::which("textutil")` 检测；`textutil -convert txt -stdout <file>`，失败则回退到文本读取。
  * [x] `auto` 模式：针对 `rtf/rtfd/doc/docx/html/htm/odt/webarchive` 等扩展优先走 `textutil`。
  * [x] 使用 `BufWriter<std::io::stdout()>` 降低系统调用次数。

* [ ] 错误处理与健壮性

  * [x] 打开/读取失败（权限/损坏）时提供包含路径的错误信息并不中断其他文件（可配置：遇错继续）。
  * [x] 对无效 glob 或空匹配给出告警但不中止整个流程。
  * [ ] 软/硬限制：对超大文件可考虑后续加入 `--max-size`（本版先不实现，留扩展位）。

* [ ] 跨平台注意点

  * [x] macOS：`textutil` 可用；其它平台自动回退。
  * [ ] Windows 路径分隔符显示为相对路径但不强制统一（必要时考虑 `path_slash::PathExt` 转 `/`）。
  * [x] 符号链接：默认 `follow_links(true)`；如需严格区分可增加选项。

* [ ] 测试计划（单元 + 集成）

  * [x] 单元测试：

    * [x] `ext_match`：`"rs,md"` 对应扩展匹配。
    * [x] `should_use_textutil`：不同扩展判断。
    * [x] 相对路径格式化：去除前缀 `./`。
  * [ ] 集成测试（`assert_cmd` + `predicates`）：

    * [ ] 临时目录结构：含文件、子目录、symlink、空目录。
    * [x] 模式组合：空格/逗号混合、含 `**`、目录 + `--ext`。
    * [x] 稳定排序：输出路径序列与快照一致。
    * [x] 无匹配时退出码=2；有权限问题时不中断其他文件。
    * [ ] macOS 条件测试：在有/无 `textutil` 两种情况下输出一致性（允许回退差异）。
  * [ ] 基准/性能（可选）：`hyperfine` 比较有无 `BufWriter`、并发读取的收益。

* [ ] 文档与示例

  * [ ] `README.md`：项目简介、安装、用法、示例、平台差异、退出码说明。
  * [ ] 使用示例：

    * [ ] `printfiles "src/**/*.rs,docs/*.md"`
    * [ ] `printfiles src/**/*.rs docs/*.md,tests/*.rs`
    * [ ] `printfiles src --ext rs,md`
    * [ ] `printfiles reports/**/*.docx --reader textutil`
    * [ ] `printfiles reports,docs --ext md,docx --reader auto`
  * [ ] 贡献指南与开发指引（`CONTRIBUTING.md`，含 `cargo fmt`, `clippy`, 测试命令）。

* [ ] 发行与分发

  * [ ] 添加 `LICENSE`（如 MIT/Apache-2.0）。
  * [ ] 语义化版本（SemVer），Tag 与发布说明（CHANGELOG）。
  * [ ] 产物打包：`cargo build --release`；附带 SHA 校验值。
  * [ ] 可选：集成 `cargo-dist` 产出多平台压缩包。
  * [ ] 可选：Homebrew Formula（macOS），在 README 标注安装命令。

* [ ] 可选/扩展功能（Roadmap）

  * [ ] `--relative-from <dir>`：指定相对根目录。
  * [ ] `--max-size <bytes>`：跳过大文件并提示。
  * [ ] `--jobs <n>`：并行读取（注意输出顺序需要缓冲后统一打印）。
  * [ ] `--binary=skip|hex|base64`：二进制文件策略。
  * [ ] `--sort name|size|mtime`：排序策略可选。
  * [ ] `--follow-links/--no-follow-links`：符号链接策略切换。
  * [ ] `--quiet`/`--verbose`：日志级别。
  * [ ] shell 补全与 man page（`clap_complete` / `clap_mangen`）。

* [ ] 最终验收

  * [ ] 以典型目录（含多级子目录、混合文件类型）运行，核对输出示例。
  * [ ] 锁定版本，创建 Release，更新 README 安装说明。
